//! Existing HTTP status spellings, kept outside the Run read model.
use super::{
    ApplicationError,
    run_projection::{RunActions, RunProjection},
    run_view::RunView,
};
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

/// Native read projection; lifecycle actions come from the read model.
pub(super) fn projection(view: &RunView) -> RunProjection {
    let actions = view.actions();
    let phase = if actions.stop && !view.worker {
        "waiting_resume".to_owned()
    } else {
        serde_json::to_value(status_and_error(&view.phase).0)
            .expect("status")
            .as_str()
            .expect("status string")
            .to_owned()
    };
    let needs_attention = matches!(view.phase, Phase::Waiting { .. })
        || (actions.stop && !view.worker)
        || phase == "failed";
    RunProjection {
        phase,
        active: view.worker,
        control_requested: None,
        terminal: view.is_settled(),
        // Preserve P's existing JSON observation while RunView exposes lifecycle actions.
        quiescent: !view.worker,
        needs_attention,
        actions: RunActions {
            pause: false,
            stop: actions.stop,
            // Existing API also exposes explicit restart of an orphan Executing Run.
            resume: actions.resume || (actions.stop && !view.worker),
        },
    }
}
