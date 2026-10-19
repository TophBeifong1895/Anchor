//! Legacy call behavior copied to the production Kernel scheduler path.
use super::*;

#[derive(Default)]
struct Calls {
    outcomes: Mutex<Vec<GraphCallOutcome>>,
    identities: Mutex<Vec<CallIdentity>>,
    not_admitted: bool,
}
impl GraphCallPort for Calls {
    fn call<'a>(
        &'a self,
        identity: &'a CallIdentity,
        _: &'a Value,
        _: &'a Value,
        _: &'a [CommitRef],
        _: crate::Cancellation,
    ) -> Pin<Box<dyn Future<Output = Result<GraphCallOutcome, GraphError>> + Send + 'a>> {
        Box::pin(async move {
            self.identities.lock().unwrap().push(identity.clone());
            if self.not_admitted {
                return Err(GraphError::CallAdmissionFailed(
                    "input_map is invalid".into(),
                ));
            }
            Ok(self.outcomes.lock().unwrap().remove(0))
        })
    }
}

#[tokio::test]
async fn graph_call_admission_failure_is_code_only_and_cannot_erase_a_bound_child() {
    use crate::graph::validator::CallSubmission;
    let (kernel, id) = kernel(snapshot("wait"));
    let calls = Calls {
        not_admitted: true,
        ..Default::default()
    };
    let content = serde_json::to_value(CallSubmission::AdmissionFailed {
        reason: "input_map is invalid".into(),
    })
    .unwrap();
    let initial = kernel.load(&id).unwrap().unwrap();
    let slot = GraphValidator::frontier(&initial).unwrap()[0].slot();
    let submission = Submission::new(&id, initial.version, Action::Write)
        .slot(&slot)
        .content(content.clone());
    let verdict = kernel
        .submit(
            &Principal::new(EXECUTOR_ID, PrincipalKind::Agent),
            0,
            submission.clone(),
        )
        .unwrap();
    assert_eq!(verdict.reason, Reason::Unauthorized);
    let done = drive(&kernel, &id, &calls, &TrackingArtifacts::default()).await;
    assert!(matches!(
        done.phase(),
        Some(Phase::Settled {
            result: Outcome::Failed { .. }
        })
    ));
    let record = record_of(&done).unwrap();
    assert!(record.graph_calls.is_empty());
    assert!(record.results.is_empty());
    assert_eq!(record.invocations["call"], 1);
    assert_eq!(record.passes["call"], 1);

    let (kernel, id) = super::kernel(snapshot("wait"));
    let host = Principal::new(HOST_ID, PrincipalKind::Code);
    let initial = kernel.load(&id).unwrap().unwrap();
    let bound = Submission::new(&id, initial.version, Action::Write)
        .slot(&slot)
        .content(
            serde_json::to_value(CallSubmission::Bound {
                child_run_id: "child".into(),
            })
            .unwrap(),
        );
    assert!(kernel.submit(&host, 0, bound).unwrap().is_accepted());
    let waiting = kernel.load(&id).unwrap().unwrap();
    assert!(
        kernel
            .submit(
                &host,
                0,
                Submission::new(&id, waiting.version, Action::Request)
                    .content(serde_json::json!({"control":"resume"}))
            )
            .unwrap()
            .is_accepted()
    );
    let before = kernel.load(&id).unwrap().unwrap();
    let verdict = kernel
        .submit(
            &host,
            0,
            Submission::new(&id, before.version, Action::Write)
                .slot(&slot)
                .content(content),
        )
        .unwrap();
    assert_eq!(verdict.reason, Reason::ContentInvalid);
    assert_eq!(
        record_of(&before).unwrap(),
        record_of(&kernel.load(&id).unwrap().unwrap()).unwrap()
    );
}
impl NodeExecutionPort for Calls {
    fn graph_call_port(&self) -> Option<&dyn GraphCallPort> {
        Some(self)
    }
    fn capabilities(&self) -> NodeExecutionCapabilities {
        NodeExecutionCapabilities::default()
    }
    fn completion_fact<'a>(
        &'a self,
        _: &'a InvocationKey,
    ) -> Pin<Box<dyn Future<Output = Result<CompletionFact, GraphError>> + Send + 'a>> {
        Box::pin(async { panic!("Op.call must use the Host call port") })
    }
    fn execute<'a>(
        &'a self,
        _: NodeExecutionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<NodeExecutionOutcome, GraphError>> + Send + 'a>> {
        Box::pin(async { panic!("Op.call must not execute as a model node") })
    }
}
fn snapshot(mode: &str) -> GraphSnapshot {
    let mut graph = parallel_fixture_graph();
    graph.nodes.truncate(1);
    graph.nodes[0].id = "call".into();
    graph.nodes[0].op = Some("child".into());
    graph.entry = "call".into();
    graph.edges.clear();
    graph.ops.clear();
    graph.ops.insert(
        "child".into(),
        serde_json::json!({"call":{"graph":"child-graph","mode":mode}}),
    );
    graph
}
async fn drive(
    kernel: &Kernel<MemoryStore>,
    id: &str,
    calls: &Calls,
    artifacts: &TrackingArtifacts,
) -> Resource {
    KernelScheduler {
        kernel,
        observer: None,
        boundary: None,
        artifacts,
        nodes: calls,
        cancellation: Arc::new(AtomicBool::new(false)),
    }
    .run(id)
    .await
    .unwrap()
}

