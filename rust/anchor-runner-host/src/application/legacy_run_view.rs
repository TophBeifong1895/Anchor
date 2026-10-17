//! Temporary legacy Run adapter. Delete after the old Runner/RunStore migration.
//! Status distinctions retained for compatibility are measured separately.
use super::run_projection::{RunActions, RunProjection};
use super::run_view::RunView;
use super::{ActiveRun, ApplicationError, RunApplication, RunLiveness, control};
use anchor_kernel::{Outcome, Phase, Reason, Waiter};
use anchor_runtime::graph::{GraphRunRecord, RunStatus};
use std::collections::HashMap;

pub(super) fn phase(record: &GraphRunRecord) -> Phase {
    let detail = record.error.clone().unwrap_or_default();
    match record.status {
        RunStatus::Ready | RunStatus::Running => Phase::Executing,
        RunStatus::Paused | RunStatus::Stopped | RunStatus::BudgetStopped => Phase::Waiting {
            waiter: Waiter::Human { reason: detail },
        },
        RunStatus::WaitingCall => Phase::Waiting {
            waiter: Waiter::Child {
                task: record
                    .graph_calls
                    .values()
                    .find_map(|c| c.child_run_id.clone())
                    .unwrap_or_default(),
            },
        },
        RunStatus::WaitingRecovery => Phase::Waiting {
            waiter: Waiter::Effect {
                effect: record.run_id.clone(),
            },
        },
        RunStatus::Completed => Phase::Settled {
            result: Outcome::Done,
        },
        RunStatus::Failed => Phase::Settled {
            result: Outcome::Failed {
                // Old records have no Kernel reason code; do not invent one.
                reason: Reason::Unclassified,
                detail,
            },
        },
        RunStatus::Aborted => Phase::Settled {
            result: Outcome::Aborted { detail },
        },
    }
}
pub(super) fn view(record: &GraphRunRecord, worker: bool) -> RunView {
    RunView {
        phase: phase(record),
        graph: record.into(),
        worker,
        updated: record.updated.clone(),
    }
}
pub(super) fn quiescent(status: RunStatus, worker: bool) -> bool {
    !worker && (status.is_terminal() || status == RunStatus::Stopped)
}
// Old callers keep their synchronization protocol until their paths migrate.
impl RunLiveness {
    pub(crate) fn observed(status: RunStatus, worker: bool) -> Self {
        Self {
            quiescent: quiescent(status, worker),
        }
    }
    pub(super) fn with_registry(
        record: &GraphRunRecord,
        active: &HashMap<String, ActiveRun>,
    ) -> Self {
        Self::observed(record.status, active.contains_key(&record.run_id))
    }
}
pub(super) fn projection(
    view: &RunView,
    record: &GraphRunRecord,
    requested: Option<&'static str>,
    session_coordinated: bool,
) -> RunProjection {
    let active = view.worker;
    let terminal = view.is_settled();
    let phase = match (active, requested, record.status) {
        (true, Some("stop"), _) => "stopping".into(),
        (true, Some("pause"), _) => "pausing".into(),
        (false, _, RunStatus::Running) => "waiting_resume".into(),
        _ => serde_json::to_value(record.status)
            .expect("RunStatus is serializable")
            .as_str()
            .expect("RunStatus serializes as a string")
            .to_owned(),
    };
    let resume = !active
        && !session_coordinated
        && control::validate_resume_budget(record).is_ok()
        && (control::validate_inactive_control(record, "resume").is_ok()
            || (record.status == RunStatus::WaitingRecovery && !record.recovery.is_empty()));
    RunProjection {
        needs_attention: matches!(
            phase.as_str(),
            "failed" | "aborted" | "waiting_resume" | "waiting_recovery" | "budget_stopped"
        ),
        phase,
        active,
        control_requested: requested,
        terminal,
        quiescent: quiescent(record.status, active),
        actions: RunActions {
            pause: active && !terminal && !session_coordinated && requested.is_none(),
            stop: if active {
                requested != Some("stop")
            } else {
                control::validate_inactive_control(record, "stop").is_ok()
            },
            resume,
        },
    }
}

impl RunApplication {
    pub(crate) fn settle_channel_run(
        &self,
        run_id: &str,
        status: RunStatus,
    ) -> Result<(), ApplicationError> {
        use anchor_platform_session::TurnStatus;
        let Some(metadata) = self.metadata(run_id)? else {
            return Ok(());
        };
        let turn_status = match status {
            RunStatus::Completed if metadata.assistant.is_none() => TurnStatus::Completed,
            RunStatus::Failed | RunStatus::Aborted => TurnStatus::Failed,
            RunStatus::Stopped => TurnStatus::Stopped,
            _ => return Ok(()),
        };
        self.settle_channel_turn(run_id, turn_status)
    }
}
