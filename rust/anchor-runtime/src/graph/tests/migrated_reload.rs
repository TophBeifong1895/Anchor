#[tokio::test]
async fn serial_kernel_file_store_reload_resumes_same_invocation_after_budget_stop() {
    let root = test_root("serial-provider-retry");
    let artifacts = MemoryArtifacts::default();
    let nodes = FakeNodes {
        budget_once: Mutex::new(true),
        ..Default::default()
    };
    let initial = GraphRunRecord::create(graph(&["one"], &[], "one"), Value::Null).unwrap();
    let run_id = initial.run_id.clone();
    let harness = Harness::with_store(FileStore::new(&root).unwrap(), initial);
    harness.run(&artifacts, &nodes).await.unwrap();
    assert_eq!(
        harness.phase(),
        Phase::Waiting {
            waiter: Waiter::Human {
                reason: "budget".into()
            }
        }
    );
    let original_key = harness.pending_input(&artifacts, "one").await.key;
    let loaded = Harness::reload(FileStore::new(&root).unwrap(), &run_id);
    assert_eq!(
        loaded.pending_input(&artifacts, "one").await.key,
        original_key
    );
    assert!(loaded.request("resume").is_accepted());
    loaded.run(&artifacts, &nodes).await.unwrap();
    assert_eq!(
        loaded.phase(),
        Phase::Settled {
            result: Outcome::Done
        }
    );
    let resumed = loaded.record();
    assert_eq!(resumed.invocations["one"], 1);
    assert_eq!(resumed.passes["one"], 1);
    let calls = nodes.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].key, original_key);
    assert_eq!(calls[1].key, original_key);
    assert_eq!(resumed.results["one"].len(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn serial_interrupted_file_store_reload_resumes_same_invocation_and_settles_done() {
    let root = test_root("serial-interrupted");
    let artifacts = MemoryArtifacts::default();
    let reason = "provider stream interrupted before final_result";
    let nodes = FakeNodes {
        interrupt_once: Mutex::new(BTreeMap::from([("work".into(), reason.into())])),
        ..Default::default()
    };
    let initial = GraphRunRecord::create(
        graph(
            &["before", "work", "after"],
            &[("before", "work"), ("work", "after")],
            "before",
        ),
        serde_json::json!({"request":"continue interrupted work"}),
    )
    .unwrap();
    let run_id = initial.run_id.clone();
    let harness = Harness::with_store(FileStore::new(&root).unwrap(), initial);
    harness.run(&artifacts, &nodes).await.unwrap();
    assert_eq!(
        harness.phase(),
        Phase::Waiting {
            waiter: Waiter::Human {
                reason: reason.into()
            }
        }
    );
    let stopped = harness.record();
    assert!(stopped.parallel.is_none());
    let dispatch_input = harness.pending_input(&artifacts, "work").await;
    let request = nodes.calls.lock().unwrap()[1].clone();
    assert_eq!(dispatch_input.node_id, "work");
    assert_eq!(dispatch_input.key, request.key);
    assert_eq!(dispatch_input.key.invocation, 1);
    assert_eq!(dispatch_input.input_commits, request.input_commits);
    assert_eq!(
        dispatch_input.input_commits,
        vec![stopped.results["before"][0].commit.clone()]
    );
    assert_eq!(dispatch_input.prepared_input, request.input);
    assert!(!stopped.invocations.contains_key("work"));
    assert!(!stopped.passes.contains_key("work"));
    assert!(!stopped.results.contains_key("work"));
    assert!(!stopped.invocations.contains_key("after"));
    assert_eq!(
        nodes
            .facts
            .lock()
            .unwrap()
            .get(&dispatch_input.key.durable_key()),
        Some(&CompletionFact::Resumable)
    );
    assert_eq!(*artifacts.freezes.lock().unwrap(), 1);
    let loaded = Harness::reload(FileStore::new(&root).unwrap(), &run_id);
    assert_eq!(loaded.record(), stopped);
    loaded.record().validate().unwrap();
    assert!(loaded.request("resume").is_accepted());
    loaded.run(&artifacts, &nodes).await.unwrap();
    assert_eq!(
        loaded.phase(),
        Phase::Settled {
            result: Outcome::Done
        }
    );
    let resumed = loaded.record();
    assert!(loaded.frontier().is_empty());
    assert_eq!(resumed.results["before"], stopped.results["before"]);
    assert_eq!(resumed.results["work"].len(), 1);
    assert_eq!(resumed.results["work"][0].key, dispatch_input.key);
    assert_eq!(resumed.results["after"].len(), 1);
    for node in ["before", "work", "after"] {
        assert_eq!(resumed.invocations[node], 1);
        assert_eq!(resumed.passes[node], 1);
    }
    let calls = nodes.calls.lock().unwrap();
    assert_eq!(calls.len(), 4);
    assert_eq!(calls[2].key, dispatch_input.key);
    assert_eq!(calls[2].input_commits, dispatch_input.input_commits);
    assert_eq!(calls[2].input, dispatch_input.prepared_input);
    assert_eq!(calls[3].key.node_id, "after");
    assert_eq!(*artifacts.freezes.lock().unwrap(), 3);
    assert_eq!(
        loaded.phase(),
        Phase::Settled {
            result: Outcome::Done
        }
    );
    fs::remove_dir_all(root).unwrap();
}

async fn exercise_parallel_resume(mode: &str, file_backed: bool) {
    let root = test_root("parallel-reload");
    let artifacts = MemoryArtifacts::default();
    let reason = "left branch provider stream interrupted before final_result";
    let nodes = FakeNodes {
        op_run: true,
        budget_on_node: Some("left".into()),
        ..Default::default()
    };
    match mode {
        "budget" => {
            *nodes.budget_once.lock().unwrap() = true;
            nodes.delays_ms.lock().unwrap().insert("left".into(), 30);
        }
        "cancel" => {
            *nodes.cancel_once.lock().unwrap() = true;
            nodes.delays_ms.lock().unwrap().insert("left".into(), 30);
        }
        "interrupted" => {
            nodes
                .interrupt_once
                .lock()
                .unwrap()
                .insert("left".into(), reason.into());
            nodes.delays_ms.lock().unwrap().insert("left".into(), 30);
        }
        _ => panic!("unknown fixture mode"),
    }
    let initial = GraphRunRecord::create(parallel_graph(), Value::Null).unwrap();
    let run_id = initial.run_id.clone();
    // Both paths exercise the actual Kernel Store; the on-disk path reopens it.
    if file_backed {
        let harness = Harness::with_store(FileStore::new(&root).unwrap(), initial);
        harness.run(&artifacts, &nodes).await.unwrap();
        let resource = harness.resource();
        let loaded = Harness::reload(FileStore::new(&root).unwrap(), &run_id);
        assert_eq!(loaded.resource(), resource);
        finish_parallel_resume(&loaded, &artifacts, &nodes, mode, reason).await;
        fs::remove_dir_all(root).unwrap();
    } else {
        let harness = Harness::memory(initial);
        harness.run(&artifacts, &nodes).await.unwrap();
        finish_parallel_resume(&harness, &artifacts, &nodes, mode, reason).await;
    }
}

async fn finish_parallel_resume<S: Store>(
    harness: &Harness<S>,
    artifacts: &MemoryArtifacts,
    nodes: &FakeNodes,
    mode: &str,
    reason: &str,
) {
    if mode == "cancel" {
        assert_eq!(harness.phase(), Phase::Executing);
    } else {
        assert_eq!(
            harness.phase(),
            Phase::Waiting {
                waiter: Waiter::Human {
                    reason: if mode == "budget" {
                        "budget".into()
                    } else {
                        reason.into()
                    }
                }
            }
        );
    }
    let stopped = harness.record();
    let activation = stopped.parallel.as_ref().unwrap();
    assert_eq!(activation.branches.len(), 2);
    let completed = activation
        .branches
        .iter()
        .filter(|branch| branch.status == ParallelBranchStatus::Completed)
        .map(|branch| branch.entry.clone())
        .collect::<BTreeSet<_>>();
    assert_eq!(completed.len(), 1, "one completed sibling remains durable");
    let pending = activation
        .branches
        .iter()
        .find(|branch| branch.status != ParallelBranchStatus::Completed)
        .unwrap();
    assert_eq!(pending.next_index, 0);
    assert!(pending.completed.is_empty());
    let dispatch_input = harness.pending_input(artifacts, &pending.entry).await;
    let calls_before = nodes.calls.lock().unwrap().clone();
    let request = calls_before
        .iter()
        .find(|request| request.key.node_id == pending.entry)
        .unwrap();
    assert_eq!(dispatch_input.key, request.key);
    assert_eq!(dispatch_input.key.invocation, 1);
    assert_eq!(dispatch_input.input_commits, request.input_commits);
    assert_eq!(
        dispatch_input.input_commits,
        vec![stopped.results["start"][0].commit.clone()]
    );
    assert_eq!(dispatch_input.prepared_input, request.input);
    assert!(!stopped.invocations.contains_key(&pending.entry));
    assert!(!stopped.passes.contains_key(&pending.entry));
    assert_eq!(*artifacts.freezes.lock().unwrap(), 2);
    stopped.validate().unwrap();
    if mode != "cancel" {
        assert!(harness.request("resume").is_accepted());
    }
    harness.run(artifacts, nodes).await.unwrap();
    assert_eq!(
        harness.phase(),
        Phase::Settled {
            result: Outcome::Done
        }
    );
    let resumed = harness.record();
    assert!(resumed.parallel.is_none());
    assert_eq!(resumed.results["start"], stopped.results["start"]);
    for node in &completed {
        assert_eq!(resumed.results[node], stopped.results[node]);
    }
    for node in ["start", "left", "right", "collect", "after"] {
        assert_eq!(resumed.results[node].len(), 1);
        assert_eq!(resumed.invocations[node], 1);
        assert_eq!(resumed.passes[node], 1);
    }
    let calls = nodes.calls.lock().unwrap();
    let pending_calls = calls
        .iter()
        .filter(|request| request.key == dispatch_input.key)
        .collect::<Vec<_>>();
    assert_eq!(pending_calls.len(), 2);
    for request in pending_calls {
        assert_eq!(request.input_commits, dispatch_input.input_commits);
        assert_eq!(request.input, dispatch_input.prepared_input);
    }
    for node in completed {
        assert_eq!(
            calls
                .iter()
                .filter(|request| request.key.node_id == node)
                .count(),
            1
        );
    }
    assert_eq!(
        calls
            .iter()
            .filter(|request| request.key.node_id == "after")
            .count(),
        1
    );
    assert!(calls.iter().all(|request| request.key.node_id != "collect"));
    assert_eq!(*artifacts.freezes.lock().unwrap(), 5);
}

#[tokio::test]
async fn parallel_resume_keeps_completed_branch_and_reuses_budget_dispatch() {
    exercise_parallel_resume("budget", false).await;
}
#[tokio::test]
async fn parallel_kernel_file_store_roundtrip_preserves_activation_and_branch_dispatch() {
    exercise_parallel_resume("budget", true).await;
}
#[tokio::test]
async fn parallel_cancelled_branch_can_reload_without_replaying_completed_sibling() {
    exercise_parallel_resume("cancel", true).await;
}
#[tokio::test]
async fn parallel_interrupted_file_store_reload_preserves_completed_branch_and_settles_done() {
    exercise_parallel_resume("interrupted", true).await;
}
