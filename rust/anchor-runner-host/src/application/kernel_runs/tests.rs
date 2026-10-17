use super::*;
use anchor_kernel::{Evidence, Outcome, Reason, Waiter};
use anchor_runtime::graph::test_support::{
    Artifacts, Scripted, ScriptedCalls, facts, random_graph, run_old,
};
use anchor_runtime::graph::*;
use serde_json::json;
use std::{future::Future, pin::Pin, sync::atomic::AtomicBool};
use tokio::sync::{Notify, Semaphore};

#[derive(Clone, Copy)]
enum Mode {
    Complete,
    Fail,
    Gate,
    Crash(bool),
    Recovery,
}
struct Nodes {
    root: std::path::PathBuf,
    mode: Mode,
    once: AtomicBool,
    trace: Mutex<Vec<InvocationKey>>,
    started: Notify,
    release: Semaphore,
}
impl Nodes {
    fn new(root: &Path, mode: Mode) -> Self {
        std::fs::create_dir_all(root).unwrap();
        Self {
            root: root.into(),
            mode,
            once: AtomicBool::new(true),
            trace: Mutex::default(),
            started: Notify::new(),
            release: Semaphore::new(0),
        }
    }
    fn fact_path(&self, key: &InvocationKey) -> std::path::PathBuf {
        self.root.join(key.durable_key())
    }
    fn save(&self, key: &InvocationKey) {
        std::fs::write(
            self.fact_path(key),
            serde_json::to_vec(&completion()).unwrap(),
        )
        .unwrap();
    }
}
fn completion() -> NodeCompletion {
    NodeCompletion {
        submission: "fixture result".into(),
        route: None,
        model_requests: 3,
        output: json!({"proof":"host-port"}),
    }
}
impl NodeExecutionPort for Nodes {
    fn capabilities(&self) -> NodeExecutionCapabilities {
        NodeExecutionCapabilities {
            agent: true,
            exact_provider_request_budget: true,
            ..Default::default()
        }
    }
    fn completion_fact<'a>(
        &'a self,
        key: &'a InvocationKey,
    ) -> Pin<Box<dyn Future<Output = Result<CompletionFact, GraphError>> + Send + 'a>> {
        Box::pin(async move {
            match std::fs::read(self.fact_path(key)) {
                Ok(bytes) if bytes == b"unknown" => {
                    Ok(CompletionFact::Uncertain("verify external effect".into()))
                }
                Ok(bytes) => Ok(CompletionFact::Completed(
                    serde_json::from_slice(&bytes).unwrap(),
                )),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    Ok(CompletionFact::NotStarted)
                }
                Err(e) => Err(e.into()),
            }
        })
    }
    fn execute<'a>(
        &'a self,
        request: NodeExecutionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<NodeExecutionOutcome, GraphError>> + Send + 'a>> {
        Box::pin(async move {
            self.trace.lock().unwrap().push(request.key.clone());
            let first = self.once.swap(false, Ordering::AcqRel);
            match self.mode {
                Mode::Fail => {
                    return Ok(NodeExecutionOutcome::Failed {
                        reason: "known failure".into(),
                    });
                }
                Mode::Gate if first => {
                    self.started.notify_one();
                    self.release.acquire().await.unwrap().forget();
                    assert!(request.cancellation.load(Ordering::Acquire));
                }
                Mode::Crash(completed) if request.key.node_id == "b" => {
                    if completed {
                        self.save(&request.key);
                    }
                    return Err(std::io::Error::other("fixture process interruption").into());
                }
                Mode::Recovery if first => {
                    return Ok(NodeExecutionOutcome::WaitingRecovery { attempts: vec![] });
                }
                _ => {}
            }
            self.save(&request.key);
            Ok(NodeExecutionOutcome::Completed(completion()))
        })
    }
}
fn record(id: &str) -> GraphRunRecord {
    GraphRunRecord::create_with_id(serde_json::from_value(json!({
        "objective":"probe", "entry":"a", "agents":{"worker":{"model":"fixture","instructions":"work"}},
        "nodes":[{"id":"a","agent":"worker"},{"id":"b","agent":"worker"},{"id":"c","agent":"worker"}],
        "edges":[{"from":"a","to":"b"},{"from":"b","to":"c"}]
    })).unwrap(), json!({}), id).unwrap()
}
fn app(root: &Path) -> RunApplication {
    RunApplication::new(root.into(), root.join("catalog"))
}
fn cancel() -> anchor_runtime::Cancellation {
    Arc::new(AtomicBool::new(false))
}

