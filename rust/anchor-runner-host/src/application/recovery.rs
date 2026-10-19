use super::{ApplicationError, RunApplication};
use anchor_runtime::graph::RecoveryDecision;

impl RunApplication {
    pub(crate) async fn recover(
        &self,
        run_id: &str,
        node_id: &str,
        invocation: u64,
        attempt_id: i64,
        decision: RecoveryDecision,
    ) -> Result<(), ApplicationError> {
        self.record_recovery_decision(run_id, node_id, invocation, attempt_id, decision)
            .await
    }
}
