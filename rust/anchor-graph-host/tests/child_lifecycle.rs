use anchor_graph_host::{ChildRunControl, GraphCatalog, InProcessGraphHost};
use anchor_runtime::{Cancellation, graph::*};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use tokio::sync::Notify;

#[derive(Clone, Copy, PartialEq)]
enum RunnerFault {
    None,
    Acquire,
    Save,
}

// Admission uses the actual filesystem store. Faults apply only after the
// catalog has registered the child and the shared Kernel Runner takes over.
struct FaultStore {
    inner: FileRunStore,
    registered: Arc<AtomicBool>,
    fault: RunnerFault,
}

impl RunStore for FaultStore {
    fn load(&self, id: &str) -> Result<Option<GraphRunRecord>, GraphError> {
        self.inner.load(id)
    }

    fn save(&self, record: &GraphRunRecord) -> Result<(), GraphError> {
        if self.registered.load(Ordering::SeqCst) && self.fault == RunnerFault::Save {
            return Err(GraphError::Unsupported(
                "injected Runner save failure".into(),
            ));
        }
        self.inner.save(record)
    }

    fn acquire_lease(&self, id: &str) -> Result<Box<dyn RunLease>, GraphError> {
        if self.registered.load(Ordering::SeqCst) && self.fault == RunnerFault::Acquire {
            return Err(GraphError::Unsupported(
                "injected Runner acquire failure".into(),
            ));
        }
        self.inner.acquire_lease(id)
    }
}

struct Catalog {
    store: FileRunStore,
    registered: Arc<AtomicBool>,
    begins: AtomicUsize,
    ends: AtomicUsize,
    finished: Mutex<Vec<RunStatus>>,
    events: Arc<Mutex<Vec<&'static str>>>,
}

impl GraphCatalog for Catalog {
    fn snapshot(&self, _: &str) -> Result<Option<GraphSnapshot>, GraphError> {
        Ok(Some(GraphSnapshot::from_authoring(json!({
            "objective":"child", "entry":"work", "agents":{},
            "ops":{"work":{"run":"true"}},
            "nodes":[{"id":"work","op":"work"}], "edges":[]
        }))?))
    }

    fn graph_ancestry(&self, _: &str) -> Result<Vec<String>, GraphError> {
        Ok(vec!["parent".into()])
    }

    fn begin_child_execution<'a>(
        &'a self,
        run_id: &'a str,
        _: &'a str,
        cancellation: Cancellation,
    ) -> Pin<Box<dyn Future<Output = Result<ChildRunControl, GraphError>> + Send + 'a>> {
        Box::pin(async move {
            // The Ready child is durable, and the admission lease is released
            // before execution registration and Runner lease acquisition.
            assert_eq!(self.store.load(run_id)?.unwrap().status, RunStatus::Ready);
            drop(self.store.acquire_lease(run_id)?);
            assert!(!self.registered.swap(true, Ordering::SeqCst));
            self.begins.fetch_add(1, Ordering::SeqCst);
            self.events.lock().unwrap().push("begin");
            Ok(ChildRunControl::linked_to_parent(cancellation))
        })
    }

    fn end_child_execution<'a>(
        &'a self,
        _: &'a str,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            // Cleanup is asynchronous; callers must await it before returning
            // an error or projecting a terminal result.
            tokio::task::yield_now().await;
            assert!(self.registered.swap(false, Ordering::SeqCst));
            self.ends.fetch_add(1, Ordering::SeqCst);
            self.events.lock().unwrap().push("end");
        })
    }

    fn child_execution_finished<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
        status: RunStatus,
    ) -> Pin<Box<dyn Future<Output = Result<(), GraphError>> + Send + 'a>> {
        Box::pin(async move {
            assert!(!self.registered.load(Ordering::SeqCst));
            self.finished.lock().unwrap().push(status);
            Ok(())
        })
    }
}

struct Artifacts;

impl ArtifactPort for Artifacts {
    fn freeze<'a>(
        &'a self,
        key: &'a InvocationKey,
        _: &'a NodeCompletion,
    ) -> Pin<Box<dyn Future<Output = Result<CommitRef, GraphError>> + Send + 'a>> {
        Box::pin(async move {
            Ok(CommitRef {
                id: format!("commit-{}", key.durable_key()),
                node_id: key.node_id.clone(),
                invocation: key.invocation,
            })
        })
    }

    fn resolve<'a>(
        &'a self,
        _: &'a CommitRef,
    ) -> Pin<Box<dyn Future<Output = Result<Value, GraphError>> + Send + 'a>> {
        Box::pin(async { Ok(json!({})) })
    }
}

struct Nodes {
    cancel: bool,
    entered: Notify,
    cancelled: Notify,
    events: Arc<Mutex<Vec<&'static str>>>,
}

impl NodeExecutionPort for Nodes {
    fn capabilities(&self) -> NodeExecutionCapabilities {
        NodeExecutionCapabilities {
            agent: false,
            op_run: true,
            host_operations: false,
            exact_provider_request_budget: true,
        }
    }

    fn completion_fact<'a>(
        &'a self,
        _: &'a InvocationKey,
    ) -> Pin<Box<dyn Future<Output = Result<CompletionFact, GraphError>> + Send + 'a>> {
        Box::pin(async { Ok(CompletionFact::NotStarted) })
    }

    fn execute<'a>(
        &'a self,
        request: NodeExecutionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<NodeExecutionOutcome, GraphError>> + Send + 'a>> {
        Box::pin(async move {
            self.events.lock().unwrap().push("node entered");
            let outcome = if self.cancel {
                self.entered.notify_one();
                self.cancelled.notified().await;
                assert!(request.cancellation.load(Ordering::SeqCst));
                NodeExecutionOutcome::Cancelled
            } else {
                assert!(!request.cancellation.load(Ordering::SeqCst));
                NodeExecutionOutcome::Completed(NodeCompletion {
                    submission: "done".into(),
                    route: None,
                    model_requests: 0,
                    output: json!({"done":true}),
                })
            };
            self.events.lock().unwrap().push("node exited");
            Ok(outcome)
        })
    }
}

