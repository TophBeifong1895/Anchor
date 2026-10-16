//! K0.1: host costs and lifecycle facts remain kernel laws.

use std::sync::Arc;

use anchor_kernel::*;
use serde_json::json;

struct Grants;

impl Authority for Grants {
    fn permits(&self, principal: &Principal, _: Action, _: &str) -> bool {
        principal.id != "unauthorized"
    }
}

struct Work;

impl Validator for Work {
    fn judge(&self, ctx: &JudgeCtx, submission: &Submission) -> Judgement {
        match submission.content["result"].as_str() {
            Some("return") => Judgement::returned(Reason::ContentInvalid),
            Some("refuse") => Judgement::refused(Reason::ContentInvalid),
            Some("pending") => Judgement::pending(json!({"approved": true})),
            _ => {
                let mut state = ctx.resource.state.clone();
                state[submission.slot.as_deref().unwrap_or("work")] = json!(true);
                Judgement::accept(state, Verification::Mechanical)
            }
        }
    }
}

fn kernel(budget: u64) -> Kernel<MemoryStore> {
    let mut kernel = Kernel::new(MemoryStore::default(), Box::new(Grants)).unwrap();
    kernel.install("validator/work", Arc::new(Work)).unwrap();
    kernel
        .create(Resource::new("task/work", "validator/work", json!({})).task(budget))
        .unwrap();
    kernel
}

fn principal(kind: PrincipalKind) -> Principal {
    Principal::new(format!("{kind:?}"), kind)
}

fn work(seen: u64, result: &str) -> Submission {
    Submission::new("task/work", seen, Action::Write)
        .slot("a")
        .content(json!({"result": result}))
}

fn control(seen: u64, verb: &str) -> Submission {
    Submission::new("task/work", seen, Action::Request).content(json!({
        "control": verb,
        "reason": "unknown effect; inspect the existing attempt"
    }))
}

fn spent(kernel: &Kernel<MemoryStore>) -> u64 {
    kernel
        .load("task/work")
        .unwrap()
        .unwrap()
        .task
        .unwrap()
        .budget
        .spent
}

#[test]
fn returned_work_exhausts_budget_on_the_second_submission_without_cost_versions() {
    let kernel = kernel(10);
    let agent = principal(PrincipalKind::Agent);
    assert_eq!(
        kernel.submit(&agent, 6, work(0, "return")).unwrap().reason,
        Reason::ContentInvalid
    );
    assert_eq!(spent(&kernel), 6);
    assert_eq!(kernel.load("task/work").unwrap().unwrap().version, 0);
    assert_eq!(
        kernel.submit(&agent, 6, work(0, "return")).unwrap().reason,
        Reason::BudgetExhausted
    );
    let settled = kernel.load("task/work").unwrap().unwrap();
    assert_eq!(spent(&kernel), 12);
    assert_eq!(settled.version, 1);
    assert!(matches!(
        settled.phase(),
        Some(Phase::Settled {
            result: Outcome::Failed {
                reason: Reason::BudgetExhausted,
                ..
            }
        })
    ));
    assert_eq!(
        kernel.submit(&agent, 6, work(0, "return")).unwrap().reason,
        Reason::TaskSettled
    );
    assert_eq!(
        spent(&kernel),
        18,
        "even a settled task's refused record is charged"
    );
    assert_eq!(kernel.load("task/work").unwrap().unwrap().version, 1);
}

#[test]
fn concurrent_branches_charge_stale_work_once_and_retry_at_zero_cost() {
    let kernel = kernel(10);
    let agent = principal(PrincipalKind::Agent);
    let branch_a = work(0, "accept");
    let branch_b = Submission::new("task/work", 0, Action::Write).slot("b");
    assert!(kernel.submit(&agent, 3, branch_a).unwrap().is_accepted());
    assert_eq!(
        kernel.submit(&agent, 4, branch_b.clone()).unwrap().reason,
        Reason::StaleVersion
    );
    assert_eq!(spent(&kernel), 7);
    let mut retry = branch_b;
    retry.seen = kernel.load("task/work").unwrap().unwrap().version;
    assert_eq!(
        retry.seen, 1,
        "charging a stale branch must not invalidate the other version again"
    );
    assert!(kernel.submit(&agent, 0, retry).unwrap().is_accepted());
    assert_eq!(spent(&kernel), 7);
    let resource = kernel.load("task/work").unwrap().unwrap();
    assert_eq!(resource.state, json!({"a": true, "b": true}));
    assert_eq!(resource.version, 2);
    let costs: Vec<_> = kernel
        .history("task/work")
        .unwrap()
        .iter()
        .map(|r| r.stamp.cost())
        .collect();
    assert_eq!(costs, [3, 4, 0]);
}

