//! Internal record bridge for callers not yet migrated to RunView.
//! Never serialized as the external lifecycle or saved back to either store.
use super::ApplicationError;
use anchor_kernel::{Outcome, Phase, Resource, Waiter};
use anchor_runtime::graph::{GraphRunRecord, RunStatus, validator::record_of};
pub(super) fn record(resource: &Resource) -> Result<GraphRunRecord, ApplicationError> {
    let mut record = record_of(resource)?;
    // JSON/legacy-consumer compatibility copy. Never save this value.
    let (status, error) = status_and_error(
        resource
            .phase()
            .ok_or_else(|| ApplicationError::Storage("Kernel Run has no phase".into()))?,
    );
    record.status = status;
    record.error = error;
    Ok(record)
}

fn status_and_error(phase: &Phase) -> (RunStatus, Option<String>) {
    match phase {
        Phase::Executing => (RunStatus::Running, None),
        Phase::Waiting {
            waiter: Waiter::Human { reason },
        } => (RunStatus::Stopped, Some(reason.clone())),
        Phase::Waiting { .. } => (RunStatus::WaitingCall, None),
        Phase::Settled {
            result: Outcome::Done,
        } => (RunStatus::Completed, None),
        Phase::Settled {
            result: Outcome::Aborted { detail },
        } => (RunStatus::Aborted, Some(detail.clone())),
        Phase::Settled {
            result: Outcome::Failed { detail, .. } | Outcome::Unknown { detail },
        } => (RunStatus::Failed, Some(detail.clone())),
        Phase::Settled {
            result: Outcome::Escalated { detail },
        } => (RunStatus::BudgetStopped, Some(detail.clone())),
    }
}
