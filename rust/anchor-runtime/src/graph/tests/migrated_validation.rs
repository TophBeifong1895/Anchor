#[test]
fn parallel_activation_facts_bind_pair_and_branch_progress_and_refuse_legacy_cursors() {
    let snapshot = parallel_graph();
    let mut record = GraphRunRecord::create(snapshot, Value::Null).unwrap();
    let fanout_key = InvocationKey {
        run_id: record.run_id.clone(),
        graph_digest: record.graph_digest.clone(),
        node_id: "start".into(),
        invocation: 1,
    };
    record.invocations.insert("start".into(), 1);
    record.passes.insert("start".into(), 1);
    record.sequence = 1;
    record.results.insert(
        "start".into(),
        vec![RunResult {
            plugin_views: Default::default(),
            plugin_digests: Default::default(),
            node_id: "start".into(),
            interruption: None,
            key: fanout_key,
            completion: NodeCompletion {
                submission: "fanout".into(),
                route: None,
                model_requests: 0,
                output: Value::Null,
            },
            commit: CommitRef {
                id: "start-1".into(),
                node_id: "start".into(),
                invocation: 1,
            },
            sequence: 1,
        }],
    );
    record.sequence = 3;
    for (sequence, target) in [(2, "left"), (3, "right")] {
        record.decided.insert(
            edge_key("start", target),
            EdgeDecision {
                selected: true,
                sequence,
                source_invocation: 1,
                result_sequence: 1,
            },
        );
    }
    let activation_id = format!("{}:start:1", record.run_id);
    record.parallel = Some(ParallelActivation {
        activation_id: activation_id.clone(),
        fanout_node: "start".into(),
        join_node: "collect".into(),
        fanout_invocation: 1,
        branches: ["left", "right"]
            .into_iter()
            .enumerate()
            .map(|(index, node)| ParallelBranchRecord {
                branch_id: format!("{activation_id}:branch:{index}"),
                entry: node.into(),
                nodes: vec![node.into()],
                next_index: 0,
                status: ParallelBranchStatus::Ready,
                completed: vec![],
                error: None,
            })
            .collect(),
    });
    record.validate().unwrap();

    let mut invalid_pair = record.clone();
    invalid_pair.parallel.as_mut().unwrap().join_node = "after".into();
    assert!(matches!(
        invalid_pair.validate(),
        Err(GraphError::CorruptRun(_))
    ));

    let mut invalid_progress = record.clone();
    invalid_progress.parallel.as_mut().unwrap().branches[0].next_index = 2;
    assert!(matches!(
        invalid_progress.validate(),
        Err(GraphError::CorruptRun(_))
    ));
    let mut old = serde_json::to_value(&record).unwrap();
    old["parallel"]["branches"][0]["cursor"] = Value::Null;
    assert!(serde_json::from_value::<GraphRunRecord>(old).is_err());
}

#[test]
fn graph_record_validation_rejects_edge_decision_not_newer_than_its_result() {
    let mut record =
        GraphRunRecord::create(graph(&["spin"], &[("spin", "spin")], "spin"), Value::Null).unwrap();
    let key = InvocationKey {
        run_id: record.run_id.clone(),
        graph_digest: record.graph_digest.clone(),
        node_id: "spin".into(),
        invocation: 1,
    };
    let result_sequence = 1;
    record.results.insert(
        "spin".into(),
        vec![RunResult {
            plugin_views: Default::default(),
            plugin_digests: Default::default(),
            node_id: "spin".into(),
            interruption: None,
            key: key.clone(),
            completion: NodeCompletion {
                submission: "spin completed".into(),
                route: Some("spin".into()),
                model_requests: 1,
                output: Value::Null,
            },
            commit: CommitRef {
                id: key.durable_key(),
                node_id: "spin".into(),
                invocation: 1,
            },
            sequence: result_sequence,
        }],
    );
    record.invocations.insert("spin".into(), 1);
    record.passes.insert("spin".into(), 1);
    record.sequence = 2;
    record.decided.insert(
        "spin|spin".into(),
        EdgeDecision {
            selected: true,
            sequence: 2,
            source_invocation: 1,
            result_sequence,
        },
    );

    record.validate().unwrap();
    let mut stale = record;
    stale.decided.get_mut("spin|spin").unwrap().sequence = result_sequence;
    assert!(matches!(stale.validate(), Err(GraphError::CorruptRun(_))));
    let resource = Resource::new(
        "run/corrupt",
        validator::GRAPH_VALIDATOR,
        serde_json::to_value(stale).unwrap(),
    )
    .task(100);
    assert!(validator::record_of(&resource).unwrap().validate().is_err());
}

