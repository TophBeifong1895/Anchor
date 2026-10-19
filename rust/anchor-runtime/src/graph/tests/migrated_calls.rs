#[tokio::test]
async fn graph_call_wait_resumes_same_identity_and_detach_records_acceptance() {
    fn call_snapshot(mode: &str) -> GraphSnapshot {
        let mut snapshot = graph(&["call", "end"], &[("call", "end")], "call");
        snapshot.nodes[0].agent = None;
        snapshot.nodes[0].op = Some("child".into());
        let mut call = serde_json::json!({
            "graph": "child-graph",
            "mode": mode,
            "input": {"x": 1},
            "input_map": {"topic": "/request/topic"},
            "files": [{"node": "produce", "path": "report.md", "as": "input/report.md"}],
            "session": "research"
        });
        if mode == "wait" {
            call["result"] = serde_json::json!({"node": "answer", "files": ["answer.md"]});
        }
        snapshot
            .ops
            .insert("child".into(), serde_json::json!({"call": call}));
        snapshot
    }

    let port = Arc::new(FakeGraphCalls {
        outcomes: Mutex::new(vec![
            GraphCallOutcome::Waiting {
                child_run_id: "child-run-1".into(),
            },
            GraphCallOutcome::Waiting {
                child_run_id: "child-run-1".into(),
            },
            GraphCallOutcome::Completed {
                child_run_id: "child-run-1".into(),
                output: serde_json::json!({"answer": 42}),
            },
        ]),
        ..Default::default()
    });
    let nodes = FakeNodes {
        graph_call_port: Some(port.clone()),
        ..Default::default()
    };
    let artifacts = MemoryArtifacts::default();
    let initial = GraphRunRecord::create(call_snapshot("wait"), Value::Null).unwrap();
    let harness = Harness::memory(initial);
    harness.run(&artifacts, &nodes).await.unwrap();
    assert!(matches!(
        harness.phase(),
        Phase::Waiting {
            waiter: Waiter::Child { .. }
        }
    ));
    assert_eq!(harness.frontier()[0].node.id, "call");
    harness.run(&artifacts, &nodes).await.unwrap();
    let resumed = harness.record();
    assert_eq!(
        harness.phase(),
        Phase::Settled {
            result: Outcome::Done
        }
    );
    {
        let identities = port.identities.lock().unwrap();
        assert_eq!(identities.len(), 3);
        assert!(identities.iter().all(|identity| identity == &identities[0]));
    }
    assert_eq!(
        resumed.graph_calls.values().next().unwrap().status,
        GraphCallStatus::Completed
    );
    assert!(
        nodes
            .calls
            .lock()
            .unwrap()
            .iter()
            .all(|request| request.key.node_id != "call")
    );

    for outcome in [
        GraphCallOutcome::Detached {
            child_run_id: "child-run-2".into(),
        },
        // If the child finishes before the parent observes it, detach still
        // returns only the accepted child reference, never its business result.
        GraphCallOutcome::Completed {
            child_run_id: "child-run-3".into(),
            output: serde_json::json!({"secret_result": "not forwarded"}),
        },
    ] {
        let port = Arc::new(FakeGraphCalls {
            outcomes: Mutex::new(vec![outcome]),
            ..Default::default()
        });
        let nodes = FakeNodes {
            graph_call_port: Some(port),
            ..Default::default()
        };
        let artifacts = MemoryArtifacts::default();
        let harness =
            Harness::memory(GraphRunRecord::create(call_snapshot("detach"), Value::Null).unwrap());
        harness.run(&artifacts, &nodes).await.unwrap();
        let result = harness.record();
        let call_result = result.results.get("call").unwrap().last().unwrap();
        assert_eq!(call_result.completion.output["status"], "accepted");
        assert!(call_result.completion.output.get("secret_result").is_none());
    }
}

#[tokio::test]
async fn graph_call_identity_must_keep_one_child_run_across_reload() {
    let mut snapshot = graph(&["call"], &[], "call");
    snapshot.nodes[0].agent = None;
    snapshot.nodes[0].op = Some("child".into());
    snapshot.ops.insert(
        "child".into(),
        serde_json::json!({"call": {"graph": "child-graph", "mode": "wait"}}),
    );

    let port = Arc::new(FakeGraphCalls {
        outcomes: Mutex::new(vec![
            GraphCallOutcome::Waiting {
                child_run_id: "child-run-1".into(),
            },
            GraphCallOutcome::Waiting {
                child_run_id: "child-run-1".into(),
            },
            // A durable identity must keep resolving to its original child Run.
            // Switching to a different child on reload would silently fork or
            // replay work, so the mismatch must fail closed.
            GraphCallOutcome::Completed {
                child_run_id: "child-run-2".into(),
                output: serde_json::json!({"answer": "unexpected"}),
            },
        ]),
        ..Default::default()
    });
    let nodes = FakeNodes {
        graph_call_port: Some(port.clone()),
        ..Default::default()
    };
    let artifacts = MemoryArtifacts::default();
    let harness = Harness::memory(GraphRunRecord::create(snapshot, Value::Null).unwrap());
    harness.run(&artifacts, &nodes).await.unwrap();
    let waiting = harness.record();
    assert!(matches!(
        harness.phase(),
        Phase::Waiting {
            waiter: Waiter::Child { .. }
        }
    ));
    assert_eq!(
        waiting
            .graph_calls
            .values()
            .next()
            .unwrap()
            .child_run_id
            .as_deref(),
        Some("child-run-1")
    );

    let result = harness.run(&artifacts, &nodes).await;
    assert!(
        matches!(result, Err(GraphError::CorruptRun(message)) if message.contains("changed child Run"))
    );
}