#[tokio::test]
async fn serial_probe_commits_artifacts_cost_and_phase_through_application() {
    let root = tempfile::tempdir().unwrap();
    let application = app(root.path());
    let runs = application.kernel_runs(true).unwrap().unwrap();
    let record = record("p-serial");
    runs.create(&record, 30).unwrap();
    let nodes = Nodes::new(&root.path().join("facts"), Mode::Complete);
    let resource = runs
        .drive(&record.run_id, &Artifacts, &nodes, cancel())
        .await
        .unwrap();
    assert_eq!(
        resource.phase(),
        Some(&Phase::Settled {
            result: Outcome::Done
        })
    );
    assert_eq!(resource.task.as_ref().unwrap().budget.spent, 9);
    let projected = application.load_run(&record.run_id).unwrap().unwrap();
    assert_eq!(projected.status, RunStatus::Completed);
    assert_eq!(projected.results.len(), 3);
    for report in runs
        .kernel
        .history(&run_resource_id(&record.run_id))
        .unwrap()
    {
        assert_eq!(report.stamp.principal().kind, PrincipalKind::Agent);
        assert_eq!(report.stamp.cost(), 3);
        assert!(matches!(
            report.submission.evidence.as_slice(),
            [Evidence::Resource { .. }]
        ));
    }
    assert!(
        application
            .project_run(&projected, None)
            .await
            .unwrap()
            .terminal
    );
    assert!(!root.path().join("runs").exists());
    assert_eq!(application.records().unwrap().len(), 1);
}

#[tokio::test]
async fn stop_refuses_late_result_then_resume_uses_durable_completion() {
    let root = tempfile::tempdir().unwrap();
    let application = app(root.path());
    let runs = application.kernel_runs(true).unwrap().unwrap();
    let record = record("p-stop");
    runs.create(&record, 50).unwrap();
    let nodes = Arc::new(Nodes::new(&root.path().join("facts"), Mode::Gate));
    let worker_runs = runs.clone();
    let worker_nodes = nodes.clone();
    let worker = tokio::spawn(async move {
        worker_runs
            .drive("p-stop", &Artifacts, worker_nodes.as_ref(), cancel())
            .await
    });
    nodes.started.notified().await;
    application.control("p-stop", "stop").await.unwrap();
    assert!(matches!(
        runs.load("p-stop").unwrap().unwrap().phase(),
        Some(Phase::Waiting { .. })
    ));
    nodes.release.add_permits(1);
    worker.await.unwrap().unwrap();
    let history = runs.kernel.history(&run_resource_id("p-stop")).unwrap();
    assert!(
        history
            .iter()
            .any(|r| r.verdict.reason == Reason::TaskWaiting
                && r.submission.slot.as_deref() == Some("a#1"))
    );
    assert_eq!(
        application
            .load_run("p-stop")
            .unwrap()
            .unwrap()
            .results
            .len(),
        0
    );
    runs.control("p-stop", "resume", None).unwrap();
    let result = runs
        .drive("p-stop", &Artifacts, nodes.as_ref(), cancel())
        .await
        .unwrap();
    assert!(result.is_settled());
    assert_eq!(
        nodes
            .trace
            .lock()
            .unwrap()
            .iter()
            .filter(|k| k.node_id == "a")
            .count(),
        1
    );
}

#[tokio::test]
async fn abandon_waiting_probe_is_terminal_and_refuses_late_submission() {
    let root = tempfile::tempdir().unwrap();
    let application = app(root.path());
    let runs = application.kernel_runs(true).unwrap().unwrap();
    let record = record("p-abandon");
    runs.create(&record, 50).unwrap();
    application.control("p-abandon", "stop").await.unwrap();
    application.abandon_run("p-abandon", None).await.unwrap();
    let resource = runs.load("p-abandon").unwrap().unwrap();
    assert!(matches!(
        resource.phase(),
        Some(Phase::Settled {
            result: Outcome::Aborted { .. }
        })
    ));
    let verdict = runs
        .kernel
        .submit(
            &Principal::new(HOST_ID, PrincipalKind::Code),
            0,
            Submission::new(
                run_resource_id("p-abandon"),
                resource.version,
                Action::Settle,
            )
            .slot("a#1")
            .content(json!({"reason":"late"})),
        )
        .unwrap();
    assert_eq!(verdict.reason, Reason::TaskSettled);
    assert!(
        application
            .project_run(&application.load_run("p-abandon").unwrap().unwrap(), None)
            .await
            .unwrap()
            .terminal
    );
}

