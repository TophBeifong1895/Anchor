//! Read-only Run presentation shared by HTTP and Pilot. These observations are
//! advisory; control still validates identity, scope and leases at write time.
#[cfg(test)]
use super::run_access::observed;
#[cfg(test)]
pub(crate) use super::run_access::projection::RunProjection;
use anchor_runtime::graph::GraphSnapshot;
use serde_json::{Value, json};

/// Canvas data comes exclusively from the compiled, frozen snapshot, including
/// expanded inline modules. Do not expose execution input, scripts or host
/// parameters as part of this display projection, or consult the live Graph.
pub(crate) fn run_definition(snapshot: &GraphSnapshot) -> Value {
    let ops = snapshot
        .ops
        .iter()
        .map(|(name, op)| {
            let display = if let Some(call) = op.get("call") {
                json!({"call":{"graph":call["graph"],"mode":call["mode"]}})
            } else if let Some(fanout) = op.get("fanout") {
                json!({"fanout":fanout})
            } else if op.get("join").is_some() {
                json!({"join":{}})
            } else if let Some(host) = op.get("host") {
                json!({"host":{"operation":host["operation"]}})
            } else {
                json!({"run":""})
            };
            (name.clone(), display)
        })
        .collect::<serde_json::Map<_, _>>();
    json!({
        "entry":snapshot.entry,
        "nodes":snapshot.nodes.iter().map(|node| {
            let mut display = json!({"id":node.id,"plugins":node.plugins});
            if let Some(agent) = &node.agent {
                display["agent"] = json!(agent);
            }
            if let Some(op) = &node.op {
                display["op"] = json!(op);
            }
            display
        }).collect::<Vec<_>>(),
        "edges":snapshot.edges,
        "ops":ops,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use anchor_runtime::graph::{GraphRunRecord, RunStatus};

    fn record() -> GraphRunRecord {
        GraphRunRecord::create_with_id(
            serde_json::from_value(json!({
                "objective":"test", "entry":"worker",
                "agents":{"role":{"model":"fixture","instructions":"work"}},
                "nodes":[{"id":"worker","agent":"role"}], "edges":[]
            }))
            .unwrap(),
            json!({}),
            "projection-test",
        )
        .unwrap()
    }

    #[test]
    fn inactive_states_reuse_control_guards_and_liveness() {
        let mut record = record();
        for (status, resume, stop, attention, quiescent) in [
            (RunStatus::Ready, true, true, false, false),
            (RunStatus::Running, true, true, true, false),
            (RunStatus::Paused, true, false, true, true),
            (RunStatus::WaitingCall, false, false, true, false),
            (RunStatus::WaitingRecovery, false, false, true, false),
            (RunStatus::BudgetStopped, true, false, true, true),
            (RunStatus::Stopped, true, false, true, true),
            (RunStatus::Completed, false, false, false, true),
            (RunStatus::Failed, false, false, true, true),
            (RunStatus::Aborted, false, false, true, true),
        ] {
            record.status = status;
            let projection = observed(&record, false, None, false);
            assert!(!projection.actions.pause, "{status:?}");
            assert_eq!(projection.actions.resume, resume, "{status:?}");
            assert_eq!(projection.actions.stop, stop, "{status:?}");
            assert_eq!(
                projection.actions.abandon,
                !matches!(
                    status,
                    RunStatus::Completed | RunStatus::Failed | RunStatus::Aborted
                )
            );
            assert_eq!(
                projection.phase == "waiting"
                    || matches!(projection.outcome, Some("failed" | "aborted"))
                    || (projection.phase == "executing" && !projection.active),
                attention || status == RunStatus::Ready,
                "{status:?}"
            );
            assert_eq!(
                super::super::RunLiveness::observed(status, false).is_quiescent(),
                quiescent,
                "{status:?}"
            );
        }
    }

    #[test]
    fn pending_control_and_session_coordination_are_observed_once() {
        let mut record = record();
        record.status = RunStatus::Running;
        let projection = observed(&record, true, Some("pause"), false);
        assert_eq!(projection.phase, "executing");
        assert!(projection.actions.abandon);
        assert!(projection.actions.pause);
        assert!(projection.actions.stop);
        assert!(!projection.actions.resume);
        let projection = observed(&record, true, Some("stop"), false);
        assert_eq!(projection.phase, "executing");
        assert!(projection.actions.stop);
        let projection = observed(&record, true, None, true);
        assert!(!projection.actions.pause);
        let projection = observed(&record, false, None, true);
        assert!(!projection.actions.resume);
        assert!(projection.actions.stop);
    }

    #[test]
    fn legacy_projection_serialization_preserves_ready_paused_and_stopped_bytes() {
        let mut record = record();
        record.updated = None;
        for (status, expected) in [
            (
                RunStatus::Ready,
                r#"{"phase":"executing","reason":null,"active":false,"actions":{"pause":false,"stop":true,"resume":true,"abandon":true},"updated":null}"#,
            ),
            (
                RunStatus::Paused,
                r#"{"phase":"waiting","waiting_for":"human","reason":"paused","active":false,"actions":{"pause":false,"stop":false,"resume":true,"abandon":true},"updated":null}"#,
            ),
            (
                RunStatus::Stopped,
                r#"{"phase":"waiting","waiting_for":"human","reason":"stopped","active":false,"actions":{"pause":false,"stop":false,"resume":true,"abandon":true},"updated":null}"#,
            ),
        ] {
            record.status = status;
            assert_eq!(
                serde_json::to_vec(&observed(&record, false, None, false)).unwrap(),
                expected.as_bytes(),
                "{status:?}"
            );
        }
    }

    #[test]
    fn frozen_canvas_projects_expanded_modules_and_hides_execution_input() {
        let snapshot = serde_json::from_value(json!({
            "objective":"private objective", "input":{"secret":"secret-input"},
            "entry":"module.worker", "agents":{"module.role":{"model":"fixture","instructions":"private instructions"}},
            "ops":{"call":{"call":{"graph":"target","mode":"wait","input":{"secret":"private-call-input"}}},"finish":{"run":"private script"}},
            "nodes":[{"id":"module.worker","agent":"module.role","with":{"private":"value"}}, {"id":"call","op":"call"}, {"id":"finish","op":"finish"}],
            "edges":[{"from":"module.worker","to":"call"},{"from":"call","to":"finish"}]
        })).unwrap();
        let definition = run_definition(&snapshot);
        assert_eq!(definition["entry"], "module.worker");
        assert_eq!(definition["nodes"][0]["id"], "module.worker");
        assert_eq!(definition["edges"][0]["from"], "module.worker");
        assert_eq!(
            definition["ops"]["call"]["call"],
            json!({"graph":"target","mode":"wait"})
        );
        assert!(!definition.to_string().contains("private"));
        assert!(definition.get("graphs").is_none());
    }
}
