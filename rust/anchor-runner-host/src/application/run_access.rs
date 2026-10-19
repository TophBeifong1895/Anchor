mod kernel_calls;
pub(super) mod kernel_runs;
pub(super) mod legacy_run_view;
mod run_json;
mod run_observations;
pub(super) mod run_view;
use super::{RunApplication, new_control, storage};
use anchor_kernel::{Outcome, Phase, Waiter};
mod abandon;
mod admission;
mod assistant_policy;
mod channels;
mod control;
mod conversations;
mod deletion;
mod dispatch;
mod graph_policy;
pub(crate) mod http;
#[cfg(test)]
mod liveness;
pub(super) mod projection;
mod reads;
mod recovery;
mod session_calls;
mod store;

pub(crate) use assistant_policy::{graph_lease, graph_read_lease};
#[cfg(test)]
pub(crate) use liveness::RunLiveness;
#[cfg(test)]
pub(super) use projection::observed;
pub(crate) use projection::projected_run_is_unfinished;
pub(crate) use reads::load;
use store::wait_child_ids;

use super::{ApplicationError, RunMetadata};
use anchor_runtime::graph::{GraphRunRecord, GraphSnapshot, PluginBinding, RunResult, RunStatus};
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
    pub(crate) results: &'record BTreeMap<String, Vec<RunResult>>,
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
    lifecycle: run_view::RunView,
    kernel_owned: bool,
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
            lifecycle: legacy_run_view::view(record, active),
            kernel_owned: false,
        }
    }

    pub(super) fn inactive(record: &'record GraphRunRecord) -> Self {
        Self::observed(record, false, None, false)
    }

    pub(crate) fn with_active(mut self, active: bool) -> Self {
        self.active = active;
        self.lifecycle.worker = active;
        self
    }

    pub(crate) fn phase(&self) -> RunPhase<'_> {
        match &self.lifecycle.phase {
            Phase::Executing => RunPhase::Executing,
            Phase::Waiting { waiter } => match waiter {
                Waiter::Human { reason } => RunPhase::Waiting {
                    for_what: RunWait::Human,
                    reason: Some(reason),
                },
                Waiter::Child { .. } => RunPhase::Waiting {
                    for_what: RunWait::Child,
                    reason: self.lifecycle.reason.as_deref(),
                },
                Waiter::Effect { .. } => RunPhase::Waiting {
                    for_what: RunWait::Effect,
                    reason: self.lifecycle.reason.as_deref(),
                },
            },
            Phase::Settled { result } => {
                let (outcome, reason) = match result {
                    Outcome::Done => (RunOutcome::Done, None),
                    Outcome::Failed { detail, .. } => (RunOutcome::Failed, Some(detail.as_str())),
                    Outcome::Escalated { detail } => (RunOutcome::Escalated, Some(detail.as_str())),
                    Outcome::Unknown { detail } => (RunOutcome::Unknown, Some(detail.as_str())),
                    Outcome::Aborted { detail } => (RunOutcome::Aborted, Some(detail.as_str())),
                };
                RunPhase::Settled { outcome, reason }
            }
        }
    }

    pub(super) fn from_kernel(
        record: GraphRunRecord,
        lifecycle: run_view::RunView,
    ) -> RunView<'static> {
        RunView {
            active: lifecycle.worker,
            record: Cow::Owned(record),
            requested: None,
            session_coordinated: false,
            lifecycle,
            kernel_owned: true,
        }
    }
    pub(super) fn owned(record: GraphRunRecord) -> RunView<'static> {
        let lifecycle = legacy_run_view::view(&record, false);
        RunView {
            record: Cow::Owned(record),
            active: false,
            requested: None,
            session_coordinated: false,
            lifecycle,
            kernel_owned: false,
        }
    }

    pub(crate) fn active(&self) -> bool {
        self.active
    }

    #[allow(
        dead_code,
        reason = "Required read facade contract; current HTTP timestamps retain filesystem metadata semantics"
    )]
    pub(crate) fn updated(&self) -> Option<&str> {
        self.lifecycle.updated.as_deref()
    }

    pub(crate) fn progress(&self) -> RunProgress<'_> {
        RunProgress {
            run_id: &self.record.run_id,
            graph_digest: &self.record.graph_digest,
            snapshot: &self.record.snapshot,
            results: &self.record.results,
            plugin_bindings: &self.record.plugin_bindings,
            invocations: &self.record.invocations,
            executed: self.record.executed_nodes(),
        }
    }

    pub(crate) fn is_settled(&self) -> bool {
        self.lifecycle.is_settled()
    }
    pub(crate) fn is_quiescent(&self) -> bool {
        !self.active
            && matches!(
                self.lifecycle.phase,
                Phase::Settled { .. }
                    | Phase::Waiting {
                        waiter: Waiter::Human { .. }
                    }
            )
    }
    #[allow(
        dead_code,
        reason = "Stable read facade predicate; the phase projection does not serialize a separate attention flag"
    )]
    pub(crate) fn needs_attention(&self) -> bool {
        matches!(
            self.phase(),
            RunPhase::Waiting { .. }
                | RunPhase::Settled {
                    outcome: RunOutcome::Failed
                        | RunOutcome::Escalated
                        | RunOutcome::Unknown
                        | RunOutcome::Aborted,
                    ..
                }
        ) || (!self.active && matches!(self.phase(), RunPhase::Executing))
    }
    pub(crate) fn can_pause(&self) -> bool {
        self.lifecycle.actions().pause && !self.session_coordinated
    }
    pub(crate) fn can_stop(&self) -> bool {
        self.lifecycle.actions().stop
    }
    pub(crate) fn can_resume(&self) -> bool {
        self.lifecycle.actions().resume && !self.session_coordinated
    }
    pub(crate) fn has_stopped_execution(&self) -> bool {
        if self.is_settled() {
            return true;
        }
        match &self.lifecycle.phase {
            Phase::Waiting {
                waiter: Waiter::Human { reason },
            } if self.kernel_owned => !matches!(reason.as_str(), "paused" | "budget"),
            _ if !self.kernel_owned => self.record.status == RunStatus::Stopped,
            _ => false,
        }
    }
    pub(crate) fn can_dispatch(&self) -> bool {
        if self.kernel_owned {
            self.lifecycle.phase == Phase::Executing
        } else {
            self.record.status == RunStatus::Ready
        }
    }

    pub(crate) fn waits_for_children(&self) -> bool {
        matches!(
            self.lifecycle.phase,
            Phase::Waiting {
                waiter: Waiter::Child { .. }
            }
        )
    }

    pub(crate) fn waits_for_invocation(&self, node: &str, invocation: u64) -> bool {
        if !self.waits_for_children() {
            return false;
        }
        if self.kernel_owned {
            let Phase::Waiting {
                waiter: Waiter::Child { task },
            } = &self.lifecycle.phase
            else {
                return false;
            };
            self.record.graph_calls.values().any(|call| {
                call.identity.node_id == node
                    && call.identity.invocation == invocation
                    && call
                        .child_run_id
                        .as_ref()
                        .is_some_and(|id| *task == format!("run/{id}"))
            })
        } else {
            self.record
                .cursor
                .as_ref()
                .is_some_and(|cursor| cursor.node_id == node && cursor.key.invocation == invocation)
        }
    }

    pub(crate) fn has_succeeded(&self) -> bool {
        matches!(
            self.phase(),
            RunPhase::Settled {
                outcome: RunOutcome::Done,
                ..
            }
        )
    }

    pub(crate) fn has_pending_recovery(&self) -> bool {
        self.record.status == RunStatus::WaitingRecovery && !self.record.recovery.is_empty()
    }

    pub(crate) fn occupies_conversation_execution(&self) -> bool {
        self.active || matches!(self.phase(), RunPhase::Executing)
    }

    pub(crate) fn can_abandon(&self) -> bool {
        self.lifecycle.actions().abandon
    }
}

