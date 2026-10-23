//! Legacy host-operation contracts exercised through the production scheduler.
use super::*;
use crate::graph::{scheduler::*, validator::*};
use anchor_kernel::{
    Action, Authority, Kernel, MemoryStore, Outcome, Phase, Principal, PrincipalKind, Submission,
    Waiter,
};
use std::sync::atomic::AtomicBool;

struct HostAuthority;
impl Authority for HostAuthority {
    fn permits(&self, p: &Principal, action: Action, _: &str) -> bool {
        match p.kind {
            PrincipalKind::Agent => action == Action::Write,
            PrincipalKind::Code => {
                matches!(action, Action::Write | Action::Settle | Action::Request)
            }
            PrincipalKind::Human => action == Action::Request,
        }
    }
}

fn kernel(snapshot: GraphSnapshot) -> (Kernel<MemoryStore>, String) {
    let resource = run_resource(
        &GraphRunRecord::create(snapshot, Value::Null).unwrap(),
        u64::MAX,
    )
    .unwrap();
    let id = resource.id.clone();
    let mut kernel = Kernel::new(MemoryStore::default(), Box::new(HostAuthority)).unwrap();
    kernel
        .install(GRAPH_VALIDATOR, Arc::new(GraphValidator))
        .unwrap();
    kernel.create(resource).unwrap();
    (kernel, id)
}

fn scheduler<'a>(
    kernel: &'a Kernel<MemoryStore>,
    artifacts: &'a ControlArtifacts,
    nodes: &'a ScriptedNodes,
) -> KernelScheduler<'a, MemoryStore, ControlArtifacts, ScriptedNodes> {
    KernelScheduler {
        kernel,
        artifacts,
        nodes,
        observer: None,
        boundary: None,
        cancellation: Arc::new(AtomicBool::new(false)),
    }
}

#[tokio::test]
async fn host_input_reply_loop_and_yield_keep_control_provenance_and_suppression() {
    // Same two-turn/yield sequence as the legacy tests. Suppression remains an
    // execution/Session observation, passed through the Graph unchanged.
    let (kernel, id) = kernel(assistant_loop());
    let artifacts = ControlArtifacts::default();
    let nodes = ScriptedNodes::new(vec![
        completed(serde_json::json!({"turn":1}), None, 0),
        NodeExecutionOutcome::Yielded {
            reason: "superseded".into(),
            route: Some("wait".into()),
        },
        completed(serde_json::json!({"turn":2}), None, 0),
        completed(serde_json::json!({"answer":2}), Some("reply"), 1),
        completed(
            serde_json::json!({"suppressed":true,"reply_for":"turn-2"}),
            None,
            0,
        ),
        NodeExecutionOutcome::Suspended,
    ]);
    let scheduler = scheduler(&kernel, &artifacts, &nodes);
    let resource = scheduler.run(&id).await.unwrap();
    assert_eq!(resource.phase(), Some(&Phase::Executing));
    let record = record_of(&resource).unwrap();
    assert_eq!(record.results["work"].len(), 2);
    assert_eq!(record.results["reply"].len(), 1);
    assert_eq!(
        record.results["reply"][0].completion.output["suppressed"],
        true
    );
    let interrupted = &record.results["work"][0];
    assert_eq!(interrupted.interruption.as_deref(), Some("superseded"));
    assert_eq!(
        interrupted.completion,
        interruption_completion("superseded", Some("wait".into()))
    );
    assert_eq!(record.results["work"][1].key.invocation, 2);
    assert_eq!(record.results["work"][1].completion.model_requests, 1);
    assert_eq!(resource.task.as_ref().unwrap().budget.spent, 1);
    let contexts = artifacts.contexts.lock().unwrap();
    let controls: Vec<_> = contexts
        .iter()
        .filter(|(_, c)| c.kind == ArtifactKind::Interruption)
        .collect();
    assert_eq!(controls.len(), 1);
    assert_eq!(controls[0].0, interrupted.key);
    assert_eq!(
        controls[0].1.input_commits,
        vec![record.results["wait"][0].commit.clone()]
    );
    assert_eq!(*artifacts.workspace_freezes.lock().unwrap(), 4);
    let history = kernel.history(&id).unwrap();
    let control = history
        .iter()
        .find(|h| h.submission.content["interruption"] == "superseded")
        .unwrap();
    assert_eq!(control.stamp.principal().kind, PrincipalKind::Code);
    assert!(control.verdict.is_accepted());
    record.validate().unwrap();
}

