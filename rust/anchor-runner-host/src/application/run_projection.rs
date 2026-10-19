//! One external lifecycle vocabulary for HTTP, Pilot and Web.
use super::{ApplicationError, RunApplication, RunMetadata, control, run_view::RunView};
use anchor_kernel::{Outcome, Phase, Waiter};
use anchor_runtime::graph::{GraphRunRecord, GraphSnapshot};
use serde::Serialize;
use serde_json::{Value, json};

pub(crate) use super::run_view::AvailableActions as RunActions;
#[derive(Debug, Serialize)]
pub(crate) struct RunProjection {
    pub(crate) phase: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) waiting_for: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) outcome: Option<&'static str>,
    pub(crate) reason: Option<String>,
    pub(crate) active: bool,
    pub(crate) actions: RunActions,
    pub(crate) updated: Option<String>,
}
impl RunProjection {
    pub(crate) fn from_view(view: &RunView) -> Self {
        let (phase, waiting_for, outcome, reason) = match &view.phase {
            Phase::Executing => ("executing", None, None, None),
            Phase::Waiting {
                waiter: Waiter::Human { reason },
            } => ("waiting", Some("human"), None, Some(reason.clone())),
            Phase::Waiting {
                waiter: Waiter::Child { .. },
            } => ("waiting", Some("child"), None, view.reason.clone()),
            Phase::Waiting {
                waiter: Waiter::Effect { .. },
            } => ("waiting", Some("effect"), None, view.reason.clone()),
            Phase::Settled {
                result: Outcome::Done,
            } => ("settled", None, Some("done"), None),
            Phase::Settled {
                result: Outcome::Failed { detail, .. },
            } => ("settled", None, Some("failed"), Some(detail.clone())),
            Phase::Settled {
                result: Outcome::Escalated { detail },
            } => ("settled", None, Some("escalated"), Some(detail.clone())),
            Phase::Settled {
                result: Outcome::Unknown { detail },
            } => ("settled", None, Some("unknown"), Some(detail.clone())),
            Phase::Settled {
                result: Outcome::Aborted { detail },
            } => ("settled", None, Some("aborted"), Some(detail.clone())),
        };
        Self {
            phase,
            waiting_for,
            outcome,
            reason,
            active: view.worker,
            actions: view.actions(),
            updated: view.updated.clone(),
        }
    }
}
impl RunApplication {
    pub(crate) async fn project_run(
        &self,
        record: &GraphRunRecord,
        metadata: Option<&RunMetadata>,
    ) -> Result<RunProjection, ApplicationError> {
        let mut view = self.read_view(record, self.run_is_active(&record.run_id).await)?;
        view.updated = self.run_updated(&record.run_id)?;
        let mut projection = RunProjection::from_view(&view);
        // Scope and identity narrow advisory actions; writes still revalidate.
        let identity = metadata.filter(|m| control::validate_control_identity(record, m).is_ok());
        let coordinated = metadata.is_some_and(|m| m.session_call.is_some());
        if !view.worker && !coordinated && identity.is_none() {
            projection.actions.stop = false;
            projection.actions.resume = false;
            projection.actions.abandon = false;
        }
        projection.actions.resume = projection.actions.resume
            && !coordinated
            && identity.is_some_and(|m| self.ensure_current_assistant(m).is_ok())
            && self.abandon_intent(&record.run_id)?.is_none()
            && self
                .reject_superseded_conversation_execution(&record.run_id)
                .is_ok();
        Ok(projection)
    }
}

#[cfg(test)]
fn observed(
    record: &GraphRunRecord,
    active: bool,
    _requested: Option<&'static str>,
    session_coordinated: bool,
) -> RunProjection {
    let mut projection = RunProjection::from_view(&super::legacy_run_view::view(record, active));
    projection.actions.resume = projection.actions.resume && !session_coordinated;
    projection
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
        assert!(projection.actions.stop);
        assert!(!projection.actions.resume);
        let projection = observed(&record, true, Some("stop"), false);
        assert_eq!(projection.phase, "executing");
        assert!(projection.actions.stop);
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
                r#"{"phase":"executing","reason":null,"active":false,"actions":{"stop":true,"resume":true,"abandon":true},"updated":null}"#,
            ),
            (
                RunStatus::Paused,
                r#"{"phase":"waiting","waiting_for":"human","reason":"paused","active":false,"actions":{"stop":false,"resume":true,"abandon":true},"updated":null}"#,
            ),
            (
                RunStatus::Stopped,
                r#"{"phase":"waiting","waiting_for":"human","reason":"stopped","active":false,"actions":{"stop":false,"resume":true,"abandon":true},"updated":null}"#,
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
