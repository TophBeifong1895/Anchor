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
        let binding = PluginBinding {
            id: "removed".into(),
            digest: "old-code".into(),
            resources: vec![],
            mcp_servers: vec![],
            authorization_digest: "old-grants"
                .try_into()
                .expect("non-empty fixture authority"),
        };
        let record = GraphRunRecord::create_with_plugins(
            snapshot,
            Value::Null,
            "invocation-authority",
            vec![binding],
        )
        .unwrap();
        let original = record.clone();
        // Current Library deliberately has no old Plugin. The explicitly opted
        // in Host owns the invocation's replacement policy, not this baseline.
        let nodes = InvocationAuthorityNodes {
            inner: FakeNodes::default(),
            authority,
        };
        let artifacts = MemoryArtifacts::default();
        let harness = Harness::memory(record);
        harness.run(&artifacts, &nodes).await.unwrap();
        assert_eq!(
            matches!(
                harness.phase(),
                Phase::Settled {
                    result: Outcome::Done
                }
            ),
            authority
        );
        if !authority {
            assert!(matches!(
                harness.phase(),
                Phase::Settled {
                    result: Outcome::Failed { .. }
                }
            ));
        }
        let result = harness.record();
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
    let artifacts = MemoryArtifacts::default();
    let harness = Harness::memory(record);
    harness.run(&artifacts, &nodes).await.unwrap();
    assert!(matches!(
        harness.phase(),
        Phase::Settled {
            result: Outcome::Failed { .. }
        }
    ));
    assert!(
        harness
            .failure_detail()
            .contains("valid admitted Plugin record"),
        "{}",
        harness.failure_detail()
    );
    assert!(nodes.inner.calls.lock().unwrap().is_empty());
}
