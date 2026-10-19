//! Combine durable status with the Host's executor registration observation.
pub(crate) use super::run_access::RunLiveness;

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
