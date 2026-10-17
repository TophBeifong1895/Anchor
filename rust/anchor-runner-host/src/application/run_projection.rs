//! Read-only Run presentation shared by HTTP and Pilot. These observations are
//! advisory; control still validates identity, scope and leases at write time.
use super::{ApplicationError, RunApplication, RunMetadata, control};
use anchor_runtime::graph::{GraphRunRecord, GraphSnapshot};
use serde::Serialize;
use serde_json::{Value, json};
use std::sync::atomic::Ordering;

#[derive(Debug, Serialize)]
pub(crate) struct RunActions {
    pub(crate) pause: bool,
    pub(crate) stop: bool,
    pub(crate) resume: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct RunProjection {
    pub(crate) phase: String,
    pub(crate) active: bool,
    pub(crate) control_requested: Option<&'static str>,
    pub(crate) terminal: bool,
    pub(crate) quiescent: bool,
    pub(crate) needs_attention: bool,
    pub(crate) actions: RunActions,
}

impl RunApplication {
    pub(crate) async fn project_run(
        &self,
        record: &GraphRunRecord,
        metadata: Option<&RunMetadata>,
    ) -> Result<RunProjection, ApplicationError> {
        if let Some(runs) = self.kernel_owner(&record.run_id)? {
            return Ok(runs.projection(
                &runs
                    .load(&record.run_id)?
                    .ok_or(ApplicationError::Missing)?,
            ));
        }
        let active = self.active.lock().await;
        let executor = active.get(&record.run_id);
        let requested = executor.and_then(|run| {
            if run.control.cancellation.load(Ordering::Relaxed) {
                Some("stop")
            } else if run.control.pause.load(Ordering::Relaxed) {
                Some("pause")
            } else {
                None
            }
        });
        let session_coordinated = metadata.is_some_and(|value| value.session_call.is_some());
        let mut projection = observed(record, executor.is_some(), requested, session_coordinated);
        let identity = metadata
            .filter(|metadata| control::validate_control_identity(record, metadata).is_ok());
        if !projection.active && !session_coordinated && identity.is_none() {
            projection.actions.stop = false;
            projection.actions.resume = false;
        }
        // The same immutable identity and lifecycle guards used by control.
        // Scope/lease races remain the final control use case's responsibility.
        if projection.actions.resume {
            projection.actions.resume = identity
                .is_some_and(|metadata| self.ensure_current_assistant(metadata).is_ok())
                && self.abandon_intent(&record.run_id)?.is_none()
                && self
                    .reject_superseded_conversation_execution(&record.run_id)
                    .is_ok();
        }
        Ok(projection)
    }
}

fn observed(
    record: &GraphRunRecord,
    active: bool,
    requested: Option<&'static str>,
    session_coordinated: bool,
) -> RunProjection {
    super::legacy_run_view::projection(
        &super::legacy_run_view::view(record, active),
        record,
        requested,
        session_coordinated,
    )
}

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
    use anchor_runtime::graph::RunStatus;

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
    fn legacy_projection_serialization_preserves_ready_paused_and_stopped_bytes() {
        let mut record = record();
        for (status, expected) in [
            (
                RunStatus::Ready,
                r#"{"phase":"ready","active":false,"control_requested":null,"terminal":false,"quiescent":false,"needs_attention":false,"actions":{"pause":false,"stop":true,"resume":true}}"#,
            ),
            (
                RunStatus::Paused,
                r#"{"phase":"paused","active":false,"control_requested":null,"terminal":false,"quiescent":false,"needs_attention":false,"actions":{"pause":false,"stop":true,"resume":true}}"#,
            ),
            (
                RunStatus::Stopped,
                r#"{"phase":"stopped","active":false,"control_requested":null,"terminal":false,"quiescent":true,"needs_attention":false,"actions":{"pause":false,"stop":true,"resume":true}}"#,
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
