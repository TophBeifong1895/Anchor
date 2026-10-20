#[test]
fn only_completed_failed_and_aborted_runs_are_terminal() {
    for (phase, terminal) in [
        (Phase::Executing, false),
        (
            Phase::Waiting {
                waiter: Waiter::Human {
                    reason: "pause".into(),
                },
            },
            false,
        ),
        (
            Phase::Waiting {
                waiter: Waiter::Child {
                    task: "child".into(),
                },
            },
            false,
        ),
        (
            Phase::Settled {
                result: Outcome::Done,
            },
            true,
        ),
        (
            Phase::Settled {
                result: Outcome::Failed {
                    reason: Reason::Unclassified,
                    detail: "failed".into(),
                },
            },
            true,
        ),
        (
            Phase::Settled {
                result: Outcome::Aborted {
                    detail: "aborted".into(),
                },
            },
            true,
        ),
        (
            Phase::Settled {
                result: Outcome::Escalated {
                    detail: "limit".into(),
                },
            },
            true,
        ),
        (
            Phase::Settled {
                result: Outcome::Unknown {
                    detail: "unknown".into(),
                },
            },
            true,
        ),
    ] {
        let mut resource =
            Resource::new("run/test", validator::GRAPH_VALIDATOR, Value::Null).task(10);
        resource.task.as_mut().unwrap().phase = phase;
        assert_eq!(resource.is_settled(), terminal);
    }
}

#[tokio::test]
async fn waiting_recovery_preserves_dispatch_identity_and_scheduler_reentry_cannot_bypass_it() {
    let artifacts = MemoryArtifacts::default();
    let nodes = FakeNodes {
        recovery_once: Mutex::new(true),
        ..Default::default()
    };
    let harness =
        Harness::memory(GraphRunRecord::create(graph(&["one"], &[], "one"), Value::Null).unwrap());
    harness.run(&artifacts, &nodes).await.unwrap();
    assert!(matches!(
        harness.phase(),
        Phase::Waiting {
            waiter: Waiter::Human { .. }
        }
    ));
    let observation = harness.kernel.history(&harness.id).unwrap().pop().unwrap();
    let key: InvocationKey =
        serde_json::from_value(observation.submission.content["invocation"].clone()).unwrap();
    assert_eq!(key.invocation, 1);
    assert_eq!(
        observation.submission.content["attempts"][0]["attempt_id"],
        17
    );
    assert_eq!(harness.pending_input(&artifacts, "one").await.key, key);
    assert_eq!(nodes.calls.lock().unwrap().len(), 1);
    let first = harness.resource();
    let again = harness.run(&artifacts, &nodes).await.unwrap();
    assert_eq!(again, first);
    assert_eq!(nodes.calls.lock().unwrap().len(), 1);
}

#[test]
fn human_stop_retains_unknown_recovery_observation_in_kernel_history() {
    let record = GraphRunRecord::create(graph(&["work"], &[], "work"), Value::Null).unwrap();
    let key = InvocationKey {
        run_id: record.run_id.clone(),
        graph_digest: record.graph_digest.clone(),
        node_id: "work".into(),
        invocation: 1,
    };
    let attempts = serde_json::json!([{
        "attempt_id":77,"step":2,"tool":"external_effect","started_at":"2026-10-05T00:00:00Z"
    }]);
    let harness = Harness::memory(record.clone());
    let (principal, cost, submission) = scheduler::observation_stop(
        &harness.resource(),
        &key,
        "unknown external tool effects".into(),
        attempts.clone(),
    );
    assert!(
        harness
            .kernel
            .submit(&principal, cost, submission)
            .unwrap()
            .is_accepted()
    );
    assert!(harness.request("stop").is_accepted());
    let history = harness.kernel.history(&harness.id).unwrap();
    assert_eq!(
        history[0].submission.content["invocation"],
        serde_json::to_value(key).unwrap()
    );
    assert_eq!(history[0].submission.content["attempts"], attempts);
    assert_eq!(harness.record(), record);
    assert!(matches!(
        harness.phase(),
        Phase::Waiting {
            waiter: Waiter::Human { .. }
        }
    ));
}

