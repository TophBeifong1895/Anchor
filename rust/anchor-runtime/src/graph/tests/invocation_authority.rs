use super::*;

struct InvocationAuthorityNodes {
    inner: FakeNodes,
    authority: bool,
}

impl NodeExecutionPort for InvocationAuthorityNodes {
    fn capabilities(&self) -> NodeExecutionCapabilities {
        self.inner.capabilities()
    }
    fn invocation_plugin_authority(&self, _: &GraphRunRecord) -> Result<bool, GraphError> {
        Ok(self.authority)
    }
    fn resolve_plugins(&self, ids: &[String]) -> Result<Vec<PluginBinding>, GraphError> {
        self.inner.resolve_plugins(ids)
    }
    fn completion_fact<'a>(
        &'a self,
        key: &'a InvocationKey,
    ) -> Pin<Box<dyn Future<Output = Result<CompletionFact, GraphError>> + Send + 'a>> {
        self.inner.completion_fact(key)
    }
    fn execute<'a>(
        &'a self,
        request: NodeExecutionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<NodeExecutionOutcome, GraphError>> + Send + 'a>> {
        self.inner.execute(request)
    }
}

#[tokio::test]
async fn invocation_authority_is_explicit_and_keeps_the_run_admission_immutable() {
    for authority in [false, true] {
        let mut snapshot = graph(&["work"], &[], "work");
        snapshot.nodes[0].plugins = vec!["removed".into()];
        let mut record = GraphRunRecord::create(snapshot, Value::Null).unwrap();
        let binding = PluginBinding {
            id: "removed".into(),
            digest: "old-code".into(),
            resources: vec![],
            mcp_servers: vec![],
            authorization_digest: "old-grants"
                .try_into()
                .expect("non-empty fixture authority"),
        };
        record.plugin_bindings.insert(binding.id.clone(), binding);
        record.plugin_bindings_initialized = true;
        let original = record.clone();
        // Current Library deliberately has no old Plugin. The explicitly opted
        // in Host owns the invocation's replacement policy, not this baseline.
        let nodes = InvocationAuthorityNodes {
            inner: FakeNodes::default(),
            authority,
        };
        let store = MemStore::default();
        let artifacts = MemoryArtifacts::default();
        let control = Control::default();
        let result = GraphRunner::new(&store, &artifacts, &nodes, &control)
            .run(record)
            .await
            .unwrap();
        assert_eq!(
            result.status,
            if authority {
                RunStatus::Completed
            } else {
                RunStatus::Failed
            }
        );
        assert_eq!(
            nodes.inner.calls.lock().unwrap().len(),
            usize::from(authority)
        );
        assert_eq!(result.snapshot, original.snapshot);
        assert_eq!(result.plugin_bindings, original.plugin_bindings);
    }
}

#[tokio::test]
async fn invocation_authority_does_not_establish_a_missing_admission_baseline() {
    let mut snapshot = graph(&["work"], &[], "work");
    snapshot.nodes[0].plugins = vec!["missing".into()];
    let record = GraphRunRecord::create(snapshot, Value::Null).unwrap();
    let nodes = InvocationAuthorityNodes {
        inner: FakeNodes::default(),
        authority: true,
    };
    let store = MemStore::default();
    let artifacts = MemoryArtifacts::default();
    let control = Control::default();
    let result = GraphRunner::new(&store, &artifacts, &nodes, &control)
        .run(record)
        .await
        .unwrap();
    assert_eq!(result.status, RunStatus::Failed);
    assert!(
        result
            .error
            .unwrap()
            .contains("valid admitted Plugin record")
    );
    assert!(nodes.inner.calls.lock().unwrap().is_empty());
}