#[test]
fn every_recorded_verdict_including_unauthorized_and_pending_costs_resources() {
    let kernel = kernel(100);
    let agent = principal(PrincipalKind::Agent);
    assert_eq!(
        kernel.submit(&agent, 2, work(0, "return")).unwrap().effect,
        Effect::Returned
    );
    assert_eq!(
        kernel.submit(&agent, 3, work(0, "refuse")).unwrap().effect,
        Effect::Refused
    );
    assert_eq!(
        kernel
            .submit(
                &Principal::new("unauthorized", PrincipalKind::Agent),
                4,
                work(0, "accept")
            )
            .unwrap()
            .reason,
        Reason::Unauthorized
    );
    assert_eq!(
        kernel.submit(&agent, 5, work(0, "pending")).unwrap().effect,
        Effect::Pending
    );
    assert_eq!(spent(&kernel), 14);
    assert_eq!(kernel.load("task/work").unwrap().unwrap().version, 0);
    assert_eq!(kernel.history("task/work").unwrap().len(), 4);
}

#[test]
fn unauthorized_record_can_exhaust_budget_without_authorizing_the_change() {
    let kernel = kernel(1);
    let verdict = kernel
        .submit(
            &Principal::new("unauthorized", PrincipalKind::Agent),
            2,
            work(0, "accept"),
        )
        .unwrap();
    assert!(!verdict.is_accepted());
    let resource = kernel.load("task/work").unwrap().unwrap();
    assert!(matches!(
        resource.phase(),
        Some(Phase::Settled {
            result: Outcome::Failed {
                reason: Reason::BudgetExhausted,
                ..
            }
        })
    ));
    assert_eq!(resource.state, json!({}));
    assert_eq!(spent(&kernel), 2);
}

#[test]
fn host_observation_can_stop_but_only_a_human_can_release_the_wait() {
    let kernel = kernel(100);
    let code = principal(PrincipalKind::Code);
    assert_eq!(
        kernel.submit(&code, 1, control(0, "stop")).unwrap().reason,
        Reason::EvidenceMissing
    );
    assert_eq!(
        kernel
            .submit(
                &code,
                1,
                control(0, "stop").evidence(Evidence::Resource {
                    id: "attempt/1".into(),
                    version: 0
                })
            )
            .unwrap()
            .reason,
        Reason::EvidenceMissing
    );
    let stop = control(0, "stop").evidence(Evidence::Observation {
        id: "attempt/1".into(),
    });
    assert!(kernel.submit(&code, 1, stop).unwrap().is_accepted());
    let waiting = kernel.load("task/work").unwrap().unwrap();
    assert!(
        matches!(waiting.phase(), Some(Phase::Waiting { waiter: Waiter::Human { reason } }) if reason.contains("unknown effect"))
    );
    assert_eq!(
        kernel
            .submit(&code, 1, control(1, "resume"))
            .unwrap()
            .reason,
        Reason::HumanOnly
    );
    assert_eq!(
        kernel.load("task/work").unwrap().unwrap().phase(),
        waiting.phase()
    );
    assert!(
        kernel
            .submit(&principal(PrincipalKind::Human), 0, control(1, "resume"))
            .unwrap()
            .is_accepted()
    );
    assert_eq!(spent(&kernel), 4);
}

#[test]
fn observed_unknown_effect_parks_the_task_without_a_human_impersonation() {
    let kernel = kernel(100);
    let stop = control(0, "stop").evidence(Evidence::Observation {
        id: "effect/unknown".into(),
    });
    assert!(
        kernel
            .submit(&principal(PrincipalKind::Code), 0, stop)
            .unwrap()
            .is_accepted()
    );
    let resource = kernel.load("task/work").unwrap().unwrap();
    assert!(matches!(
        resource.phase(),
        Some(Phase::Waiting {
            waiter: Waiter::Human { .. }
        })
    ));
    let history = kernel.history("task/work").unwrap();
    assert_eq!(history[0].stamp.principal().kind, PrincipalKind::Code);
}

#[test]
fn agent_cannot_stop_even_with_observation_evidence() {
    let kernel = kernel(100);
    let stop = control(0, "stop").evidence(Evidence::Observation {
        id: "claimed-stop".into(),
    });
    assert_eq!(
        kernel
            .submit(&principal(PrincipalKind::Agent), 1, stop)
            .unwrap()
            .reason,
        Reason::HumanOnly
    );
    assert_eq!(
        kernel.load("task/work").unwrap().unwrap().phase(),
        Some(&Phase::Executing)
    );
    assert_eq!(spent(&kernel), 1);
}