#[tokio::test]
async fn budget_stop_preserves_dispatch_identity_and_resume_uses_same_invocation() {
    let mut g = graph(&["one"], &[], "one");
    g.agents.get_mut("worker").unwrap().max_steps = Some(0);
    let artifacts = MemoryArtifacts::default();
    let nodes = FakeNodes {
        budget_once: Mutex::new(true),
        ..Default::default()
    };
    let harness = Harness::memory(GraphRunRecord::create(g, Value::Null).unwrap());
    harness.run(&artifacts, &nodes).await.unwrap();
    assert_eq!(
        harness.phase(),
        Phase::Waiting {
            waiter: Waiter::Human {
                reason: "budget".into()
            }
        }
    );
    let pending = harness.pending_input(&artifacts, "one").await;
    assert_eq!(pending.key.invocation, 1);
    // S6: preparing a slot is pure. Only a judged completion counts a pass.
    assert!(harness.record().invocations.is_empty());
    assert!(harness.record().passes.is_empty());
    assert_eq!(
        nodes.facts.lock().unwrap().get(&pending.key.durable_key()),
        None
    );
    assert_eq!(*artifacts.freezes.lock().unwrap(), 0);
    assert_eq!(harness.resource().task.unwrap().budget.spent, 2);
    assert!(harness.request("resume").is_accepted());
    harness.run(&artifacts, &nodes).await.unwrap();
    assert_eq!(
        harness.phase(),
        Phase::Settled {
            result: Outcome::Done
        }
    );
    let calls = nodes.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].key, calls[1].key);
    assert_eq!(calls[0].key, pending.key);
    assert_eq!(harness.record().invocations["one"], 1);
    assert_eq!(harness.record().passes["one"], 1);
}

#[tokio::test]
async fn kernel_scheduler_dispatches_durable_resumable_node_with_same_invocation() {
    let nodes = FakeNodes::default();
    let artifacts = MemoryArtifacts::default();
    let record = GraphRunRecord::create(graph(&["one"], &[], "one"), Value::Null).unwrap();
    let key = InvocationKey {
        run_id: record.run_id.clone(),
        graph_digest: record.graph_digest.clone(),
        node_id: "one".into(),
        invocation: 1,
    };
    nodes
        .facts
        .lock()
        .unwrap()
        .insert(key.durable_key(), CompletionFact::Resumable);
    let harness = Harness::memory(record);
    harness.run(&artifacts, &nodes).await.unwrap();
    assert_eq!(
        harness.phase(),
        Phase::Settled {
            result: Outcome::Done
        }
    );
    assert_eq!(nodes.calls.lock().unwrap().len(), 1);
    assert_eq!(nodes.calls.lock().unwrap()[0].key, key);
    assert_eq!(harness.record().invocations["one"], 1);
}

