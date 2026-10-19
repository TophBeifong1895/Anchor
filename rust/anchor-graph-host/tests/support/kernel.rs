// Included by unit and integration tests with the Call port types in scope.
use anchor_kernel::{
    Action, Authority, FileStore, Kernel, Phase, Principal, PrincipalKind, Record, Resource, Store,
    StoreError, Submission,
};
use anchor_runtime::graph::{
    scheduler::{KernelScheduler, kernel_error},
    validator::{GraphValidator, call_identity, record_of, run_resource, run_resource_id},
};
use anchor_runtime::{Cancellation, graph::*};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    future::Future,
    path::Path,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

struct TestAuthority;
impl Authority for TestAuthority {
    fn permits(&self, principal: &Principal, action: Action, _: &str) -> bool {
        match principal.kind {
            PrincipalKind::Agent => action == Action::Write,
            PrincipalKind::Code => {
                matches!(action, Action::Write | Action::Request | Action::Settle)
            }
            PrincipalKind::Human => action == Action::Request,
        }
    }
}

pub struct FixtureStore {
    file: FileStore,
    pub fail_commit: AtomicBool,
}
impl Store for FixtureStore {
    fn load(&self, id: &str) -> Result<Option<Resource>, StoreError> {
        self.file.load(id)
    }
    fn create(&self, resource: &Resource) -> Result<(), StoreError> {
        self.file.create(resource)
    }
    fn commit(&self, id: &str, record: &Record) -> Result<(), StoreError> {
        if self.fail_commit.swap(false, Ordering::SeqCst) {
            return Err(std::io::Error::other("injected Kernel commit failure").into());
        }
        self.file.commit(id, record)
    }
    fn history(&self, id: &str) -> Result<Vec<Record>, StoreError> {
        self.file.history(id)
    }
    fn list(&self, prefix: &str) -> Result<Vec<String>, StoreError> {
        self.file.list(prefix)
    }
}

pub struct Runs {
    pub kernel: Kernel<FixtureStore>,
    leases: Mutex<BTreeMap<String, Arc<Semaphore>>>,
    pub fail_lease: AtomicBool,
}
impl Runs {
    pub fn new(root: &Path) -> Self {
        let mut kernel = Kernel::new(
            FixtureStore {
                file: FileStore::new(root).unwrap(),
                fail_commit: AtomicBool::new(false),
            },
            Box::new(TestAuthority),
        )
        .unwrap();
        kernel
            .install(validator::GRAPH_VALIDATOR, Arc::new(GraphValidator))
            .unwrap();
        Self {
            kernel,
            leases: Mutex::default(),
            fail_lease: AtomicBool::new(false),
        }
    }
    pub fn control(&self, id: &str, operation: &str) {
        let resource = self.kernel.load(&run_resource_id(id)).unwrap().unwrap();
        let verdict = self
            .kernel
            .submit(
                &Principal::new(scheduler::OPERATOR_ID, PrincipalKind::Human),
                0,
                Submission::new(&resource.id, resource.version, Action::Request)
                    .content(json!({"control":operation})),
            )
            .unwrap();
        assert!(verdict.is_accepted(), "{verdict:?}");
    }
}
struct Lease {
    _permit: OwnedSemaphorePermit,
}
impl RunLease for Lease {}
impl CallRunAccess for Runs {
    fn load(&self, id: &str) -> Result<Option<CallRun>, GraphError> {
        self.kernel
            .load(&run_resource_id(id))
            .map_err(kernel_error)?
            .map(|resource| {
                let phase = resource
                    .phase()
                    .cloned()
                    .ok_or_else(|| GraphError::CorruptRun("Run has no task phase".into()))?;
                Ok(CallRun {
                    record: record_of(&resource)?,
                    phase,
                })
            })
            .transpose()
    }
    fn admit(&self, record: &GraphRunRecord) -> Result<(), GraphError> {
        self.kernel
            .create(run_resource(record, 100)?)
            .map_err(kernel_error)
    }
    fn acquire_lease(&self, id: &str) -> Result<Box<dyn RunLease>, GraphError> {
        if self.fail_lease.swap(false, Ordering::SeqCst) {
            return Err(GraphError::Unsupported(
                "injected Kernel acquire failure".into(),
            ));
        }
        let semaphore = self
            .leases
            .lock()
            .unwrap()
            .entry(id.to_owned())
            .or_insert_with(|| Arc::new(Semaphore::new(1)))
            .clone();
        Ok(Box::new(Lease {
            _permit: semaphore
                .try_acquire_owned()
                .map_err(|_| GraphError::RunBusy(id.to_owned()))?,
        }))
    }
    fn validate_parent<'a>(
        &'a self,
        identity: &'a CallIdentity,
        input: &'a Value,
        commits: &'a [CommitRef],
    ) -> Pin<Box<dyn Future<Output = Result<GraphRunRecord, GraphError>> + Send + 'a>> {
        Box::pin(async move {
            let resource = self
                .kernel
                .load(&run_resource_id(&identity.parent_run_id))
                .map_err(kernel_error)?
                .ok_or_else(|| GraphError::CorruptRun("missing Kernel parent".into()))?;
            if !matches!(
                resource.phase(),
                Some(
                    Phase::Executing
                        | Phase::Waiting {
                            waiter: anchor_kernel::Waiter::Child { .. }
                        }
                )
            ) {
                return Err(GraphError::CorruptRun(
                    "parent cannot dispatch a Graph Call".into(),
                ));
            }
            let record = record_of(&resource)?;
            let dispatch = GraphValidator::frontier(&resource)?
                .into_iter()
                .find(|dispatch| {
                    call_identity(&record, dispatch).is_ok_and(|found| found == *identity)
                })
                .ok_or_else(|| GraphError::CorruptRun("Call is not the Kernel frontier".into()))?;
            let key = InvocationKey {
                run_id: record.run_id.clone(),
                graph_digest: record.graph_digest.clone(),
                node_id: identity.node_id.clone(),
                invocation: identity.invocation,
            };
            let prepared =
                scheduler::dispatch_input(&InputArtifacts, &record, &dispatch, key).await?;
            if prepared.prepared_input != *input || dispatch.inputs != commits {
                return Err(GraphError::CorruptRun(
                    "Call input or provenance differs from Kernel frontier".into(),
                ));
            }
            Ok(record)
        })
    }
}