impl<'record> From<&'record GraphRunRecord> for RunView<'record> {
    fn from(record: &'record GraphRunRecord) -> Self {
        Self::inactive(record)
    }
}

impl<'view> From<&'view RunView<'_>> for RunView<'view> {
    fn from(view: &'view RunView<'_>) -> Self {
        RunView {
            record: Cow::Borrowed(&view.record),
            active: view.active,
            requested: view.requested,
            session_coordinated: view.session_coordinated,
            lifecycle: view.lifecycle.clone(),
            kernel_owned: view.kernel_owned,
        }
    }
}

fn is_settled(status: RunStatus) -> bool {
    matches!(legacy_run_view::phase(status, None), Phase::Settled { .. })
}

#[cfg(test)]
fn is_quiescent(status: RunStatus, executor_registered: bool) -> bool {
    !executor_registered
        && matches!(
            legacy_run_view::phase(status, None),
            Phase::Settled { .. }
                | Phase::Waiting {
                    waiter: Waiter::Human { .. }
                }
        )
}

fn waits_for_children(status: RunStatus) -> bool {
    status == RunStatus::WaitingCall
}

fn succeeded(status: RunStatus) -> bool {
    status == RunStatus::Completed
}

fn validate_control_identity(
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

fn reject_terminal_control(record: &GraphRunRecord) -> Result<(), ApplicationError> {
    if is_settled(record.status) {
        return Err(ApplicationError::Conflict(
            "terminal Run cannot be controlled".into(),
        ));
    }
    Ok(())
}

fn validate_inactive_control(
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

fn validate_resume_budget(record: &GraphRunRecord) -> Result<(), ApplicationError> {
    if record.status == RunStatus::BudgetStopped {
        return Err(ApplicationError::Conflict(
            "budget-stopped Run requires an explicit budget policy".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
pub(crate) use abandon::AbandonResult;