#[tokio::test]
async fn restart_recovers_same_invocation_from_completed_or_not_started_fact() {
    for completed in [true, false] {
        let root = tempfile::tempdir().unwrap();
        let application = app(root.path());
        let runs = application.kernel_runs(true).unwrap().unwrap();
        let record = record("p-restart");
        runs.create(&record, 50).unwrap();
        let crashing = Nodes::new(&root.path().join("facts"), Mode::Crash(completed));
        assert!(
            runs.drive("p-restart", &Artifacts, &crashing, cancel())
                .await
                .is_err()
        );
        assert_eq!(
            crashing
                .trace
                .lock()
                .unwrap()
                .iter()
                .map(|k| (&*k.node_id, k.invocation))
                .collect::<Vec<_>>(),
            vec![("a", 1), ("b", 1)]
        );
        drop(runs);
        drop(application);
        let fresh = app(root.path());
        let runs = fresh.kernel_runs(false).unwrap().unwrap();
        let resumed = Nodes::new(&root.path().join("facts"), Mode::Complete);
        assert!(
            runs.drive("p-restart", &Artifacts, &resumed, cancel())
                .await
                .unwrap()
                .is_settled()
        );
        let trace = resumed.trace.lock().unwrap();
        assert!(trace.iter().all(|k| k.node_id != "a" && k.invocation == 1));
        assert_eq!(
            trace.iter().filter(|k| k.node_id == "b").count(),
            usize::from(!completed)
        );
        assert_eq!(
            fresh.load_run("p-restart").unwrap().unwrap().invocations["b"],
            1
        );
    }
}

#[tokio::test]
async fn uncertain_effect_parks_until_human_verification_then_resume_or_abandon() {
    for abandon in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let application = app(root.path());
        let runs = application.kernel_runs(true).unwrap().unwrap();
        let record = record("p-unknown");
        runs.create(&record, 50).unwrap();
        let nodes = Nodes::new(&root.path().join("facts"), Mode::Complete);
        let key = InvocationKey {
            run_id: record.run_id.clone(),
            graph_digest: record.graph_digest.clone(),
            node_id: "a".into(),
            invocation: 1,
        };
        std::fs::write(nodes.fact_path(&key), b"unknown").unwrap();
        runs.drive("p-unknown", &Artifacts, &nodes, cancel())
            .await
            .unwrap();
        assert!(nodes.trace.lock().unwrap().is_empty());
        let history = runs.kernel.history(&run_resource_id("p-unknown")).unwrap();
        assert_eq!(history[0].stamp.principal().kind, PrincipalKind::Code);
        assert!(matches!(
            history[0].submission.evidence.as_slice(),
            [Evidence::Observation { .. }]
        ));
        if abandon {
            application.abandon_run("p-unknown", None).await.unwrap();
        } else {
            // The operator has verified the durable result, before resume.
            nodes.save(&key);
            runs.control("p-unknown", "resume", None).unwrap();
            runs.drive("p-unknown", &Artifacts, &nodes, cancel())
                .await
                .unwrap();
            assert!(nodes.trace.lock().unwrap().iter().all(|k| k.node_id != "a"));
        }
        assert!(runs.load("p-unknown").unwrap().unwrap().is_settled());
    }
}

#[tokio::test]
async fn waiting_recovery_outcome_parks_and_known_failure_settles() {
    for mode in [Mode::Recovery, Mode::Fail] {
        let root = tempfile::tempdir().unwrap();
        let application = app(root.path());
        let runs = application.kernel_runs(true).unwrap().unwrap();
        let record = record("p-outcome");
        runs.create(&record, 50).unwrap();
        let nodes = Nodes::new(&root.path().join("facts"), mode);
        let resource = runs
            .drive("p-outcome", &Artifacts, &nodes, cancel())
            .await
            .unwrap();
        match mode {
            Mode::Recovery => {
                assert!(matches!(
                    resource.phase(),
                    Some(Phase::Waiting {
                        waiter: Waiter::Human { .. }
                    })
                ));
                assert_eq!(nodes.trace.lock().unwrap().len(), 1);
            }
            Mode::Fail => {
                assert!(
                    matches!(resource.phase(), Some(Phase::Settled { result: Outcome::Failed { detail, .. } }) if detail == "a failed: known failure")
                );
                let record = record_of(&resource).unwrap();
                assert!(record.results.is_empty());
                assert_eq!(record.invocations["a"], 1);
            }
            _ => unreachable!(),
        }
    }
}

