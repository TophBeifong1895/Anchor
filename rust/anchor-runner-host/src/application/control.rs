use super::{ApplicationError, RunApplication};

impl RunApplication {
    pub(crate) async fn control(
        &self,
        run_id: &str,
        operation: &str,
    ) -> Result<(), ApplicationError> {
        self.control_expected(run_id, operation, None).await
    }
}
