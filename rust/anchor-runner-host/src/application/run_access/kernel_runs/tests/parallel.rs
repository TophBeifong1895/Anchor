use super::*;
use anchor_runtime::graph::test_support::parallel_fixture_graph;
use tokio::sync::mpsc;

struct ParallelNodes {
    durable: Nodes,
    started: mpsc::UnboundedSender<String>,
    gates: std::collections::BTreeMap<String, Semaphore>,
}
impl NodeExecutionPort for ParallelNodes {
    fn capabilities(&self) -> NodeExecutionCapabilities {
        self.durable.capabilities()
    }
    fn completion_fact<'a>(
        &'a self,
        key: &'a InvocationKey,
    ) -> Pin<Box<dyn Future<Output = Result<CompletionFact, GraphError>> + Send + 'a>> {
        self.durable.completion_fact(key)
    }
    fn execute<'a>(
        &'a self,
        request: NodeExecutionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<NodeExecutionOutcome, GraphError>> + Send + 'a>> {
        Box::pin(async move {
            self.durable.trace.lock().unwrap().push(request.key.clone());
            self.started.send(request.key.node_id.clone()).unwrap();
            if let Some(gate) = self.gates.get(&request.key.node_id) {
                gate.acquire().await.unwrap().forget();
            }
            assert!(
                !request.cancellation.load(Ordering::Acquire),
                "pause must not cancel running siblings"
            );
            self.durable.save(&request.key);
            Ok(NodeExecutionOutcome::Completed(completion()))
        })
    }
}

#[tokio::test]
async fn parallel_pause_commits_current_result_drains_sibling_and_resumes_without_replaying() {
    let root = tempfile::tempdir().unwrap();
    let application = app(root.path());
    let runs = application.kernel_runs(true).unwrap().unwrap();
    let mut snapshot = parallel_fixture_graph();
    let mut tail = snapshot.nodes[1].clone();
    tail.id = "tail".into();
    snapshot.nodes.push(tail);
    snapshot
        .edges
        .iter_mut()
        .find(|e| e.from_node == "left")
        .unwrap()
        .to_node = "tail".into();
    snapshot.edges.push(GraphEdge {
        from_node: "tail".into(),
        to_node: "collect".into(),
    });
    let record = GraphRunRecord::create_with_id(snapshot, json!({}), "m2-parallel-pause").unwrap();
    runs.create(&record, 100).unwrap();
    let (started, mut events) = mpsc::unbounded_channel();
    let nodes = ParallelNodes {
        durable: Nodes::new(&root.path().join("facts"), Mode::Complete),
        started,
        gates: std::collections::BTreeMap::from([
            ("left".into(), Semaphore::new(0)),
            ("right".into(), Semaphore::new(0)),
        ]),
    };
    let control = async {
        let mut initial = std::collections::BTreeSet::new();
        for _ in 0..2 {
            initial.insert(
                tokio::time::timeout(std::time::Duration::from_secs(3), events.recv())
                    .await
                    .unwrap()
                    .unwrap(),
            );
        }
        assert_eq!(
            initial,
            std::collections::BTreeSet::from(["left".into(), "right".into()])
        );
        assert!(runs.active(&record.run_id));
        application.control(&record.run_id, "pause").await.unwrap();
        nodes.gates["left"].add_permits(1);
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if matches!(
                    runs.load(&record.run_id).unwrap().unwrap().phase(),
                    Some(Phase::Waiting { .. })
                ) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let resource = runs.load(&record.run_id).unwrap().unwrap();
        let paused = record_of(&resource).unwrap();
        assert_eq!(paused.results["left"].len(), 1);
        assert!(!paused.results.contains_key("right"));
        assert!(!paused.results.contains_key("tail"));
        assert!(
            runs.control(&record.run_id, "resume", None).is_err(),
            "worker must drain before resume"
        );
        nodes.gates["right"].add_permits(1);
    };
    let (result, ()) = tokio::join!(
        runs.drive(&record.run_id, &Artifacts, &nodes, cancel()),
        control
    );
    let paused = result.unwrap();
    assert_eq!(
        paused.phase(),
        Some(&Phase::Waiting {
            waiter: Waiter::Human {
                reason: "paused".into()
            }
        })
    );
    assert_eq!(paused.task.as_ref().unwrap().budget.spent, 6);
    assert!(!runs.active(&record.run_id));
    let history = runs
        .kernel
        .history(&run_resource_id(&record.run_id))
        .unwrap();
    let stop = history
        .iter()
        .find(|r| r.submission.content["reason"] == "paused")
        .unwrap();
    assert_eq!(stop.stamp.principal().kind, PrincipalKind::Human);
    assert!(
        history
            .iter()
            .any(|r| r.submission.slot.as_deref() == Some("right#1")
                && r.verdict.reason == Reason::TaskWaiting)
    );
    assert!(!runs.pause_pending(&record.run_id).unwrap());
    runs.control(&record.run_id, "resume", None).unwrap();
    let result = runs
        .drive(&record.run_id, &Artifacts, &nodes, cancel())
        .await
        .unwrap();
    assert_eq!(
        result.phase(),
        Some(&Phase::Settled {
            result: Outcome::Done
        })
    );
    let trace = nodes.durable.trace.lock().unwrap();
    for node in ["left", "right", "tail", "after"] {
        assert_eq!(trace.iter().filter(|k| k.node_id == node).count(), 1);
    }
    assert_eq!(record_of(&result).unwrap().results["collect"].len(), 1);
}