#[tokio::test]
async fn application_probe_matches_old_runner_on_one_thousand_serial_seeds() {
    let root = tempfile::tempdir().unwrap();
    let application = app(root.path());
    let runs = application.kernel_runs(true).unwrap().unwrap();
    for seed in 0..1000 {
        let record =
            GraphRunRecord::create_with_id(random_graph(seed), json!({}), format!("p-diff-{seed}"))
                .unwrap();
        let (old_trace, old) = run_old(record.clone(), seed, true).await;
        runs.create(&record, u64::MAX).unwrap();
        let nodes = Scripted {
            seed,
            inject_failures: true,
            trace: Mutex::default(),
            calls: ScriptedCalls {
                seed,
                ..Default::default()
            },
        };
        let resource = runs
            .drive(&record.run_id, &Artifacts, &nodes, cancel())
            .await
            .unwrap();
        assert_eq!(old_trace, *nodes.trace.lock().unwrap(), "trace seed {seed}");
        assert_eq!(
            facts(&old),
            facts(&record_of(&resource).unwrap()),
            "facts seed {seed}"
        );
        match old.status {
            RunStatus::Completed => assert_eq!(
                resource.phase(),
                Some(&Phase::Settled {
                    result: Outcome::Done
                })
            ),
            RunStatus::Stopped => assert!(matches!(
                resource.phase(),
                Some(Phase::Settled {
                    result: Outcome::Escalated { .. }
                })
            )),
            RunStatus::Failed => assert!(
                matches!(resource.phase(), Some(Phase::Settled { result:Outcome::Failed { detail, .. } }) if Some(detail) == old.error.as_ref())
            ),
            status => panic!("unexpected {status:?}"),
        }
    }
}

#[test]
fn probe_guards_keep_one_writer_and_charge_stale_work_once() {
    let root = tempfile::tempdir().unwrap();
    let application = app(root.path());
    let runs = application.kernel_runs(true).unwrap().unwrap();
    let record = record("p-stale");
    runs.create(&record, 50).unwrap();
    assert!(matches!(
        app(root.path()).kernel_runs(false),
        Err(ApplicationError::Conflict(_))
    ));
    runs.control("p-stale", "stop", None).unwrap();
    runs.control("p-stale", "resume", None).unwrap();
    let key = InvocationKey {
        run_id: record.run_id.clone(),
        graph_digest: record.graph_digest.clone(),
        node_id: "a".into(),
        invocation: 1,
    };
    let commit = anchor_runtime::graph::test_support::commit_of(&key);
    let content = anchor_runtime::graph::validator::NodeSubmission {
        summary: "done".into(),
        route: None,
        output: json!({}),
        model_requests: 999999,
        commit: commit.clone(),
        plugin_views: Default::default(),
        plugin_digests: Default::default(),
        interruption: None,
    };
    let verdict = submit_with_retry(
        &runs.kernel,
        &Principal::new(EXECUTOR_ID, PrincipalKind::Agent),
        3,
        Submission::new(run_resource_id("p-stale"), 0, Action::Write)
            .slot("a#1")
            .content(serde_json::to_value(content).unwrap())
            .evidence(Evidence::Resource {
                id: commit.id,
                version: 0,
            }),
        None,
    )
    .unwrap();
    assert!(verdict.is_accepted());
    let history = runs.kernel.history(&run_resource_id("p-stale")).unwrap();
    assert_eq!(history[2].verdict.reason, Reason::StaleVersion);
    assert_eq!((history[2].stamp.cost(), history[3].stamp.cost()), (3, 0));
    let resource = runs.load("p-stale").unwrap().unwrap();
    assert_eq!(resource.task.as_ref().unwrap().budget.spent, 3);
    let refused = runs
        .kernel
        .submit(
            &Principal::new(EXECUTOR_ID, PrincipalKind::Agent),
            0,
            Submission::new(&resource.id, resource.version, Action::Settle)
                .slot("b#1")
                .content(json!({"reason":"model claim"})),
        )
        .unwrap();
    assert_eq!(refused.reason, Reason::Unauthorized);
    FileRunStore::new(root.path().join("runs"))
        .save(&record)
        .unwrap();
    assert!(matches!(
        application.load_run("p-stale"),
        Err(ApplicationError::Conflict(_))
    ));
}

#[test]
fn broken_host_observation_never_blocks_kernel_control_or_lifecycle_reads() {
    let root = tempfile::tempdir().unwrap();
    let application = app(root.path());
    let runs = application.kernel_runs(true).unwrap().unwrap();
    let record = record("r-observation-failure");
    runs.create(&record, 50).unwrap();
    let observations = root.path().join("kernel-run-observations");
    std::fs::remove_file(observations.join("r-observation-failure.json")).unwrap();
    std::fs::remove_dir(&observations).unwrap();
    std::fs::write(&observations, b"fixture unavailable").unwrap();
    runs.control(&record.run_id, "stop", None).unwrap();
    let resource = runs.load(&record.run_id).unwrap().unwrap();
    let view = runs.view(&resource).unwrap();
    assert!(matches!(
        view.phase,
        Phase::Waiting {
            waiter: Waiter::Human { .. }
        }
    ));
    assert!(view.updated.is_none());
    assert!(view.actions().resume);
    runs.control(&record.run_id, "abandon", None).unwrap();
    assert!(runs.load(&record.run_id).unwrap().unwrap().is_settled());
}
