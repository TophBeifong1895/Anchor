mod liveness;
mod projection;

pub(crate) use liveness::RunLiveness;
#[cfg(test)]
pub(super) use projection::observed;

use super::{ApplicationError, RunMetadata};
use anchor_runtime::graph::{GraphRunRecord, GraphSnapshot, RunStatus};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RunPhase<'record> {
    Executing,
    Waiting {
        for_what: RunWait,
        reason: Option<&'record str>,
    },
    Settled {
        outcome: RunOutcome,
        reason: Option<&'record str>,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RunWait {
    Human,
    Child,
    Effect,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RunOutcome {
    Done,
    Failed,
    Aborted,
}

pub(crate) struct RunProgress<'record> {
    pub(crate) snapshot: &'record GraphSnapshot,
    executed: Vec<String>,
}

impl RunProgress<'_> {
    pub(crate) fn executed_nodes(&self) -> &[String] {
        &self.executed
    }
}

pub(crate) struct RunView<'record> {
    record: &'record GraphRunRecord,
    active: bool,
    requested: Option<&'static str>,
    session_coordinated: bool,
}

impl<'record> RunView<'record> {
    pub(super) fn observed(
        record: &'record GraphRunRecord,
        active: bool,
        requested: Option<&'static str>,
        session_coordinated: bool,
    ) -> Self {
        Self {
            record,
            active,
            requested,
            session_coordinated,
        }
    }

    pub(super) fn inactive(record: &'record GraphRunRecord) -> Self {
        Self::observed(record, false, None, false)
    }

    pub(crate) fn phase(&self) -> RunPhase<'record> {
        phase(self.record.status, self.record.error.as_deref())
    }

    pub(crate) fn active(&self) -> bool {
        self.active
    }

    pub(crate) fn progress(&self) -> RunProgress<'record> {
        RunProgress {
            snapshot: &self.record.snapshot,
            executed: self.record.executed_nodes(),
        }
    }

    pub(crate) fn is_settled(&self) -> bool {
        is_settled(self.record.status)
    }

    pub(crate) fn is_quiescent(&self) -> bool {
        is_quiescent(self.record.status, self.active)
    }

    pub(crate) fn needs_attention(&self) -> bool {
        matches!(
            self.legacy_phase(),
            "failed" | "aborted" | "waiting_resume" | "waiting_recovery" | "budget_stopped"
        )
    }

    pub(crate) fn can_pause(&self) -> bool {
        self.active && !self.is_settled() && !self.session_coordinated && self.requested.is_none()
    }

    pub(crate) fn can_stop(&self) -> bool {
        if self.active {
            self.requested != Some("stop")
        } else {
            validate_inactive_control(self.record, "stop").is_ok()
        }
    }

    pub(crate) fn can_resume(&self) -> bool {
        !self.active
            && !self.session_coordinated
            && validate_resume_budget(self.record).is_ok()
            && (validate_inactive_control(self.record, "resume").is_ok()
                || (self.record.status == RunStatus::WaitingRecovery
                    && !self.record.recovery.is_empty()))
    }

    pub(crate) fn can_abandon(&self) -> bool {
        !matches!(
            self.phase(),
            RunPhase::Settled {
                outcome: RunOutcome::Done | RunOutcome::Failed,
                ..
            }
        )
    }

    fn legacy_phase(&self) -> &'static str {
        match (self.active, self.requested, self.record.status) {
            (true, Some("stop"), _) => "stopping",
            (true, Some("pause"), _) => "pausing",
            (false, _, RunStatus::Running) => "waiting_resume",
            _ => legacy_status(self.record.status),
        }
    }
}

fn phase(status: RunStatus, reason: Option<&str>) -> RunPhase<'_> {
    match status {
        RunStatus::Ready | RunStatus::Running => RunPhase::Executing,
        RunStatus::Paused | RunStatus::Stopped | RunStatus::BudgetStopped => RunPhase::Waiting {
            for_what: RunWait::Human,
            reason,
        },
        RunStatus::WaitingCall => RunPhase::Waiting {
            for_what: RunWait::Child,
            reason,
        },
        RunStatus::WaitingRecovery => RunPhase::Waiting {
            for_what: RunWait::Effect,
            reason,
        },
        RunStatus::Completed => RunPhase::Settled {
            outcome: RunOutcome::Done,
            reason,
        },
        RunStatus::Failed => RunPhase::Settled {
            outcome: RunOutcome::Failed,
            reason,
        },
        RunStatus::Aborted => RunPhase::Settled {
            outcome: RunOutcome::Aborted,
            reason,
        },
    }
}

fn is_settled(status: RunStatus) -> bool {
    status.is_terminal()
}

fn is_quiescent(status: RunStatus, executor_registered: bool) -> bool {
    !executor_registered && (is_settled(status) || status == RunStatus::Stopped)
}

pub(crate) fn legacy_status(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Ready => "ready",
        RunStatus::Running => "running",
        RunStatus::Paused => "paused",
        RunStatus::BudgetStopped => "budget_stopped",
        RunStatus::WaitingCall => "waiting_call",
        RunStatus::WaitingRecovery => "waiting_recovery",
        RunStatus::Completed => "completed",
        RunStatus::Aborted => "aborted",
        RunStatus::Stopped => "stopped",
        RunStatus::Failed => "failed",
    }
}

pub(super) fn validate_control_identity(
    record: &GraphRunRecord,
    metadata: &RunMetadata,
) -> Result<(), ApplicationError> {
    if metadata.graph_digest != record.graph_digest {
        return Err(ApplicationError::Invalid(
            "Run Graph identity metadata does not match its frozen snapshot".into(),
        ));
    }
    Ok(())
}

pub(super) fn reject_terminal_control(record: &GraphRunRecord) -> Result<(), ApplicationError> {
    if is_settled(record.status) {
        return Err(ApplicationError::Conflict(
            "terminal Run cannot be controlled".into(),
        ));
    }
    Ok(())
}

pub(super) fn validate_inactive_control(
    record: &GraphRunRecord,
    operation: &str,
) -> Result<(), ApplicationError> {
    if record.status == RunStatus::WaitingRecovery && operation != "stop" {
        return Err(ApplicationError::Conflict(
            "Run recovery context is not ready".into(),
        ));
    }
    reject_terminal_control(record)?;
    if operation == "pause" {
        return Err(ApplicationError::Conflict("that Run is not active".into()));
    }
    Ok(())
}

pub(super) fn validate_resume_budget(record: &GraphRunRecord) -> Result<(), ApplicationError> {
    if record.status == RunStatus::BudgetStopped {
        return Err(ApplicationError::Conflict(
            "budget-stopped Run requires an explicit budget policy".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
