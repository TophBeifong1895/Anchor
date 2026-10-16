//! Combine durable status with the Host's executor registration observation.
use super::ActiveRun;
use anchor_runtime::graph::{GraphRunRecord, RunStatus};
use std::collections::HashMap;

/// Registration includes startup, cleanup and parked input waits; it does not
/// mean a model is busy. This observation neither acquires a lock nor freezes
/// Runner writes. Callers retain their existing registry/lease protocol.
pub(crate) struct RunLiveness {
    status: RunStatus,
    executor_registered: bool,
}

impl RunLiveness {
    /// Use the caller's existing observations, including API polling snapshots.
    pub(crate) fn observed(status: RunStatus, executor_registered: bool) -> Self {
        Self {
            status,
            executor_registered,
        }
    }

    /// The caller supplies the registry under its existing lock; do not lock
    /// again or reload a record outside that caller's synchronization protocol.
    pub(super) fn with_registry(
        record: &GraphRunRecord,
        active: &HashMap<String, ActiveRun>,
    ) -> Self {
        Self::observed(record.status, active.contains_key(&record.run_id))
    }

    pub(crate) fn is_quiescent(&self) -> bool {
        !self.executor_registered
            && (self.status.is_terminal() || self.status == RunStatus::Stopped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parked_or_unregistered_ready_runs_are_not_quiescent() {
        for (status, inactive_quiescent) in [
            (RunStatus::Ready, false),
            (RunStatus::Running, false),
            (RunStatus::Paused, false),
            (RunStatus::BudgetStopped, false),
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