#[tokio::test]
async fn graph_call_wait_resumes_same_identity_and_detach_records_acceptance() {
    let calls = Calls {
        outcomes: Mutex::new(vec![
            GraphCallOutcome::Waiting {
                child_run_id: "child-run-1".into(),
            },
            GraphCallOutcome::Waiting {
                child_run_id: "child-run-1".into(),
            },
            GraphCallOutcome::Completed {
                child_run_id: "child-run-1".into(),
                output: serde_json::json!({"answer":42}),
            },
        ]),
        ..Default::default()
    };
    let (kernel, id) = kernel(snapshot("wait"));
    let artifacts = TrackingArtifacts::default();
    let first = drive(&kernel, &id, &calls, &artifacts).await;
    assert_eq!(
        first.phase(),
        Some(&Phase::Waiting {
            waiter: Waiter::Child {
                task: "run/child-run-1".into()
            }
        })
    );
    let resumed = drive(&kernel, &id, &calls, &artifacts).await;
    assert_eq!(
        resumed.phase(),
        Some(&Phase::Settled {
            result: Outcome::Done
        })
    );
    {
        let identities = calls.identities.lock().unwrap();
        assert_eq!(identities.len(), 3);
        assert!(identities.iter().all(|i| i == &identities[0]));
    }
    assert_eq!(
        record_of(&resumed)
            .unwrap()
            .graph_calls
            .values()
            .next()
            .unwrap()
            .status,
        GraphCallStatus::Completed
    );
    assert_eq!(
        artifacts.0.lock().unwrap()[0].1.kind,
        ArtifactKind::GraphCall
    );
    let history = kernel.history(&id).unwrap();
    assert!(
        history
            .iter()
            .all(|h| h.stamp.principal().kind == PrincipalKind::Code)
    );
    assert!(
        history
            .iter()
            .any(|h| h.submission.content["control"] == "resume")
    );

    for outcome in [
        GraphCallOutcome::Detached {
            child_run_id: "child-run-2".into(),
        },
        GraphCallOutcome::Completed {
            child_run_id: "child-run-3".into(),
            output: serde_json::json!({"secret_result":"not forwarded"}),
        },
    ] {
        let calls = Calls {
            outcomes: Mutex::new(vec![outcome]),
            ..Default::default()
        };
        let (kernel, id) = super::kernel(snapshot("detach"));
        let result = drive(&kernel, &id, &calls, &TrackingArtifacts::default()).await;
        let record = record_of(&result).unwrap();
        let call_result = record.results["call"].last().unwrap();
        assert_eq!(call_result.completion.output["status"], "accepted");
        assert!(call_result.completion.output.get("secret_result").is_none());
    }
}

#[tokio::test]
async fn graph_call_identity_must_keep_one_child_run_across_reload() {
    let calls = Calls {
        outcomes: Mutex::new(vec![
            GraphCallOutcome::Waiting {
                child_run_id: "child-run-1".into(),
            },
            GraphCallOutcome::Waiting {
                child_run_id: "child-run-1".into(),
            },
            GraphCallOutcome::Completed {
                child_run_id: "child-run-2".into(),
                output: Value::Null,
            },
        ]),
        ..Default::default()
    };
    let (kernel, id) = kernel(snapshot("wait"));
    let artifacts = TrackingArtifacts::default();
    let waiting = drive(&kernel, &id, &calls, &artifacts).await;
    let result = KernelScheduler {
        kernel: &kernel,
        observer: None,
        boundary: None,
        artifacts: &artifacts,
        nodes: &calls,
        cancellation: Arc::new(AtomicBool::new(false)),
    }
    .run(&id)
    .await;
    assert!(matches!(result, Err(GraphError::CorruptRun(_))));
    assert_eq!(kernel.load(&id).unwrap().unwrap().phase(), waiting.phase());
    let record = record_of(&kernel.load(&id).unwrap().unwrap()).unwrap();
    assert_eq!(
        record
            .graph_calls
            .values()
            .next()
            .unwrap()
            .child_run_id
            .as_deref(),
        Some("child-run-1")
    );
    assert_eq!(record.results, record_of(&waiting).unwrap().results);
}
