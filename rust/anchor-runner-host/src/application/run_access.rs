mod assistant_policy;
mod channels;
pub(crate) mod http;
mod liveness;
mod projection;
mod reads;

pub(crate) use assistant_policy::{graph_lease, graph_read_lease};
pub(crate) use liveness::RunLiveness;
#[cfg(test)]
pub(super) use projection::observed;
pub(crate) use reads::load;

use super::{ApplicationError, RunMetadata};
use anchor_runtime::graph::{GraphRunRecord, GraphSnapshot, PluginBinding, RunStatus};
use std::{borrow::Cow, collections::BTreeMap};

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

#[derive(Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RunOutcome {
    Done,
    Failed,
    Escalated,
    Unknown,
    Aborted,
}

pub(crate) struct RunProgress<'record> {
    pub(crate) run_id: &'record str,
    pub(crate) graph_digest: &'record str,
    pub(crate) snapshot: &'record GraphSnapshot,
    pub(crate) plugin_bindings: &'record BTreeMap<String, PluginBinding>,
    pub(crate) invocations: &'record BTreeMap<String, u64>,
    executed: Vec<String>,
}

impl RunProgress<'_> {
    pub(crate) fn executed_nodes(&self) -> &[String] {
        &self.executed
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RunView<'record> {
    record: Cow<'record, GraphRunRecord>,
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
            record: Cow::Borrowed(record),
            active,
            requested,
            session_coordinated,
        }
    }

    pub(super) fn inactive(record: &'record GraphRunRecord) -> Self {
        Self::observed(record, false, None, false)
    }

    pub(crate) fn with_active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    pub(crate) fn phase(&self) -> RunPhase<'_> {
        phase(self.record.status, self.record.error.as_deref())
    }

    pub(crate) fn active(&self) -> bool {
        self.active
    }

    pub(crate) fn progress(&self) -> RunProgress<'_> {
        RunProgress {
            run_id: &self.record.run_id,
            graph_digest: &self.record.graph_digest,
            snapshot: &self.record.snapshot,
            plugin_bindings: &self.record.plugin_bindings,
            invocations: &self.record.invocations,
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
            validate_inactive_control(&self.record, "stop").is_ok()
        }
    }

    pub(crate) fn can_resume(&self) -> bool {
        !self.active
            && !self.session_coordinated
            && validate_resume_budget(&self.record).is_ok()
            && (validate_inactive_control(&self.record, "resume").is_ok()
                || self.has_pending_recovery())
    }

    pub(crate) fn has_stopped_execution(&self) -> bool {
        has_stopped_execution(self.record.status)
    }

    pub(crate) fn can_dispatch(&self) -> bool {
        self.record.status == RunStatus::Ready
    }

    pub(crate) fn waits_for_children(&self) -> bool {
        self.record.status == RunStatus::WaitingCall
    }

    pub(crate) fn waits_for_invocation(&self, node: &str, invocation: u64) -> bool {
        self.waits_for_children()
            && self
                .record
                .cursor
                .as_ref()
                .is_some_and(|cursor| cursor.node_id == node && cursor.key.invocation == invocation)
    }

    pub(crate) fn has_succeeded(&self) -> bool {
        succeeded(self.record.status)
    }

    pub(crate) fn has_pending_recovery(&self) -> bool {
        self.record.status == RunStatus::WaitingRecovery && !self.record.recovery.is_empty()
    }

    pub(crate) fn occupies_conversation_execution(&self) -> bool {
        self.active || matches!(self.record.status, RunStatus::Ready | RunStatus::Running)
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

impl<'record> From<&'record GraphRunRecord> for RunView<'record> {
    fn from(record: &'record GraphRunRecord) -> Self {
        Self::inactive(record)
    }
}

impl<'view> From<&'view RunView<'_>> for RunView<'view> {
    fn from(view: &'view RunView<'_>) -> Self {
        Self::observed(
            &view.record,
            view.active,
            view.requested,
            view.session_coordinated,
        )
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
    !executor_registered && has_stopped_execution(status)
}

fn has_stopped_execution(status: RunStatus) -> bool {
    is_settled(status) || is_stopped(status)
}

fn is_stopped(status: RunStatus) -> bool {
    status == RunStatus::Stopped
}

pub(super) fn succeeded(status: RunStatus) -> bool {
    status == RunStatus::Completed
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