#[tokio::test]
async fn uncertain_and_pause_fail_closed_at_invocation_boundary() {
    let artifacts = MemoryArtifacts::default();
    let nodes = FakeNodes {
        uncertain: Mutex::new(true),
        ..Default::default()
    };
    let harness =
        Harness::memory(GraphRunRecord::create(graph(&["one"], &[], "one"), Value::Null).unwrap());
    harness.run(&artifacts, &nodes).await.unwrap();
    assert!(matches!(
        harness.phase(),
        Phase::Waiting {
            waiter: Waiter::Human { .. }
        }
    ));
    assert!(nodes.calls.lock().unwrap().is_empty());
    assert_eq!(
        harness
            .pending_input(&artifacts, "one")
            .await
            .key
            .invocation,
        1
    );
    assert!(harness.record().invocations.is_empty());
    assert!(harness.record().passes.is_empty());
    assert!(harness.record().decided.is_empty());
    let paused =
        Harness::memory(GraphRunRecord::create(graph(&["one"], &[], "one"), Value::Null).unwrap());
    assert!(paused.request("stop").is_accepted());
    paused.run(&artifacts, &nodes).await.unwrap();
    assert!(matches!(
        paused.phase(),
        Phase::Waiting {
            waiter: Waiter::Human { .. }
        }
    ));
    assert!(paused.record().passes.is_empty());
    assert!(nodes.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn stop_before_dispatch_and_executor_cancel_preserve_pending_dispatch() {
    let artifacts = MemoryArtifacts::default();
    let nodes = FakeNodes::default();
    let harness =
        Harness::memory(GraphRunRecord::create(graph(&["one"], &[], "one"), Value::Null).unwrap());
    assert!(harness.request("stop").is_accepted());
    harness.run(&artifacts, &nodes).await.unwrap();
    assert!(matches!(harness.phase(), Phase::Waiting { .. }));
    assert!(harness.record().invocations.is_empty());
    assert!(nodes.calls.lock().unwrap().is_empty());
    assert!(harness.request("resume").is_accepted());
    *nodes.cancel_once.lock().unwrap() = true;
    harness.run(&artifacts, &nodes).await.unwrap();
    // Cancel only drains this worker; a persisted Human stop is a separate fact.
    assert_eq!(harness.phase(), Phase::Executing);
    let pending = harness.pending_input(&artifacts, "one").await;
    assert_eq!(pending.key.invocation, 1);
    assert_eq!(
        nodes.facts.lock().unwrap().get(&pending.key.durable_key()),
        None
    );
    harness.run(&artifacts, &nodes).await.unwrap();
    assert_eq!(
        harness.phase(),
        Phase::Settled {
            result: Outcome::Done
        }
    );
    let calls = nodes.calls.lock().unwrap();
    assert_eq!(calls[0].key, calls[1].key);
}

#[tokio::test]
async fn legacy_cursor_payload_is_refused_while_frontier_identity_remains_authoritative() {
    let record = GraphRunRecord::create(graph(&["one"], &[], "one"), Value::Null).unwrap();
    let harness = Harness::memory(record);
    let mut resource = harness.resource();
    resource.state["cursor"] = serde_json::json!({"node_id":"foreign","key":{"run_id":"other","node_id":"foreign","graph_digest":"bad","invocation":1}});
    let error = validator::record_of(&resource).unwrap_err().to_string();
    assert!(error.contains("old Graph Run format"), "{error}");
    // Dispatch identities now come exclusively from the validator's frontier.
    assert_eq!(harness.frontier()[0].node.id, "one");
    assert_eq!(harness.frontier()[0].invocation, 1);
}

#[tokio::test]
async fn reentering_a_terminal_run_does_not_advance_kernel_version_or_history() {
    let artifacts = MemoryArtifacts::default();
    let nodes = FakeNodes::default();
    let root = test_root("reentry");
    let harness = Harness::with_store(
        FileStore::new(&root).unwrap(),
        GraphRunRecord::create(graph(&["one"], &[], "one"), Value::Null).unwrap(),
    );
    harness.run(&artifacts, &nodes).await.unwrap();
    let record = harness.resource();
    let history = harness.kernel.history(&harness.id).unwrap();
    assert_eq!(harness.run(&artifacts, &nodes).await.unwrap(), record);
    assert_eq!(harness.resource(), record);
    assert_eq!(harness.kernel.history(&harness.id).unwrap(), history);
    assert_eq!(nodes.calls.lock().unwrap().len(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn kernel_history_commit_preserves_graph_admission_across_file_reload() {
    let root = test_root("commit-observation");
    let record = GraphRunRecord::create(graph(&["one"], &[], "one"), Value::Null).unwrap();
    let harness = Harness::with_store(FileStore::new(&root).unwrap(), record.clone());
    let version = harness.resource().version;
    assert!(harness.request("stop").is_accepted());
    assert!(harness.resource().version > version);
    let loaded = Harness::reload(FileStore::new(&root).unwrap(), &record.run_id);
    assert_eq!(loaded.resource(), harness.resource());
    assert_eq!(
        loaded.kernel.history(&loaded.id).unwrap(),
        harness.kernel.history(&harness.id).unwrap()
    );
    assert_eq!(loaded.record(), record);
    assert_eq!(loaded.record().started, record.started);
    assert_eq!(loaded.record().format, RUN_RECORD_FORMAT);
    fs::remove_dir_all(root).unwrap();
}
