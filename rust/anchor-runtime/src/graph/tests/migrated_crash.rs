#[tokio::test]
async fn file_run_store_recovers_completed_node_and_artifact_facts_after_process_crash() {
    let executable = std::env::current_exe().unwrap();
    for (crash_name, expected_exit) in [("after-node-fact", 71), ("after-artifact-commit", 72)] {
        let root = std::env::temp_dir().join(format!(
            "anchor-graph-recovery-{}-{}-{crash_name}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();

        let initial = GraphRunRecord::create(graph(&["one"], &[], "one"), Value::Null).unwrap();
        let run_id = initial.run_id.clone();
        write_durable_json(&root.join("initial.json"), &initial).unwrap();

        let crashed = Command::new(&executable)
            .args([
                "--exact",
                "graph::tests::graph_runner_recovery_process_helper",
                "--nocapture",
            ])
            .env("ANCHOR_GRAPH_RECOVERY_TEST_ROOT", &root)
            .env("ANCHOR_GRAPH_RECOVERY_TEST_RUN", &run_id)
            .env("ANCHOR_GRAPH_RECOVERY_TEST_CRASH", crash_name)
            .output()
            .unwrap();
        assert_eq!(
            crashed.status.code(),
            Some(expected_exit),
            "child did not crash at {crash_name}; stdout={}, stderr={}",
            String::from_utf8_lossy(&crashed.stdout),
            String::from_utf8_lossy(&crashed.stderr)
        );

        let harness = Harness::reload(FileStore::new(root.join("kernel")).unwrap(), &run_id);
        assert_eq!(harness.phase(), Phase::Executing);
        let cursor = harness
            .pending_input(
                &DurableTestPorts {
                    root: root.clone(),
                    crash_point: None,
                },
                "one",
            )
            .await;
        assert_eq!(cursor.node_id, "one");
        assert_eq!(cursor.key.invocation, 1);
        assert_eq!(fs::read(root.join("execute-count")).unwrap(), b"1");
        assert!(
            root.join(format!("node-{}.json", cursor.key.durable_key()))
                .exists(),
            "durable completion fact must survive {crash_name}"
        );

        let ports = DurableTestPorts {
            root: root.clone(),
            crash_point: None,
        };
        harness.run(&ports, &ports).await.unwrap();
        let recovered = harness.record();
        assert_eq!(
            harness.phase(),
            Phase::Settled {
                result: Outcome::Done
            }
        );
        assert!(harness.frontier().is_empty());
        assert_eq!(recovered.invocations["one"], 1);
        assert_eq!(recovered.passes["one"], 1);
        assert_eq!(recovered.results["one"].len(), 1);
        assert_eq!(fs::read(root.join("execute-count")).unwrap(), b"1");
        assert_eq!(fs::read(root.join("freeze-count")).unwrap(), b"1");

        let result = &recovered.results["one"][0];
        let artifact_path = ports.artifact_path(&result.key);
        let artifact: DurableTestArtifact =
            serde_json::from_slice(&fs::read(artifact_path).unwrap()).unwrap();
        assert_eq!(result.commit, artifact.commit);
        assert_eq!(result.commit.id, result.key.durable_key());
        assert_eq!(
            harness.record(),
            recovered,
            "the recovered result must be the durable Run fact"
        );

        fs::remove_dir_all(root).unwrap();
    }
}

#[tokio::test]
async fn file_run_store_recovers_not_started_invocation_after_process_crash() {
    let executable = std::env::current_exe().unwrap();
    let root = std::env::temp_dir().join(format!(
        "anchor-graph-recovery-{}-{}-before-dispatch",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();

    let initial = GraphRunRecord::create(graph(&["one"], &[], "one"), Value::Null).unwrap();
    let run_id = initial.run_id.clone();
    write_durable_json(&root.join("initial.json"), &initial).unwrap();

    let crashed = Command::new(&executable)
        .args([
            "--exact",
            "graph::tests::graph_runner_recovery_process_helper",
            "--nocapture",
        ])
        .env("ANCHOR_GRAPH_RECOVERY_TEST_ROOT", &root)
        .env("ANCHOR_GRAPH_RECOVERY_TEST_RUN", &run_id)
        .env("ANCHOR_GRAPH_RECOVERY_TEST_CRASH", "after-not-started")
        .output()
        .unwrap();
    assert_eq!(crashed.status.code(), Some(70));

    let harness = Harness::reload(FileStore::new(root.join("kernel")).unwrap(), &run_id);
    assert_eq!(harness.phase(), Phase::Executing);
    let cursor = harness
        .pending_input(
            &DurableTestPorts {
                root: root.clone(),
                crash_point: None,
            },
            "one",
        )
        .await;
    assert_eq!(cursor.node_id, "one");
    assert_eq!(cursor.key.invocation, 1);
    assert!(
        !root
            .join(format!("node-{}.json", cursor.key.durable_key()))
            .exists()
    );
    assert!(!root.join("execute-count").exists());
    drop(
        FileRunLeaseProvider::new(root.join("leases"))
            .acquire_lease(&run_id)
            .unwrap(),
    );

    let ports = DurableTestPorts {
        root: root.clone(),
        crash_point: None,
    };
    harness.run(&ports, &ports).await.unwrap();
    let recovered = harness.record();
    assert_eq!(
        harness.phase(),
        Phase::Settled {
            result: Outcome::Done
        }
    );
    assert!(harness.frontier().is_empty());
    assert_eq!(recovered.invocations["one"], 1);
    assert_eq!(recovered.passes["one"], 1);
    assert_eq!(recovered.results["one"].len(), 1);
    assert_eq!(fs::read(root.join("execute-count")).unwrap(), b"1");
    let result = &recovered.results["one"][0];
    assert_eq!(result.key, cursor.key);
    assert_eq!(result.commit.id, cursor.key.durable_key());
    assert_eq!(harness.record(), recovered);

    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn parallel_wave_recovers_each_branch_after_process_crash() {
    let executable = std::env::current_exe().unwrap();
    for (crash_name, expected_exit) in [
        ("after-not-started", 70),
        ("after-node-fact", 71),
        ("after-artifact-commit", 72),
    ] {
        let root = std::env::temp_dir().join(format!(
            "anchor-parallel-recovery-{}-{}-{crash_name}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();

        let initial = GraphRunRecord::create(parallel_graph(), Value::Null).unwrap();
        let run_id = initial.run_id.clone();
        write_durable_json(&root.join("initial.json"), &initial).unwrap();

        let crashed = Command::new(&executable)
            .args([
                "--exact",
                "graph::tests::graph_runner_recovery_process_helper",
                "--nocapture",
            ])
            .env("ANCHOR_GRAPH_RECOVERY_TEST_ROOT", &root)
            .env("ANCHOR_GRAPH_RECOVERY_TEST_RUN", &run_id)
            .env("ANCHOR_GRAPH_RECOVERY_TEST_CRASH", crash_name)
            .output()
            .unwrap();
        assert_eq!(
            crashed.status.code(),
            Some(expected_exit),
            "parallel child did not crash at {crash_name}; stdout={}, stderr={}",
            String::from_utf8_lossy(&crashed.stdout),
            String::from_utf8_lossy(&crashed.stderr)
        );

        let harness = Harness::reload(FileStore::new(root.join("kernel")).unwrap(), &run_id);
        let interrupted = harness.record();
        assert_eq!(harness.phase(), Phase::Executing);
        let activation = interrupted.parallel.as_ref().unwrap();
        assert!(
            activation
                .branches
                .iter()
                .all(|branch| branch.status == ParallelBranchStatus::Ready
                    || branch.status == ParallelBranchStatus::Completed)
        );
        assert!(!harness.frontier().is_empty());
        let ports = DurableTestPorts {
            root: root.clone(),
            crash_point: None,
        };
        harness.run(&ports, &ports).await.unwrap();
        let recovered = harness.record();
        assert_eq!(
            harness.phase(),
            Phase::Settled {
                result: Outcome::Done
            }
        );
        assert!(recovered.parallel.is_none());
        for node in ["left", "right", "after"] {
            assert_eq!(recovered.results[node].len(), 1, "{node}, {crash_name}");
        }
        assert_eq!(recovered.results["collect"].len(), 1);
        assert_eq!(harness.record(), recovered);

        fs::remove_dir_all(root).unwrap();
    }
}

#[tokio::test]
async fn file_run_store_recovers_durable_failed_node_without_replay_or_freeze() {
    let executable = std::env::current_exe().unwrap();
    let root = std::env::temp_dir().join(format!(
        "anchor-graph-recovery-{}-{}-after-failed-fact",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();

    let initial = GraphRunRecord::create(
        graph(&["one", "two"], &[("one", "two")], "one"),
        Value::Null,
    )
    .unwrap();
    let run_id = initial.run_id.clone();
    write_durable_json(&root.join("initial.json"), &initial).unwrap();

    let crashed = Command::new(&executable)
        .args([
            "--exact",
            "graph::tests::graph_runner_recovery_process_helper",
            "--nocapture",
        ])
        .env("ANCHOR_GRAPH_RECOVERY_TEST_ROOT", &root)
        .env("ANCHOR_GRAPH_RECOVERY_TEST_RUN", &run_id)
        .env("ANCHOR_GRAPH_RECOVERY_TEST_CRASH", "after-failed-fact")
        .output()
        .unwrap();
    assert_eq!(crashed.status.code(), Some(73));

    let harness = Harness::reload(FileStore::new(root.join("kernel")).unwrap(), &run_id);
    assert_eq!(harness.phase(), Phase::Executing);
    let cursor = harness
        .pending_input(
            &DurableTestPorts {
                root: root.clone(),
                crash_point: None,
            },
            "one",
        )
        .await;
    assert_eq!(cursor.node_id, "one");
    assert_eq!(cursor.key.invocation, 1);
    assert_eq!(fs::read(root.join("execute-count")).unwrap(), b"1");
    let fact: DurableTestNodeFact = serde_json::from_slice(
        &fs::read(root.join(format!("node-{}.json", cursor.key.durable_key()))).unwrap(),
    )
    .unwrap();
    assert_eq!(
        fact,
        DurableTestNodeFact::Failed("durable test failure".into())
    );
    drop(
        FileRunLeaseProvider::new(root.join("leases"))
            .acquire_lease(&run_id)
            .unwrap(),
    );

    let ports = DurableTestPorts {
        root: root.clone(),
        crash_point: None,
    };
    harness.run(&ports, &ports).await.unwrap();
    let recovered = harness.record();
    assert!(matches!(
        harness.phase(),
        Phase::Settled {
            result: Outcome::Failed { .. }
        }
    ));
    assert!(harness.resource().is_settled());
    assert_eq!(recovered.invocations["one"], 1);
    assert_eq!(recovered.passes["one"], 1);
    assert!(recovered.results.is_empty());
    assert!(
        recovered.decided.is_empty(),
        "failure must not settle outgoing edges"
    );
    assert!(harness.failure_detail().contains("durable test failure"));
    assert_eq!(fs::read(root.join("execute-count")).unwrap(), b"1");
    assert!(!root.join("freeze-count").exists());
    assert!(
        !root
            .join(format!("artifact-{}.json", cursor.key.durable_key()))
            .exists()
    );
    assert_eq!(harness.record(), recovered);

    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn graph_runner_recovery_process_helper() {
    let (Ok(root), Ok(run_id), Ok(crash_name)) = (
        std::env::var("ANCHOR_GRAPH_RECOVERY_TEST_ROOT"),
        std::env::var("ANCHOR_GRAPH_RECOVERY_TEST_RUN"),
        std::env::var("ANCHOR_GRAPH_RECOVERY_TEST_CRASH"),
    ) else {
        return;
    };
    if !PathBuf::from(&root)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.starts_with("anchor-graph-recovery-")
                || name.starts_with("anchor-parallel-recovery-")
        })
    {
        return;
    }
    let crash_point = match crash_name.as_str() {
        "after-not-started" => CrashPoint::NotStartedFactCheck,
        "after-node-fact" => CrashPoint::NodeFact,
        "after-failed-fact" => CrashPoint::FailedFact,
        "after-artifact-commit" => CrashPoint::ArtifactCommit,
        _ => panic!("unknown graph recovery crash point `{crash_name}`"),
    };
    let bytes = fs::read(PathBuf::from(&root).join("initial.json")).unwrap();
    let record: GraphRunRecord = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(record.run_id, run_id);

    let ports = DurableTestPorts {
        root: PathBuf::from(&root),
        crash_point: Some(crash_point),
    };
    let harness = Harness::with_store(
        FileStore::new(PathBuf::from(&root).join("kernel")).unwrap(),
        record,
    );
    harness.run(&ports, &ports).await.unwrap();
    assert_eq!(
        harness.phase(),
        Phase::Settled {
            result: Outcome::Done
        }
    );
}
