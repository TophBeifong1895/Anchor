#[tokio::test]
async fn plugin_authorization_drift_fails_before_dispatch() {
    let mut snapshot = graph(&["work"], &[], "work");
    snapshot.nodes[0].plugins = vec!["review".into()];
    let binding = PluginBinding {
        id: "review".into(),
        digest: "manifest-v1".into(),
        resources: vec!["instructions.md".into()],
        mcp_servers: vec!["search".into()],
        authorization_digest: "authorization-v1"
            .try_into()
            .expect("non-empty fixture authority"),
    };
    let nodes = FakeNodes {
        resolved_plugins: Mutex::new(vec![binding.clone()]),
        budget_once: Mutex::new(true),
        ..Default::default()
    };
    let artifacts = MemoryArtifacts::default();
    let harness = Harness::memory(admitted(snapshot, &nodes));
    harness.run(&artifacts, &nodes).await.unwrap();
    let first = harness.record();
    assert_eq!(
        harness.phase(),
        Phase::Waiting {
            waiter: Waiter::Human {
                reason: "budget".into()
            }
        }
    );
    assert_eq!(first.plugin_bindings.get("review"), Some(&binding));
    assert!(first.plugin_bindings_initialized);
    assert_eq!(nodes.calls.lock().unwrap()[0].plugins, vec![binding]);

    {
        let mut changed = nodes.resolved_plugins.lock().unwrap();
        changed[0].digest = "manifest-v2".into();
        changed[0].authorization_digest = "authorization-v2"
            .try_into()
            .expect("non-empty fixture authority");
    }
    assert!(harness.request("resume").is_accepted());
    harness.run(&artifacts, &nodes).await.unwrap();
    assert!(matches!(
        harness.phase(),
        Phase::Settled {
            result: Outcome::Failed { .. }
        }
    ));
    assert!(harness.failure_detail().contains("authorization"));
    assert_eq!(nodes.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn plugin_code_changes_resume_and_are_observed_per_call() {
    let mut snapshot = graph(&["work"], &[], "work");
    snapshot.nodes[0].plugins = vec!["review".into()];
    let binding = PluginBinding {
        id: "review".into(),
        digest: "code-v1".into(),
        resources: vec!["instructions.md".into()],
        mcp_servers: vec!["search".into()],
        authorization_digest: "authorization-v1"
            .try_into()
            .expect("non-empty fixture authority"),
    };
    let nodes = FakeNodes {
        resolved_plugins: Mutex::new(vec![binding.clone()]),
        budget_once: Mutex::new(true),
        ..Default::default()
    };
    let artifacts = MemoryArtifacts::default();
    let harness = Harness::memory(admitted(snapshot, &nodes));
    harness.run(&artifacts, &nodes).await.unwrap();
    assert_eq!(
        harness.phase(),
        Phase::Waiting {
            waiter: Waiter::Human {
                reason: "budget".into()
            }
        }
    );

    let mut current = binding;
    current.digest = "code-v2".into();
    current.resources.push("skills/new/SKILL.md".into());
    *nodes.resolved_plugins.lock().unwrap() = vec![current.clone()];
    let dispatch_input = harness.pending_input(&artifacts, "work").await;
    nodes.observed_views.lock().unwrap().insert(
        dispatch_input.key.durable_key(),
        BTreeMap::from([("review".to_owned(), "view-v2".to_owned())]),
    );
    nodes.observed_digests.lock().unwrap().insert(
        dispatch_input.key.durable_key(),
        BTreeMap::from([("review".to_owned(), "code-v2".to_owned())]),
    );
    assert!(harness.request("resume").is_accepted());
    harness.run(&artifacts, &nodes).await.unwrap();
    let resumed = harness.record();
    assert_eq!(
        harness.phase(),
        Phase::Settled {
            result: Outcome::Done
        }
    );
    assert_eq!(nodes.calls.lock().unwrap()[1].plugins, vec![current]);
    assert_eq!(resumed.plugin_bindings["review"].digest, "code-v1");
    assert_eq!(resumed.results["work"][0].plugin_views["review"], "view-v2");
    assert_eq!(
        resumed.results["work"][0].plugin_digests["review"],
        "code-v2"
    );
}

#[tokio::test]
async fn parallel_dispatch_uses_current_plugin_resources_and_preserves_admitted_authority() {
    let mut snapshot = parallel_graph();
    for node in &mut snapshot.nodes {
        if node.agent.is_some() {
            node.plugins = vec!["review".into()];
        }
    }
    let admitted = PluginBinding {
        id: "review".into(),
        digest: "code-v1".into(),
        resources: vec!["old.md".into()],
        mcp_servers: vec![],
        authorization_digest: "authority-v1"
            .try_into()
            .expect("non-empty fixture authority"),
    };
    let mut current = admitted.clone();
    current.digest = "code-v2".into();
    current.resources = vec!["skills/current/SKILL.md".into()];
    let mut record = GraphRunRecord::create(snapshot, Value::Null).unwrap();
    record
        .plugin_bindings
        .insert(admitted.id.clone(), admitted.clone());
    record.plugin_bindings_initialized = true;
    let nodes = FakeNodes {
        resolved_plugins: Mutex::new(vec![current.clone()]),
        op_run: true,
        ..Default::default()
    };
    for node in ["left", "right", "after"] {
        let key = InvocationKey {
            run_id: record.run_id.clone(),
            graph_digest: record.graph_digest.clone(),
            node_id: node.into(),
            invocation: 1,
        };
        nodes.observed_digests.lock().unwrap().insert(
            key.durable_key(),
            BTreeMap::from([("review".into(), current.digest.clone())]),
        );
    }
    let artifacts = MemoryArtifacts::default();
    let harness = Harness::memory(record);
    harness.run(&artifacts, &nodes).await.unwrap();
    let result = harness.record();
    assert_eq!(
        harness.phase(),
        Phase::Settled {
            result: Outcome::Done
        }
    );
    assert_eq!(result.plugin_bindings["review"], admitted);
    for call in nodes.calls.lock().unwrap().iter() {
        assert_eq!(call.plugins, vec![current.clone()]);
    }
    for node in ["left", "right", "after"] {
        assert_eq!(result.results[node][0].plugin_digests["review"], "code-v2");
    }
}

#[tokio::test]
async fn a_node_call_records_the_dependency_view_it_used() {
    let snapshot = graph(&["work"], &[], "work");
    let record = GraphRunRecord::create(snapshot, Value::Null).unwrap();
    let key = InvocationKey {
        run_id: record.run_id.clone(),
        graph_digest: record.graph_digest.clone(),
        node_id: "work".into(),
        invocation: 1,
    };
    let views = BTreeMap::from([("review".to_owned(), "/library/views/review/abc".to_owned())]);
    let nodes = FakeNodes {
        observed_views: Mutex::new(BTreeMap::from([(key.durable_key(), views.clone())])),
        ..Default::default()
    };
    let artifacts = MemoryArtifacts::default();
    let harness = Harness::memory(record);
    harness.run(&artifacts, &nodes).await.unwrap();
    let finished = harness.record();
    assert_eq!(
        harness.phase(),
        Phase::Settled {
            result: Outcome::Done
        }
    );
    assert_eq!(
        finished.results["work"][0].plugin_views, views,
        "the observation is part of the settled result"
    );
    assert!(
        nodes.observed_views.lock().unwrap().is_empty(),
        "reading the observation consumes it"
    );
}

#[tokio::test]
async fn plugin_run_without_admitted_binding_fails_before_dispatch() {
    let mut snapshot = graph(&["work"], &[], "work");
    snapshot.nodes[0].plugins = vec!["review".into()];
    let nodes = FakeNodes {
        resolved_plugins: Mutex::new(vec![PluginBinding {
            id: "review".into(),
            digest: "manifest-v1".into(),
            resources: vec![],
            mcp_servers: vec![],
            authorization_digest: "authorization-v1"
                .try_into()
                .expect("non-empty fixture authority"),
        }]),
        ..Default::default()
    };
    let artifacts = MemoryArtifacts::default();
    let unbound = GraphRunRecord::create(snapshot, Value::Null).unwrap();
    let harness = Harness::memory(unbound);
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
            .contains("valid admitted Plugin record")
    );
    assert!(nodes.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn legacy_plugin_authority_requires_offline_migration_before_execution() {
    let mut snapshot = graph(&["work"], &[], "work");
    snapshot.nodes[0].plugins = vec!["review".into()];
    let binding = PluginBinding {
        id: "review".into(),
        digest: "code".into(),
        resources: vec![],
        mcp_servers: vec![],
        authorization_digest: "authority".try_into().unwrap(),
    };
    let record = GraphRunRecord::create_with_plugins(
        snapshot,
        Value::Null,
        "legacy-authority",
        vec![binding],
    )
    .unwrap();
    for authority in [None, Some(Value::Null), Some(serde_json::json!(""))] {
        let mut value = serde_json::to_value(&record).unwrap();
        let binding = value["plugin_bindings"]["review"].as_object_mut().unwrap();
        match authority {
            Some(value) => {
                binding.insert("authorization_digest".into(), value);
            }
            None => {
                binding.remove("authorization_digest");
            }
        }
        let error = serde_json::from_value::<GraphRunRecord>(value.clone())
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("old authorization format")
                && error.contains("offline migration required"),
            "{error}"
        );
        let root = test_root("legacy-plugin");
        let resource = Resource::new(
            validator::run_resource_id(&record.run_id),
            validator::GRAPH_VALIDATOR,
            value,
        )
        .task(100);
        let store = FileStore::new(&root).unwrap();
        store.create(&resource).unwrap();
        let harness = Harness::reload(store, &record.run_id);
        let nodes = FakeNodes::default();
        let error = harness
            .run(&MemoryArtifacts::default(), &nodes)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("old authorization format")
                && error.contains("offline migration required"),
            "{error}"
        );
        assert!(nodes.calls.lock().unwrap().is_empty());
        assert_eq!(harness.resource(), resource);
        fs::remove_dir_all(root).unwrap();
    }
}

#[tokio::test]
async fn a_new_run_does_not_bind_plugins_without_a_current_authorization_baseline() {
    let mut snapshot = graph(&["work"], &[], "work");
    snapshot.nodes[0].plugins = vec!["review".into()];
    let nodes = FakeNodes {
        resolved_plugins: Mutex::new(vec![PluginBinding {
            id: "review".into(),
            digest: String::new(),
            resources: vec![],
            mcp_servers: vec![],
            authorization_digest: "current-authority".try_into().unwrap(),
        }]),
        ..Default::default()
    };
    let bindings = nodes.resolve_plugins(&["review".into()]).unwrap();
    assert!(
        GraphRunRecord::create_with_plugins(
            snapshot.clone(),
            Value::Null,
            "invalid-admission",
            bindings
        )
        .is_err()
    );
    let record = GraphRunRecord::create(snapshot, Value::Null).unwrap();
    let harness = Harness::memory(record);
    harness
        .run(&MemoryArtifacts::default(), &nodes)
        .await
        .unwrap();
    assert!(matches!(
        harness.phase(),
        Phase::Settled {
            result: Outcome::Failed { .. }
        }
    ));
    assert!(!harness.record().plugin_bindings_initialized);
    assert!(harness.record().plugin_bindings.is_empty());
    assert!(nodes.calls.lock().unwrap().is_empty());
    assert!(AuthorizationDigest::try_from(String::new()).is_err());
    let digest = AuthorizationDigest::try_from("current-authority").unwrap();
    assert_eq!(
        serde_json::to_value(&digest).unwrap(),
        serde_json::json!("current-authority")
    );
    assert_eq!(
        serde_json::from_value::<AuthorizationDigest>(serde_json::json!("current-authority"))
            .unwrap(),
        digest
    );
    assert!(serde_json::from_value::<AuthorizationDigest>(serde_json::json!("")).is_err());
}

#[tokio::test]
async fn admission_accepts_paired_parallel_topology_and_kernel_scheduler_executes_control_ops() {
    let mut plugin_snapshot = graph(&["one"], &[], "one");
    plugin_snapshot.nodes[0].plugins = vec!["p".into()];
    let nodes = FakeNodes::default();
    let harness = Harness::memory(GraphRunRecord::create(plugin_snapshot, Value::Null).unwrap());
    harness
        .run(&MemoryArtifacts::default(), &nodes)
        .await
        .unwrap();
    assert!(matches!(
        harness.phase(),
        Phase::Settled {
            result: Outcome::Failed { .. }
        }
    ));
    assert!(nodes.calls.lock().unwrap().is_empty());
    let snapshot = parallel_graph();
    let snapshot = GraphSnapshot::admit(serde_json::to_value(snapshot).unwrap()).unwrap();
    assert_eq!(
        snapshot.parallel_regions().unwrap()["start"].branches,
        vec![vec!["left".to_string()], vec!["right".to_string()]]
    );
    let nodes = FakeNodes::default();
    let harness = Harness::memory(GraphRunRecord::create(snapshot, Value::Null).unwrap());
    harness
        .run(&MemoryArtifacts::default(), &nodes)
        .await
        .unwrap();
    assert_eq!(
        harness.phase(),
        Phase::Settled {
            result: Outcome::Done
        }
    );
    assert!(
        nodes
            .calls
            .lock()
            .unwrap()
            .iter()
            .all(|request| request.key.node_id != "start" && request.key.node_id != "collect")
    );
    let mut snapshot = graph(&["one"], &[], "one");
    snapshot.agents.get_mut("worker").unwrap().max_steps = Some(2);
    let harness = Harness::memory(GraphRunRecord::create(snapshot, Value::Null).unwrap());
    let nodes = FakeNodes {
        caps_budget_off: true,
        ..Default::default()
    };
    assert!(matches!(
        harness.run(&MemoryArtifacts::default(), &nodes).await,
        Err(GraphError::Unsupported(_))
    ));
    assert!(nodes.calls.lock().unwrap().is_empty());
}
