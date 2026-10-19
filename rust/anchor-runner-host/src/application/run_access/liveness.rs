use super::is_quiescent;
use crate::application::ActiveRun;
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
    pub(in crate::application) fn with_registry(
        record: &GraphRunRecord,
        active: &HashMap<String, ActiveRun>,
    ) -> Self {
        Self::observed(record.status, active.contains_key(&record.run_id))
    }

    pub(crate) fn is_quiescent(&self) -> bool {
        is_quiescent(self.status, self.executor_registered)
    }
}
