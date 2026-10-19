//! Native quiescence uses phase and the Host's worker observation.
use super::run_view::RunView;
use anchor_kernel::{Phase, Waiter};
pub(crate) struct RunLiveness {
    pub(super) quiescent: bool,
}
impl RunLiveness {
    pub(crate) fn from_view(view: &RunView) -> Self {
        Self::from_phase(&view.phase, view.worker)
    }
    fn from_phase(phase: &Phase, worker: bool) -> Self {
        Self {
            quiescent: !worker
                && (matches!(phase, Phase::Settled { .. })
                    || matches!(
                        phase,
                        Phase::Waiting {
                            waiter: Waiter::Human { .. }
                        }
                    )),
        }
    }
    pub(crate) fn observed(status: anchor_runtime::graph::RunStatus, worker: bool) -> Self {
        Self::from_phase(&super::legacy_run_view::phase(status, None), worker)
    }
    pub(super) fn with_registry(
        record: &anchor_runtime::graph::GraphRunRecord,
        active: &std::collections::HashMap<String, super::ActiveRun>,
    ) -> Self {
        Self::observed(record.status, active.contains_key(&record.run_id))
    }
    pub(crate) fn is_quiescent(&self) -> bool {
        self.quiescent
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anchor_runtime::graph::RunStatus;

    #[test]
    fn parked_or_unregistered_ready_runs_are_not_quiescent() {
        for (status, inactive_quiescent) in [
            (RunStatus::Ready, false),
            (RunStatus::Running, false),
            (RunStatus::Paused, true),
            (RunStatus::BudgetStopped, true),
            (RunStatus::WaitingCall, false),
            (RunStatus::WaitingRecovery, false),
            (RunStatus::Completed, true),
            (RunStatus::Aborted, true),
            (RunStatus::Stopped, true),
            (RunStatus::Failed, true),
        ] {
            assert_eq!(
                RunLiveness::observed(status, false).is_quiescent(),
                inactive_quiescent,
                "unregistered {status:?}"
            );
            assert!(
                !RunLiveness::observed(status, true).is_quiescent(),
                "registered {status:?} still owns an executor"
            );
        }
    }
}
