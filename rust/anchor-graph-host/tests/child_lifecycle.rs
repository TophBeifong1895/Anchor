use anchor_graph_host::{
    CallRun, CallRunAccess, ChildExecutionPort, ChildRunControl, GraphCatalog, InProcessGraphHost,
    PluginCatalog,
};
use anchor_kernel::{Outcome, Phase};
use anchor_runtime::{Cancellation, graph::*};
use serde_json::{Value, json};
use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use tokio::sync::Notify;
mod fixture {
    use super::{CallRun, CallRunAccess, ChildExecutionPort, PluginCatalog};
    include!("support/kernel.rs");
}
use fixture::{Executor, Runs, parent_call};

#[derive(Clone, Copy, PartialEq)]
enum Fault {
    None,
    Acquire,
    Commit,
}

struct Catalog {
    runs: Arc<Runs>,
    registered: AtomicBool,
    fault: Fault,
    begins: AtomicUsize,
    ends: AtomicUsize,
    finished: Mutex<Vec<Phase>>,
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
            assert_eq!(self.runs.load(run_id)?.unwrap().phase, Phase::Executing);
            // The admission lease ends before execution registration.
            drop(self.runs.acquire_lease(run_id)?);
            assert!(!self.registered.swap(true, Ordering::SeqCst));
            match self.fault {
                Fault::Acquire => self.runs.fail_lease.store(true, Ordering::SeqCst),
                Fault::Commit => self
                    .runs
                    .kernel
                    .store()
                    .fail_commit
                    .store(true, Ordering::SeqCst),
                Fault::None => {}
            }
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
        phase: Phase,
    ) -> Pin<Box<dyn Future<Output = Result<(), GraphError>> + Send + 'a>> {
        Box::pin(async move {
            assert!(!self.registered.load(Ordering::SeqCst));
            self.finished.lock().unwrap().push(phase);
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
async fn exercise(fault: Fault, cancel: bool) {
    let root = tempfile::tempdir().unwrap();
    let runs = Arc::new(Runs::new(root.path()));
    let events = Arc::new(Mutex::new(Vec::new()));
    let catalog = Catalog {
        runs: runs.clone(),
        registered: AtomicBool::new(false),
        fault,
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
    let executor = Executor {
        runs: &runs,
        artifacts: &Artifacts,
        nodes: &nodes,
        plugins: None,
    };
    let spec = json!({"graph":"child", "mode":"wait"});
    let (identity, input) = parent_call(&runs, &spec);
    let host = InProcessGraphHost::new(&catalog, &*runs, &Artifacts, &nodes)
        .with_child_executor(&executor, false);
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
    let run_id = anchor_graph_host::child_run_id(&identity);
    drop(runs.acquire_lease(&run_id).unwrap());
    let child = runs.load(&run_id).unwrap().unwrap();
    let finished = catalog.finished.lock().unwrap();
    if fault != Fault::None {
        let error = result.unwrap_err().to_string();
        assert!(
            error.contains(if fault == Fault::Acquire {
                "injected Kernel acquire failure"
            } else {
                "injected Kernel commit failure"
            }),
            "{error}"
        );
        assert!(
            error.starts_with("unsupported graph capability: execute child Run:"),
            "{error}"
        );
        if fault == Fault::Acquire {
            assert_eq!(*events.lock().unwrap(), ["begin", "end"]);
        } else {
            assert_eq!(
                *events.lock().unwrap(),
                ["begin", "node entered", "node exited", "end"]
            );
        }
        assert!(finished.is_empty());
        assert_eq!(child.phase, Phase::Executing);
        assert!(child.record.results.is_empty());
    } else {
        assert_eq!(
            *events.lock().unwrap(),
            ["begin", "node entered", "node exited", "end"]
        );
        if cancel {
            assert!(matches!(result.unwrap(), GraphCallOutcome::Waiting { .. }));
            assert!(matches!(
                child.phase,
                Phase::Waiting {
                    waiter: anchor_kernel::Waiter::Human { .. }
                }
            ));
            assert!(child.record.results.is_empty());
            let resource = runs
                .kernel
                .load(&validator::run_resource_id(&run_id))
                .unwrap()
                .unwrap();
            let frontier = validator::GraphValidator::frontier(&resource).unwrap();
            assert_eq!(frontier.len(), 1);
            assert_eq!(frontier[0].node.id, "work");
            assert_eq!(frontier[0].invocation, 1);
        } else {
            assert!(
                matches!(result.unwrap(), GraphCallOutcome::Completed { output, .. } if output == json!({"done":true}))
            );
            assert_eq!(
                child.phase,
                Phase::Settled {
                    result: Outcome::Done
                }
            );
            assert!(!cancellation.load(Ordering::SeqCst));
        }
        assert_eq!(*finished, [child.phase]);
    }
}
#[tokio::test]
async fn successful_wait_releases_registration_without_cancelling() {
    exercise(Fault::None, false).await;
}
#[tokio::test]
async fn kernel_acquire_error_releases_registration_and_preserves_error() {
    exercise(Fault::Acquire, false).await;
}
#[tokio::test]
async fn kernel_commit_error_releases_registration_and_preserves_error() {
    exercise(Fault::Commit, false).await;
}
#[tokio::test]
async fn cooperative_cancellation_waits_for_node_exit_and_preserves_open_invocation() {
    exercise(Fault::None, true).await;
}