struct Control;

impl RunControl for Control {
    fn pause_requested(&self) -> bool {
        false
    }
    fn stop_requested(&self) -> bool {
        false
    }
    fn cancellation(&self) -> Cancellation {
        Cancellation::default()
    }
}

fn parent_call(store: &FileRunStore, spec: &Value) -> (CallIdentity, Value) {
    let snapshot = GraphSnapshot::from_authoring(json!({
        "objective":"parent", "entry":"invoke", "agents":{},
        "ops":{"invoke":{"call":spec}},
        "nodes":[{"id":"invoke","op":"invoke"}], "edges":[]
    }))
    .unwrap();
    let mut parent = GraphRunRecord::create(snapshot, json!({})).unwrap();
    let input = json!({"input":{}});
    parent.status = RunStatus::Running;
    parent.invocations.insert("invoke".into(), 1);
    parent.passes.insert("invoke".into(), 1);
    parent.cursor = Some(RunCursor {
        node_id: "invoke".into(),
        key: InvocationKey {
            run_id: parent.run_id.clone(),
            graph_digest: parent.graph_digest.clone(),
            node_id: "invoke".into(),
            invocation: 1,
        },
        input_commits: vec![],
        prepared_input: input.clone(),
    });
    let identity = CallIdentity {
        parent_run_id: parent.run_id.clone(),
        parent_graph_digest: parent.graph_digest.clone(),
        node_id: "invoke".into(),
        invocation: 1,
        call_spec_digest: format!("{:x}", Sha256::digest(serde_json::to_vec(spec).unwrap())),
    };
    store.save(&parent).unwrap();
    (identity, input)
}

async fn exercise(fault: RunnerFault, cancel: bool) {
    let root = tempfile::tempdir().unwrap();
    let registered = Arc::new(AtomicBool::new(false));
    let events = Arc::new(Mutex::new(Vec::new()));
    let store = FaultStore {
        inner: FileRunStore::new(root.path()),
        registered: registered.clone(),
        fault,
    };
    let catalog = Catalog {
        store: store.inner.clone(),
        registered,
        begins: AtomicUsize::new(0),
        ends: AtomicUsize::new(0),
        finished: Mutex::new(Vec::new()),
        events: events.clone(),
    };
    let nodes = Nodes {
        cancel,
        entered: Notify::new(),
        cancelled: Notify::new(),
        events: events.clone(),
    };
    let spec = json!({"graph":"child", "mode":"wait"});
    let (identity, input) = parent_call(&store.inner, &spec);
    let host =
        InProcessGraphHost::<_, _, _, Control>::new(&catalog, &store, &Artifacts, &nodes, &Control);
    let cancellation = Cancellation::default();
    let call = host.call(&identity, &spec, &input, &[], cancellation.clone());
    let result = if cancel {
        let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            tokio::join!(call, async {
                nodes.entered.notified().await;
                cancellation.store(true, Ordering::SeqCst);
                nodes.cancelled.notify_one();
            })
        })
        .await
        .expect("cooperative child cancellation must finish");
        result
    } else {
        call.await
    };
    assert_eq!(catalog.begins.load(Ordering::SeqCst), 1);
    assert_eq!(catalog.ends.load(Ordering::SeqCst), 1);
    assert!(!catalog.registered.load(Ordering::SeqCst));
    // Both admission and Runner leases are released even when Runner fails.
    let run_id = format!("call-{}", identity.durable_key());
    drop(store.inner.acquire_lease(&run_id).unwrap());
    let child = store.inner.load(&run_id).unwrap().unwrap();
    let finished = catalog.finished.lock().unwrap();
    if fault != RunnerFault::None {
        let error = result.unwrap_err().to_string();
        let operation = if fault == RunnerFault::Acquire {
            "acquire"
        } else {
            "save"
        };
        assert!(error.contains(&format!("execute child Run: unsupported graph capability: injected Runner {operation} failure")), "{error}");
        assert_eq!(*events.lock().unwrap(), ["begin", "end"]);
        assert!(finished.is_empty());
        assert_eq!(child.status, RunStatus::Ready);
        assert!(child.cursor.is_none());
    } else {
        assert_eq!(
            *events.lock().unwrap(),
            ["begin", "node entered", "node exited", "end"]
        );
        if cancel {
            assert!(matches!(result.unwrap(), GraphCallOutcome::Waiting { .. }));
            assert_eq!(child.status, RunStatus::Stopped);
            assert!(child.cursor.is_some());
            assert!(child.results.is_empty());
        } else {
            assert!(
                matches!(result.unwrap(), GraphCallOutcome::Completed { output, .. } if output == json!({"done":true}))
            );
            assert_eq!(child.status, RunStatus::Completed);
            assert!(!cancellation.load(Ordering::SeqCst));
        }
        assert_eq!(*finished, [child.status]);
    }
}

#[tokio::test]
async fn successful_wait_releases_registration_without_cancelling() {
    exercise(RunnerFault::None, false).await;
}

#[tokio::test]
async fn runner_acquire_error_releases_registration_and_preserves_error() {
    exercise(RunnerFault::Acquire, false).await;
}

#[tokio::test]
async fn runner_save_error_releases_registration_and_preserves_error() {
    exercise(RunnerFault::Save, false).await;
}

#[tokio::test]
async fn cooperative_cancellation_waits_for_node_exit_and_preserves_stopped_cursor() {
    exercise(RunnerFault::None, true).await;
}
