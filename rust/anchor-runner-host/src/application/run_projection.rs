//! Read-only Run presentation shared by HTTP and Pilot. These observations are
//! advisory; control still validates identity, scope and leases at write time.
#[cfg(test)]
use super::run_access::observed;
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
            (RunStatus::Paused, true, true, false, false),
            (RunStatus::WaitingCall, true, true, false, false),
            (RunStatus::WaitingRecovery, false, true, true, false),
            (RunStatus::BudgetStopped, false, true, true, false),
            (RunStatus::Stopped, true, true, false, true),
            (RunStatus::Completed, false, false, false, true),
            (RunStatus::Failed, false, false, true, true),
            (RunStatus::Aborted, false, false, true, true),
        ] {
            record.status = status;
            let projection = observed(&record, false, None, false);
            assert_eq!(projection.actions.resume, resume, "{status:?}");
            assert_eq!(projection.actions.stop, stop, "{status:?}");
            assert!(!projection.actions.pause);
            assert_eq!(projection.needs_attention, attention, "{status:?}");
            assert_eq!(projection.quiescent, quiescent, "{status:?}");
        }
    }

    #[test]
    fn pending_control_and_session_coordination_are_observed_once() {
        let mut record = record();
        record.status = RunStatus::Running;
        let projection = observed(&record, true, Some("pause"), false);
        assert_eq!(projection.phase, "pausing");
        assert!(!projection.actions.pause);
        assert!(projection.actions.stop);
        assert!(!projection.actions.resume);
        let projection = observed(&record, true, Some("stop"), false);
        assert_eq!(projection.phase, "stopping");
        assert!(!projection.actions.stop);
        let projection = observed(&record, false, None, true);
        assert!(!projection.actions.resume);
        assert!(projection.actions.stop);
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
