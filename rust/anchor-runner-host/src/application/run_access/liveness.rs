//! Compatibility liveness constructor retained for legacy test fixtures.
use anchor_runtime::graph::RunStatus;
pub(crate) struct RunLiveness {
    quiescent: bool,
}
impl RunLiveness {
    pub(crate) fn observed(status: RunStatus, executor_registered: bool) -> Self {
        Self {
            quiescent: super::is_quiescent(status, executor_registered),
        }
    }
    pub(crate) fn is_quiescent(&self) -> bool {
        self.quiescent
    }
}
