//! Run access boundary for the constitution migration.
//! Keep native ownership, lifecycle views and temporary compatibility bridges
//! together until the complete master F1 interface is ready to merge.
pub(crate) mod kernel_runs;
pub(super) mod legacy_run_view;
mod run_json;
mod run_observations;
pub(super) mod run_view;

use super::{ApplicationError, RunApplication, new_control, storage};
use anchor_runtime::graph::GraphRunRecord;

impl RunApplication {
    pub(crate) fn read_view(
        &self,
        record: &GraphRunRecord,
        worker: bool,
    ) -> Result<run_view::RunView, ApplicationError> {
        if let Some(runs) = self.kernel_owner(&record.run_id)? {
            return runs.view(
                &runs
                    .load(&record.run_id)?
                    .ok_or(ApplicationError::Missing)?,
            );
        }
        Ok(legacy_run_view::view(record, worker))
    }
}
