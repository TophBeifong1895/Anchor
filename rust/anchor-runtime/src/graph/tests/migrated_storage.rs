fn test_root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "anchor-{label}-{}-{}",
        std::process::id(),
        RUN_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ))
}

struct FaultStore<S> {
    inner: S,
    fail_before: Mutex<Option<usize>>,
    fail_after: Mutex<Option<usize>>,
    commits: std::sync::atomic::AtomicUsize,
}
impl<S> FaultStore<S> {
    fn new(inner: S, before: Option<usize>, after: Option<usize>) -> Self {
        Self {
            inner,
            fail_before: Mutex::new(before),
            fail_after: Mutex::new(after),
            commits: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}
impl<S: Store> Store for FaultStore<S> {
    fn load(&self, id: &str) -> Result<Option<Resource>, StoreError> {
        self.inner.load(id)
    }
    fn create(&self, resource: &Resource) -> Result<(), StoreError> {
        self.inner.create(resource)
    }
    fn commit(&self, id: &str, record: &anchor_kernel::Record) -> Result<(), StoreError> {
        let count = self.commits.fetch_add(1, Ordering::SeqCst) + 1;
        if *self.fail_before.lock().unwrap() == Some(count) {
            return Err(StoreError::Io(io::Error::other("injected before commit")));
        }
        self.inner.commit(id, record)?;
        if *self.fail_after.lock().unwrap() == Some(count) {
            return Err(StoreError::Io(io::Error::other(
                "injected after durable commit",
            )));
        }
        Ok(())
    }
    fn history(&self, id: &str) -> Result<Vec<anchor_kernel::Record>, StoreError> {
        self.inner.history(id)
    }
    fn list(&self, prefix: &str) -> Result<Vec<String>, StoreError> {
        self.inner.list(prefix)
    }
}

#[tokio::test]
async fn known_node_failure_is_terminal_and_does_not_advance_edges() {
    let snapshot = graph(
        &["fails", "downstream"],
        &[("fails", "downstream")],
        "fails",
    );
    let artifacts = MemoryArtifacts::default();
    let nodes = FakeNodes {
        fail_once: Mutex::new(Some("provider rejected request".into())),
        ..Default::default()
    };
    let harness = Harness::with_store(
        FaultStore::new(MemoryStore::default(), Some(1), None),
        GraphRunRecord::create(snapshot, Value::Null).unwrap(),
    );
    assert!(harness.run(&artifacts, &nodes).await.is_err());
    assert_eq!(harness.phase(), Phase::Executing);
    assert_eq!(harness.frontier()[0].node.id, "fails");
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
            .contains("provider rejected request")
    );
    assert!(harness.failure_detail().contains("fails"));
    let failed = harness.record();
    assert_eq!(failed.invocations["fails"], 1);
    assert_eq!(failed.passes["fails"], 1);
    assert_eq!(nodes.calls.lock().unwrap().len(), 1);
    assert!(failed.decided.is_empty());
    assert!(!failed.invocations.contains_key("downstream"));
    assert!(!failed.passes.contains_key("downstream"));
    harness.run(&artifacts, &nodes).await.unwrap();
    assert_eq!(harness.record(), failed);
    assert_eq!(nodes.calls.lock().unwrap().len(), 1);
    assert_eq!(*artifacts.freezes.lock().unwrap(), 0);
}

#[tokio::test]
async fn invalid_durable_route_fails_run_without_settling_or_replaying() {
    let artifacts = MemoryArtifacts::default();
    let nodes = FakeNodes {
        invalid_route_once: Mutex::new(true),
        ..Default::default()
    };
    let harness = Harness::with_store(
        FaultStore::new(MemoryStore::default(), Some(1), None),
        GraphRunRecord::create(
            graph(&["start", "next"], &[("start", "next")], "start"),
            Value::Null,
        )
        .unwrap(),
    );
    assert!(harness.run(&artifacts, &nodes).await.is_err());
    assert_eq!(harness.phase(), Phase::Executing);
    harness.run(&artifacts, &nodes).await.unwrap();
    assert!(matches!(
        harness.phase(),
        Phase::Settled {
            result: Outcome::Failed { .. }
        }
    ));
    assert!(harness.failure_detail().contains("unlisted-route"));
    let failed = harness.record();
    assert_eq!(failed.invocations["start"], 1);
    assert_eq!(failed.passes["start"], 1);
    assert!(failed.decided.is_empty());
    assert!(failed.results.is_empty());
    assert_eq!(nodes.calls.lock().unwrap().len(), 1);
    harness.run(&artifacts, &nodes).await.unwrap();
    assert_eq!(harness.record(), failed);
    assert_eq!(nodes.calls.lock().unwrap().len(), 1);
    assert_eq!(*artifacts.freezes.lock().unwrap(), 0);
}

#[tokio::test]
async fn completion_fact_survives_kernel_commit_failure_without_reexecution() {
    let artifacts = MemoryArtifacts::default();
    let nodes = FakeNodes::default();
    let harness = Harness::with_store(
        FaultStore::new(MemoryStore::default(), Some(1), None),
        GraphRunRecord::create(graph(&["one"], &[], "one"), Value::Null).unwrap(),
    );
    assert!(harness.run(&artifacts, &nodes).await.is_err());
    assert_eq!(harness.phase(), Phase::Executing);
    assert_eq!(harness.frontier()[0].invocation, 1);
    harness.run(&artifacts, &nodes).await.unwrap();
    assert_eq!(
        harness.phase(),
        Phase::Settled {
            result: Outcome::Done
        }
    );
    assert_eq!(nodes.calls.lock().unwrap().len(), 1);
    assert_eq!(*artifacts.freezes.lock().unwrap(), 1);
}

#[tokio::test]
async fn durable_kernel_commit_feedback_error_preserves_result_across_reload() {
    let root = test_root("commit-feedback");
    let initial = GraphRunRecord::create(graph(&["one"], &[], "one"), Value::Null).unwrap();
    let run_id = initial.run_id.clone();
    let ports = DurableTestPorts {
        root: root.clone(),
        crash_point: None,
    };
    let harness = Harness::with_store(
        FaultStore::new(FileStore::new(root.join("kernel")).unwrap(), None, Some(1)),
        initial,
    );
    let stale = harness.resource();
    assert!(harness.run(&ports, &ports).await.is_err());
    let committed = harness.resource();
    assert_eq!(committed.version, stale.version + 1);
    assert_eq!(harness.record().results["one"].len(), 1);
    // A caller holding a stale value cannot overwrite the durable result.
    let stale_verdict = harness
        .kernel
        .submit(
            &Principal::new("anchor/operator", PrincipalKind::Human),
            0,
            Submission::new(&harness.id, stale.version, Action::Request)
                .content(serde_json::json!({"control":"stop"})),
        )
        .unwrap();
    assert_eq!(stale_verdict.reason, Reason::TaskSettled);
    assert_eq!(harness.resource(), committed);
    let fresh = Harness::reload(FileStore::new(root.join("kernel")).unwrap(), &run_id);
    fresh.run(&ports, &ports).await.unwrap();
    assert_eq!(
        fresh.phase(),
        Phase::Settled {
            result: Outcome::Done
        }
    );
    assert_eq!(fresh.record().results["one"].len(), 1);
    assert_eq!(fresh.record().invocations["one"], 1);
    assert_eq!(fresh.record().passes["one"], 1);
    assert_eq!(fs::read(root.join("execute-count")).unwrap(), b"1");
    assert_eq!(fs::read(root.join("freeze-count")).unwrap(), b"1");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn legacy_run_formats_are_named_refusals_on_decode_and_kernel_resource_read() {
    let root = test_root("old-formats");
    let record =
        GraphRunRecord::create_with_id(graph(&["work"], &[], "work"), Value::Null, "old-format")
            .unwrap();
    let current = serde_json::to_value(&record).unwrap();
    let mut fixtures = Vec::new();
    for format in 1..RUN_RECORD_FORMAT {
        let mut old = current.clone();
        old["format"] = serde_json::json!(format);
        fixtures.push(old);
    }
    for field in [
        "environments",
        "status",
        "cursor",
        "updated",
        "error",
        "recovery",
        "recovery_submissions",
    ] {
        let mut old = current.clone();
        old[field] = Value::Null;
        fixtures.push(old);
    }
    let mut authorization = current.clone();
    authorization["plugin_bindings"] = serde_json::json!({"old":{"id":"old","digest":"digest"}});
    fixtures.push(authorization);
    let mut deleted = current;
    deleted["graph_calls"] = serde_json::json!({"old":{"identity":{"parent_run_id":record.run_id,"parent_graph_digest":record.graph_digest,"node_id":"work","invocation":1,"call_spec_digest":"spec"},"graph":"child","child_run_id":null,"mode":"wait","status":"deleted"}});
    fixtures.push(deleted);
    let store = FileStore::new(&root).unwrap();
    for value in fixtures {
        let error = serde_json::from_value::<GraphRunRecord>(value.clone())
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("old Graph Run format") || error.contains("old authorization format"),
            "{error}"
        );
        assert!(error.contains("offline migration required"), "{error}");
        let resource = Resource::new(
            validator::run_resource_id(&record.run_id),
            validator::GRAPH_VALIDATOR,
            value,
        )
        .task(100);
        // FileStore stores opaque state; the owning Graph decoder refuses it.
        let id = resource.id.clone();
        store.create(&resource).unwrap();
        let before = store.load(&id).unwrap().unwrap();
        let error = validator::record_of(&before).unwrap_err().to_string();
        assert!(error.contains("offline migration required"), "{error}");
        assert_eq!(store.load(&id).unwrap().unwrap(), before);
        store.remove(&id).unwrap();
    }
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn legacy_cursor_payload_is_named_refusal_without_resource_rewrite() {
    let record = GraphRunRecord::create(graph(&["work"], &[], "work"), Value::Null).unwrap();
    let mut resource = validator::run_resource(&record, 100).unwrap();
    resource.state["cursor"] = serde_json::json!({"node_id":"work","key":{"run_id":record.run_id,"graph_digest":record.graph_digest,"node_id":"work","invocation":1},"input_commits":[],"prepared_input":null});
    let root = test_root("old-cursor");
    let store = FileStore::new(&root).unwrap();
    store.create(&resource).unwrap();
    let harness = Harness::reload(store, &record.run_id);
    let nodes = FakeNodes::default();
    assert!(
        harness
            .run(&MemoryArtifacts::default(), &nodes)
            .await
            .unwrap_err()
            .to_string()
            .contains("offline migration required")
    );
    assert_eq!(harness.resource(), resource);
    assert!(nodes.calls.lock().unwrap().is_empty());
    fs::remove_dir_all(root).unwrap();
}
