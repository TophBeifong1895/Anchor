use super::{ApplicationError, RunApplication};
use anchor_runtime::graph::RunLease;
use std::path::Path;

impl RunApplication {
    pub(crate) fn graph_admission_lease(
        &self,
        graph_path: &Path,
    ) -> Result<Box<dyn RunLease>, ApplicationError> {
        let graph_path = crate::run_metadata::graph_identity(graph_path)?;
        let lease = self.graph_lease(&graph_path)?;
        // An unfinished Run with no immutable Graph identity cannot be attributed
        // to any Graph, so refuse to admit or create Runs until it is resolved.
        for (id, record) in self.run_views()? {
            if !record.is_settled() && self.metadata(&id)?.is_none() {
                return Err(ApplicationError::Conflict(
                    "an unfinished Run has no immutable Graph identity; admission is quarantined"
                        .into(),
                ));
            }
        }
        Ok(lease)
    }
}
