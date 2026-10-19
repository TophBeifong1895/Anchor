// Test wiring around the production Kernel scheduler; no execution policy.
use crate::graph::{
    scheduler::KernelScheduler,
    validator::{self, GraphValidator, record_of},
};
use anchor_kernel::{
    Action, Authority, Kernel, MemoryStore, Phase, Principal, PrincipalKind, Resource, Store,
    Submission, Verdict,
};

struct TestAuthority;
impl Authority for TestAuthority {
    fn permits(&self, _: &Principal, _: Action, _: &str) -> bool {
        true
    }
}

struct Harness<S: Store = MemoryStore> {
    kernel: Kernel<S>,
    id: String,
}

impl Harness {
    fn memory(record: GraphRunRecord) -> Self {
        Self::with_store(MemoryStore::default(), record)
    }
}

impl<S: Store> Harness<S> {
    fn with_store(store: S, record: GraphRunRecord) -> Self {
        let resource = validator::run_resource(&record, u64::MAX).unwrap();
        let id = resource.id.clone();
        let mut kernel = Kernel::new(store, Box::new(TestAuthority)).unwrap();
        kernel
            .install(validator::GRAPH_VALIDATOR, Arc::new(GraphValidator))
            .unwrap();
        kernel.create(resource).unwrap();
        Self { kernel, id }
    }

    fn reload(store: S, run_id: &str) -> Self {
        let mut kernel = Kernel::new(store, Box::new(TestAuthority)).unwrap();
        kernel
            .install(validator::GRAPH_VALIDATOR, Arc::new(GraphValidator))
            .unwrap();
        Self {
            kernel,
            id: validator::run_resource_id(run_id),
        }
    }

    fn resource(&self) -> Resource {
        self.kernel.load(&self.id).unwrap().unwrap()
    }
    fn record(&self) -> GraphRunRecord {
        record_of(&self.resource()).unwrap()
    }
    fn phase(&self) -> Phase {
        self.resource().phase().unwrap().clone()
    }
    fn failure_detail(&self) -> String {
        match self.phase() {
            Phase::Settled {
                result:
                    Outcome::Failed { detail, .. }
                    | Outcome::Escalated { detail }
                    | Outcome::Unknown { detail },
            } => detail,
            other => panic!("expected a failed or escalated Kernel outcome: {other:?}"),
        }
    }
    async fn pending_input<A: ArtifactPort + ?Sized>(
        &self,
        artifacts: &A,
        node: &str,
    ) -> scheduler::DispatchInput {
        let record = self.record();
        let dispatch = self
            .frontier()
            .into_iter()
            .find(|dispatch| dispatch.node.id == node)
            .unwrap();
        let key = InvocationKey {
            run_id: record.run_id.clone(),
            graph_digest: record.graph_digest.clone(),
            node_id: node.into(),
            invocation: dispatch.invocation,
        };
        scheduler::dispatch_input(artifacts, &record, &dispatch, key)
            .await
            .unwrap()
    }
    fn frontier(&self) -> Vec<validator::Dispatch> {
        GraphValidator::frontier(&self.resource()).unwrap()
    }

    fn request(&self, control: &str) -> Verdict {
        self.kernel
            .submit(
                &Principal::new("anchor/operator", PrincipalKind::Human),
                0,
                Submission::new(&self.id, self.resource().version, Action::Request)
                    .content(serde_json::json!({"control":control})),
            )
            .unwrap()
    }

    async fn run<A: ArtifactPort + ?Sized, N: NodeExecutionPort + ?Sized>(
        &self,
        artifacts: &A,
        nodes: &N,
    ) -> Result<Resource, GraphError> {
        self.drive(
            artifacts,
            nodes,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .await
    }

    async fn drive<A: ArtifactPort + ?Sized, N: NodeExecutionPort + ?Sized>(
        &self,
        artifacts: &A,
        nodes: &N,
        cancellation: crate::Cancellation,
    ) -> Result<Resource, GraphError> {
        KernelScheduler {
            kernel: &self.kernel,
            observer: None,
            boundary: None,
            artifacts,
            nodes,
            cancellation,
        }
        .run(&self.id)
        .await
    }
}

fn admitted<N: NodeExecutionPort + ?Sized>(snapshot: GraphSnapshot, nodes: &N) -> GraphRunRecord {
    let ids = snapshot
        .nodes
        .iter()
        .flat_map(|node| node.plugins.iter().map(|reference| reference.id.clone()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let plugins = if ids.is_empty() {
        Vec::new()
    } else {
        nodes.resolve_plugins(&ids).unwrap()
    };
    GraphRunRecord::create_with_plugins(snapshot, Value::Null, "fixture", plugins).unwrap()
}