struct InputArtifacts;
impl ArtifactPort for InputArtifacts {
    fn freeze<'a>(
        &'a self,
        _: &'a InvocationKey,
        _: &'a NodeCompletion,
    ) -> Pin<Box<dyn Future<Output = Result<CommitRef, GraphError>> + Send + 'a>> {
        Box::pin(async { unreachable!() })
    }
    fn resolve<'a>(
        &'a self,
        _: &'a CommitRef,
    ) -> Pin<Box<dyn Future<Output = Result<Value, GraphError>> + Send + 'a>> {
        Box::pin(async { Ok(json!({})) })
    }
}

pub fn parent_call(runs: &Runs, spec: &Value) -> (CallIdentity, Value) {
    let snapshot = GraphSnapshot::from_authoring(json!({
        "objective":"parent", "entry":"invoke", "agents":{},
        "ops":{"invoke":{"call":spec}},
        "nodes":[{"id":"invoke","op":"invoke"}], "edges":[]
    }))
    .unwrap();
    let parent = GraphRunRecord::create(
        snapshot,
        json!({"session":"forged", "channel":{"sender_id":"forged"}}),
    )
    .unwrap();
    runs.admit(&parent).unwrap();
    let resource = runs
        .kernel
        .load(&run_resource_id(&parent.run_id))
        .unwrap()
        .unwrap();
    let dispatch = GraphValidator::frontier(&resource).unwrap().remove(0);
    let identity = call_identity(&parent, &dispatch).unwrap();
    (
        identity,
        json!({"input":parent.input,"committed_inputs":[]}),
    )
}

pub struct Executor<'a, A, N> {
    pub runs: &'a Runs,
    pub artifacts: &'a A,
    pub nodes: &'a N,
    pub plugins: Option<&'a dyn PluginCatalog>,
}
struct CurrentNodes<'a, N> {
    nodes: &'a N,
    plugins: Option<&'a dyn PluginCatalog>,
}
impl<N: NodeExecutionPort> NodeExecutionPort for CurrentNodes<'_, N> {
    fn capabilities(&self) -> NodeExecutionCapabilities {
        self.nodes.capabilities()
    }
    fn resolve_plugins(&self, ids: &[String]) -> Result<Vec<PluginBinding>, GraphError> {
        match self.plugins {
            Some(plugins) => plugins.resolve(ids),
            None => self.nodes.resolve_plugins(ids),
        }
    }
    fn invocation_plugin_authority(&self, record: &GraphRunRecord) -> Result<bool, GraphError> {
        self.nodes.invocation_plugin_authority(record)
    }
    fn completion_fact<'a>(
        &'a self,
        key: &'a InvocationKey,
    ) -> Pin<Box<dyn Future<Output = Result<CompletionFact, GraphError>> + Send + 'a>> {
        self.nodes.completion_fact(key)
    }
    fn execute<'a>(
        &'a self,
        request: NodeExecutionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<NodeExecutionOutcome, GraphError>> + Send + 'a>> {
        self.nodes.execute(request)
    }
    fn observed_plugin_views(&self, key: &InvocationKey) -> BTreeMap<String, String> {
        self.nodes.observed_plugin_views(key)
    }
    fn observed_plugin_digests(&self, key: &InvocationKey) -> BTreeMap<String, String> {
        self.nodes.observed_plugin_digests(key)
    }
}
impl<A: ArtifactPort, N: NodeExecutionPort> ChildExecutionPort for Executor<'_, A, N> {
    fn execute<'a>(
        &'a self,
        child: CallRun,
        cancellation: Cancellation,
    ) -> Pin<Box<dyn Future<Output = Result<CallRun, GraphError>> + Send + 'a>> {
        Box::pin(async move {
            let _lease = self.runs.acquire_lease(&child.record.run_id)?;
            let nodes = CurrentNodes {
                nodes: self.nodes,
                plugins: self.plugins,
            };
            let resource = KernelScheduler {
                kernel: &self.runs.kernel,
                observer: None,
                boundary: None,
                artifacts: self.artifacts,
                nodes: &nodes,
                cancellation: cancellation.clone(),
            }
            .run(&run_resource_id(&child.record.run_id))
            .await?;
            let resource = if cancellation.load(Ordering::SeqCst)
                && resource.phase() == Some(&Phase::Executing)
            {
                self.runs.control(&child.record.run_id, "stop");
                self.runs
                    .kernel
                    .load(&resource.id)
                    .map_err(kernel_error)?
                    .unwrap()
            } else {
                resource
            };
            Ok(CallRun {
                record: record_of(&resource)?,
                phase: resource.phase().cloned().unwrap(),
            })
        })
    }
}