#[test]
fn waiting_rejects_non_lifecycle_work_for_every_identity_and_charges_it() {
    for kind in [
        PrincipalKind::Agent,
        PrincipalKind::Code,
        PrincipalKind::Human,
    ] {
        let kernel = kernel(100);
        assert!(
            kernel
                .submit(&principal(PrincipalKind::Human), 0, control(0, "stop"))
                .unwrap()
                .is_accepted()
        );
        let before = kernel.load("task/work").unwrap().unwrap();
        for action in [
            Action::Write,
            Action::Execute,
            Action::Settle,
            Action::Request,
        ] {
            let submission = Submission::new("task/work", 1, action)
                .slot("a")
                .content(json!({"result": "accept"}));
            assert_eq!(
                kernel
                    .submit(&principal(kind), 2, submission)
                    .unwrap()
                    .reason,
                Reason::TaskWaiting,
                "{kind:?} {action:?}"
            );
        }
        let after = kernel.load("task/work").unwrap().unwrap();
        assert_eq!(
            (after.version, &after.state, after.phase()),
            (before.version, &before.state, before.phase())
        );
        assert_eq!(spent(&kernel), 8, "{kind:?}");
        assert_eq!(kernel.history("task/work").unwrap().len(), 5);
    }
}

#[test]
fn waiting_work_that_exhausts_budget_settles_once_without_applying_work() {
    let kernel = kernel(1);
    assert!(
        kernel
            .submit(&principal(PrincipalKind::Human), 0, control(0, "stop"))
            .unwrap()
            .is_accepted()
    );
    assert_eq!(
        kernel
            .submit(&principal(PrincipalKind::Code), 2, work(1, "accept"))
            .unwrap()
            .reason,
        Reason::TaskWaiting,
        "waiting work is refused as TaskWaiting even when its cost settles the budget"
    );
    let resource = kernel.load("task/work").unwrap().unwrap();
    assert!(matches!(
        resource.phase(),
        Some(Phase::Settled {
            result: Outcome::Failed {
                reason: Reason::BudgetExhausted,
                ..
            }
        })
    ));
    assert_eq!(resource.state, json!({}));
    assert_eq!(spent(&kernel), 2);
}

#[test]
fn stopped_task_refuses_a_late_code_result_without_advancing_state() {
    let kernel = kernel(100);
    assert!(
        kernel
            .submit(&principal(PrincipalKind::Human), 0, control(0, "stop"))
            .unwrap()
            .is_accepted()
    );
    let before = kernel.load("task/work").unwrap().unwrap();
    assert_eq!(
        kernel
            .submit(&principal(PrincipalKind::Code), 0, work(1, "accept"))
            .unwrap()
            .reason,
        Reason::TaskWaiting
    );
    assert_eq!(kernel.load("task/work").unwrap().unwrap(), before);
    assert_eq!(kernel.history("task/work").unwrap().len(), 2);
}

#[test]
fn approval_observation_also_obeys_the_targets_history_budget() {
    let kernel = kernel(1);
    assert_eq!(
        kernel
            .submit(&principal(PrincipalKind::Agent), 1, work(0, "pending"))
            .unwrap()
            .effect,
        Effect::Pending
    );
    let pending = kernel
        .store()
        .list("pending/task/work/")
        .unwrap()
        .pop()
        .unwrap();
    let approval =
        Submission::new(pending, 0, Action::Write).content(json!({"decision": "approve"}));
    assert_eq!(
        kernel
            .submit(&principal(PrincipalKind::Human), 1, approval)
            .unwrap()
            .reason,
        Reason::BudgetExhausted
    );
    let resource = kernel.load("task/work").unwrap().unwrap();
    assert_eq!(resource.state, json!({}));
    assert_eq!(spent(&kernel), 2);
    assert!(matches!(
        resource.phase(),
        Some(Phase::Settled {
            result: Outcome::Failed {
                reason: Reason::BudgetExhausted,
                ..
            }
        })
    ));
}

#[test]
fn waiting_gate_refuses_even_unauthorized_or_stale_non_lifecycle_requests() {
    let kernel = kernel(100);
    assert!(
        kernel
            .submit(&principal(PrincipalKind::Human), 0, control(0, "stop"))
            .unwrap()
            .is_accepted()
    );
    let before = kernel.load("task/work").unwrap().unwrap();
    let unauthorized = Principal::new("unauthorized", PrincipalKind::Code);
    for submission in [
        work(0, "accept"),
        control(1, "unsupported"),
        Submission::new("task/work", 1, Action::Request),
    ] {
        assert_eq!(
            kernel.submit(&unauthorized, 1, submission).unwrap().reason,
            Reason::TaskWaiting
        );
    }
    let after = kernel.load("task/work").unwrap().unwrap();
    assert_eq!(
        (after.version, &after.state, after.phase()),
        (before.version, &before.state, before.phase())
    );
    assert_eq!(spent(&kernel), 3);
}