#[tokio::test]
async fn budget_and_interruption_park_with_observation_then_resume_the_same_invocation() {
    for (outcome, reason, cost) in [
        (
            NodeExecutionOutcome::BudgetExhausted { model_requests: 2 },
            "budget",
            2,
        ),
        (
            NodeExecutionOutcome::Interrupted {
                reason: "executor interruption".into(),
            },
            "executor interruption",
            0,
        ),
    ] {
        let (kernel, id) = kernel(graph(&["work"], &[], "work"));
        let artifacts = ControlArtifacts::default();
        let nodes = ScriptedNodes::new(vec![
            outcome,
            completed(serde_json::json!({"answer":42}), None, 1),
        ]);
        let scheduler = scheduler(&kernel, &artifacts, &nodes);
        let waiting = scheduler.run(&id).await.unwrap();
        assert_eq!(
            waiting.phase(),
            Some(&Phase::Waiting {
                waiter: Waiter::Human {
                    reason: reason.into()
                }
            })
        );
        assert_eq!(waiting.task.as_ref().unwrap().budget.spent, cost);
        assert!(record_of(&waiting).unwrap().results.is_empty());
        assert!(artifacts.contexts.lock().unwrap().is_empty());
        assert_eq!(scheduler.run(&id).await.unwrap(), waiting);
        assert_eq!(nodes.calls.lock().unwrap().len(), 1);
        let history = kernel.history(&id).unwrap();
        assert_eq!(history[0].stamp.principal().kind, PrincipalKind::Code);
        assert!(
            history[0]
                .submission
                .evidence
                .iter()
                .any(|e| matches!(e, anchor_kernel::Evidence::Observation { .. }))
        );
        assert!(
            kernel
                .submit(
                    &Principal::new(OPERATOR_ID, PrincipalKind::Human),
                    0,
                    Submission::new(&id, waiting.version, Action::Request)
                        .content(serde_json::json!({"control":"resume"}))
                )
                .unwrap()
                .is_accepted()
        );
        let done = scheduler.run(&id).await.unwrap();
        assert_eq!(
            done.phase(),
            Some(&Phase::Settled {
                result: Outcome::Done
            })
        );
        assert_eq!(done.task.as_ref().unwrap().budget.spent, cost + 1);
        let calls = nodes.calls.lock().unwrap();
        assert_eq!(calls[0].key, calls[1].key);
    }
}

#[tokio::test]
async fn explicit_operator_abort_settles_once_without_node_artifacts() {
    let (kernel, id) = kernel(graph(&["work"], &[], "work"));
    let artifacts = ControlArtifacts::default();
    let nodes = ScriptedNodes::new(vec![NodeExecutionOutcome::Aborted]);
    let scheduler = scheduler(&kernel, &artifacts, &nodes);
    let done = scheduler.run(&id).await.unwrap();
    assert!(matches!(
        done.phase(),
        Some(Phase::Settled {
            result: Outcome::Aborted { .. }
        })
    ));
    assert!(artifacts.contexts.lock().unwrap().is_empty());
    assert_eq!(
        kernel.history(&id).unwrap()[0].stamp.principal().kind,
        PrincipalKind::Human
    );
    assert_eq!(scheduler.run(&id).await.unwrap(), done);
    assert_eq!(nodes.calls.lock().unwrap().len(), 1);
}