#[tokio::test]
async fn kernel_graph_validation_rejects_damaged_result_and_edge_facts() {
    let artifacts = MemoryArtifacts::default();
    let nodes = FakeNodes::default();
    let harness = Harness::memory(
        GraphRunRecord::create(
            graph(&["one", "two"], &[("one", "two")], "one"),
            Value::Null,
        )
        .unwrap(),
    );
    harness.run(&artifacts, &nodes).await.unwrap();
    assert_eq!(
        harness.phase(),
        Phase::Settled {
            result: Outcome::Done
        }
    );
    let valid = harness.record();
    let mut bad_records = Vec::new();
    let mut bad = valid.clone();
    bad.results.get_mut("one").unwrap()[0].node_id = "two".into();
    bad_records.push(bad);
    let mut bad = valid.clone();
    bad.results.get_mut("one").unwrap()[0].key.node_id = "two".into();
    bad_records.push(bad);
    let mut bad = valid.clone();
    bad.results.get_mut("one").unwrap()[0].commit.node_id = "two".into();
    bad_records.push(bad);
    let mut bad = valid.clone();
    bad.results.get_mut("one").unwrap()[0].sequence = valid.sequence + 1;
    bad_records.push(bad);
    let mut bad = valid.clone();
    let duplicate_sequence = bad.results["one"][0].sequence;
    bad.results.get_mut("two").unwrap()[0].sequence = duplicate_sequence;
    bad_records.push(bad);
    let mut bad = valid.clone();
    let selected = bad.decided.get_mut("one|two").unwrap();
    selected.result_sequence = valid.sequence + 1;
    bad_records.push(bad);
    let mut bad = valid.clone();
    bad.decided.insert(
        "one|ghost".into(),
        EdgeDecision {
            selected: false,
            sequence: valid.sequence,
            source_invocation: 0,
            result_sequence: 0,
        },
    );
    bad_records.push(bad);
    let mut bad = valid.clone();
    bad.decided.get_mut("one|two").unwrap().sequence = valid.sequence + 1;
    bad_records.push(bad);
    for bad in bad_records {
        assert!(matches!(bad.validate(), Err(GraphError::CorruptRun(_))));
        let resource = Resource::new(
            "run/corrupt",
            validator::GRAPH_VALIDATOR,
            serde_json::to_value(&bad).unwrap(),
        )
        .task(100);
        assert!(validator::record_of(&resource).unwrap().validate().is_err());
        let harness = Harness::memory(bad);
        let nodes = FakeNodes::default();
        let before = harness.resource();
        assert!(matches!(
            harness.run(&MemoryArtifacts::default(), &nodes).await,
            Err(GraphError::CorruptRun(_))
        ));
        assert_eq!(harness.resource(), before);
        assert!(nodes.calls.lock().unwrap().is_empty());
    }
    let mut loop_snapshot = graph(&["spin"], &[("spin", "spin")], "spin");
    loop_snapshot.nodes[0].max_rounds = Some(2);
    let looped = Harness::memory(GraphRunRecord::create(loop_snapshot, Value::Null).unwrap());
    looped.run(&artifacts, &nodes).await.unwrap();
    let mut reversed = looped.record();
    assert_eq!(reversed.results["spin"].len(), 2);
    reversed.results.get_mut("spin").unwrap().reverse();
    assert!(matches!(
        reversed.validate(),
        Err(GraphError::CorruptRun(_))
    ));
    let resource = Resource::new(
        "run/corrupt",
        validator::GRAPH_VALIDATOR,
        serde_json::to_value(reversed).unwrap(),
    )
    .task(100);
    assert!(validator::record_of(&resource).unwrap().validate().is_err());
}
