use super::*;
use anchor_kernel::{Authority, Effect, Kernel, MemoryStore, Principal};
use std::sync::Arc;

struct Allow;
impl Authority for Allow {
    fn permits(&self, _: &Principal, _: Action, _: &str) -> bool {
        true
    }
}
fn resource(parallel: bool) -> Resource {
    let snapshot = if parallel {
        super::differential::random_parallel_graph(0)
    } else {
        super::differential::random_graph(0)
    };
    run_resource(
        &GraphRunRecord::create_with_id(snapshot, Value::Null, "settle").unwrap(),
        u64::MAX,
    )
    .unwrap()
}
fn settle(resource: &Resource, slot: &str, kind: PrincipalKind, content: Value) -> Judgement {
    GraphValidator.judge(
        &JudgeCtx {
            resource,
            principal: &Principal::new("test", kind),
        },
        &Submission::new(&resource.id, resource.version, Action::Settle)
            .slot(slot)
            .content(content),
    )
}
fn complete(resource: &mut Resource, dispatch: &Dispatch) {
    let judged = GraphValidator.judge(
        &JudgeCtx {
            resource,
            principal: &Principal::new("host", PrincipalKind::Code),
        },
        &Submission::new(&resource.id, resource.version, Action::Write)
            .slot(dispatch.slot())
            .content(serde_json::json!({"summary":"done", "commit": {
                "id":dispatch.slot(), "node_id":dispatch.node.id, "invocation":dispatch.invocation
            }})),
    );
    assert!(judged.verdict.is_accepted(), "{judged:?}");
    resource.state = judged.state.unwrap();
}
#[test]
fn serial_failure_counts_start_without_results_or_edge_decisions() {
    let resource = resource(false);
    let before = record_of(&resource).unwrap();
    let dispatch = GraphValidator::frontier(&resource).unwrap().remove(0);
    let judged = settle(
        &resource,
        &dispatch.slot(),
        PrincipalKind::Code,
        serde_json::json!({"reason":"broken"}),
    );
    assert!(judged.verdict.is_accepted());
    let after: GraphRunRecord = serde_json::from_value(judged.state.unwrap()).unwrap();
    assert_eq!(
        after.invocations,
        BTreeMap::from([(dispatch.node.id.clone(), 1)])
    );
    assert_eq!(after.passes, after.invocations);
    assert_eq!(before.results, after.results);
    assert_eq!(before.decided, after.decided);
    assert_eq!(
        judged.phase,
        Some(Phase::Settled {
            result: Outcome::Failed {
                reason: Reason::Unclassified,
                detail: format!("{} failed: broken", dispatch.node.id)
            }
        })
    );
}
#[test]
fn failure_refuses_non_code_closed_slots_and_unknown_fields() {
    let resource = resource(false);
    let slot = GraphValidator::frontier(&resource).unwrap()[0].slot();
    for kind in [PrincipalKind::Agent, PrincipalKind::Human] {
        let judged = settle(
            &resource,
            &slot,
            kind,
            serde_json::json!({"reason":"broken"}),
        );
        assert_eq!(judged.verdict.effect, Effect::Refused);
        assert_eq!(judged.verdict.reason, Reason::Unauthorized);
    }
    assert_eq!(
        settle(
            &resource,
            "closed#1",
            PrincipalKind::Code,
            serde_json::json!({"reason":"broken"})
        )
        .verdict
        .reason,
        Reason::SlotClosed
    );
    for field in ["node_id", "invocation", "extra"] {
        let mut content = serde_json::json!({"reason":"broken"});
        content[field] = Value::Null;
        let judged = settle(&resource, &slot, PrincipalKind::Code, content);
        assert_eq!(judged.verdict.effect, Effect::Returned);
        assert_eq!(judged.verdict.reason, Reason::ContentInvalid);
    }
}
#[test]
fn fanout_join_and_call_cannot_be_settled_as_work() {
    let mut res = resource(true);
    loop {
        let dispatches = GraphValidator::frontier(&res).unwrap();
        for dispatch in dispatches {
            if dispatch.kind != DispatchKind::Work {
                assert_eq!(
                    settle(
                        &res,
                        &dispatch.slot(),
                        PrincipalKind::Code,
                        serde_json::json!({"reason":"broken"})
                    )
                    .verdict
                    .reason,
                    Reason::SlotClosed
                );
            }
            complete(&mut res, &dispatch);
        }
        if record_of(&res).unwrap().results.contains_key("j") {
            break;
        }
    }
    let mut record = record_of(&resource(false)).unwrap();
    let entry = record.snapshot.entry.clone();
    let node = record
        .snapshot
        .nodes
        .iter_mut()
        .find(|n| n.id == entry)
        .unwrap();
    node.agent = None;
    node.op = Some("call".into());
    record.snapshot.edges.retain(|e| e.from_node != entry);
    record.snapshot.ops.insert(
        "call".into(),
        serde_json::json!({"call":{"graph":"child", "mode":"wait"}}),
    );
    let res = run_resource(&record, u64::MAX).unwrap();
    let dispatch = GraphValidator::frontier(&res).unwrap().remove(0);
    assert_eq!(dispatch.kind, DispatchKind::Call);
    assert_eq!(
        settle(
            &res,
            &dispatch.slot(),
            PrincipalKind::Code,
            serde_json::json!({"reason":"broken"})
        )
        .verdict
        .reason,
        Reason::SlotClosed
    );
}
#[test]
fn parallel_failure_keeps_sibling_and_kernel_refuses_every_late_slot() {
    let mut res = resource(true);
    while record_of(&res).unwrap().parallel.is_none() {
        let dispatch = GraphValidator::frontier(&res).unwrap().remove(0);
        complete(&mut res, &dispatch);
    }
    loop {
        let dispatch = GraphValidator::frontier(&res).unwrap().remove(0);
        complete(&mut res, &dispatch);
        if record_of(&res).unwrap().parallel.as_ref().unwrap().branches[0].status
            == ParallelBranchStatus::Completed
        {
            break;
        }
    }
    let before = record_of(&res).unwrap();
    let dispatch = GraphValidator::frontier(&res).unwrap().remove(0);
    let mut kernel = Kernel::new(MemoryStore::default(), Box::new(Allow)).unwrap();
    kernel
        .install(GRAPH_VALIDATOR, Arc::new(GraphValidator))
        .unwrap();
    kernel.create(res.clone()).unwrap();
    let host = Principal::new("host", PrincipalKind::Code);
    assert!(
        kernel
            .submit(
                &host,
                0,
                Submission::new(&res.id, 0, Action::Settle)
                    .slot(dispatch.slot())
                    .content(serde_json::json!({"reason":"broken"}))
            )
            .unwrap()
            .is_accepted()
    );
    let after = kernel.load(&res.id).unwrap().unwrap();
    let record = record_of(&after).unwrap();
    assert_eq!(record.results, before.results);
    assert_eq!(record.decided, before.decided);
    let branch = &record.parallel.as_ref().unwrap().branches[dispatch.branch.unwrap()];
    assert_eq!(branch.status, ParallelBranchStatus::Failed);
    assert_eq!(branch.error.as_deref(), Some("broken"));
    assert!(!record.invocations.contains_key("j"));
    assert!(kernel.open(&res.id).unwrap().is_empty());
    for slot in [dispatch.slot(), "j#1".into(), "late-sibling#1".into()] {
        for kind in [PrincipalKind::Agent, PrincipalKind::Code] {
            assert_eq!(
                kernel
                    .submit(
                        &Principal::new("late", kind),
                        0,
                        Submission::new(&res.id, after.version, Action::Write).slot(slot.clone())
                    )
                    .unwrap()
                    .reason,
                Reason::TaskSettled
            );
        }
    }
    assert_eq!(
        kernel
            .history(&res.id)
            .unwrap()
            .iter()
            .filter(|r| r.verdict.reason == Reason::TaskSettled)
            .count(),
        6
    );
}
