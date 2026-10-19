//! Temporary, lossy legacy read adapter. Delete after the old Runner migration.
use super::run_view::RunView;
use anchor_kernel::{Outcome, Phase, Reason, Waiter};
use anchor_runtime::graph::{GraphRunRecord, RunStatus};

/// The only old lifecycle -> Phase mapping. No legacy display/action rules.
pub(in crate::application) fn phase(status: RunStatus, error: Option<&str>) -> Phase {
    let detail = error.unwrap_or_default().to_owned();
    match status {
        RunStatus::Ready | RunStatus::Running => Phase::Executing,
        RunStatus::Paused => Phase::Waiting {
            waiter: Waiter::Human {
                reason: "paused".into(),
            },
        },
        RunStatus::Stopped => Phase::Waiting {
            waiter: Waiter::Human {
                reason: error.unwrap_or("stopped").into(),
            },
        },
        RunStatus::BudgetStopped => Phase::Waiting {
            waiter: Waiter::Human {
                reason: "budget".into(),
            },
        },
        RunStatus::WaitingCall => Phase::Waiting {
            waiter: Waiter::Child {
                task: String::new(),
            },
        },
        RunStatus::WaitingRecovery => Phase::Waiting {
            waiter: Waiter::Effect {
                effect: String::new(),
            },
        },
        RunStatus::Completed => Phase::Settled {
            result: Outcome::Done,
        },
        RunStatus::Failed => Phase::Settled {
            result: Outcome::Failed {
                reason: Reason::Unclassified,
                detail,
            },
        },
        RunStatus::Aborted => Phase::Settled {
            result: Outcome::Aborted { detail },
        },
    }
}
pub(in crate::application) fn view(record: &GraphRunRecord, worker: bool) -> RunView {
    RunView {
        phase: phase(record.status, record.error.as_deref()),
        graph: record.into(),
        worker,
        updated: record.updated.clone(),
        reason: record.error.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::{run_access::run_view::GraphProgress, run_projection::RunProjection};
    #[test]
    fn every_legacy_status_maps_lossily_with_both_worker_observations() {
        for (status, phase_name, waiter, outcome, reason) in [
            (RunStatus::Ready, "executing", None, None, None),
            (RunStatus::Running, "executing", None, None, None),
            (
                RunStatus::Paused,
                "waiting",
                Some("human"),
                None,
                Some("paused"),
            ),
            (
                RunStatus::Stopped,
                "waiting",
                Some("human"),
                None,
                Some("explanation"),
            ),
            (
                RunStatus::BudgetStopped,
                "waiting",
                Some("human"),
                None,
                Some("budget"),
            ),
            (
                RunStatus::WaitingRecovery,
                "waiting",
                Some("effect"),
                None,
                Some("explanation"),
            ),
            (
                RunStatus::WaitingCall,
                "waiting",
                Some("child"),
                None,
                Some("explanation"),
            ),
            (RunStatus::Completed, "settled", None, Some("done"), None),
            (
                RunStatus::Failed,
                "settled",
                None,
                Some("failed"),
                Some("explanation"),
            ),
            (
                RunStatus::Aborted,
                "settled",
                None,
                Some("aborted"),
                Some("explanation"),
            ),
        ] {
            for worker in [false, true] {
                let view = RunView {
                    phase: phase(status, Some("explanation")),
                    graph: GraphProgress {
                        passes: Default::default(),
                    },
                    worker,
                    updated: None,
                    reason: Some("explanation".into()),
                };
                let projected = RunProjection::from_view(&view);
                assert_eq!(
                    (
                        projected.phase,
                        projected.waiting_for,
                        projected.outcome,
                        projected.reason.as_deref(),
                        projected.active
                    ),
                    (phase_name, waiter, outcome, reason, worker),
                    "{status:?}/{worker}"
                );
                assert_eq!(projected.actions.pause, phase_name == "executing" && worker);
                assert_eq!(projected.actions.stop, phase_name == "executing");
                assert_eq!(
                    projected.actions.resume,
                    !worker && (phase_name == "executing" || waiter == Some("human"))
                );
                assert_eq!(projected.actions.abandon, phase_name != "settled");
                let json = serde_json::to_value(projected).unwrap();
                assert_eq!(json.get("waiting_for").is_some(), waiter.is_some());
                assert_eq!(json.get("outcome").is_some(), outcome.is_some());
                assert!(json.get("status").is_none());
                assert_eq!(
                    json["actions"]["pause"],
                    phase_name == "executing" && worker
                );
            }
        }
        assert!(
            matches!(phase(RunStatus::Stopped, None), Phase::Waiting { waiter: Waiter::Human { reason } } if reason == "stopped")
        );
    }
}
