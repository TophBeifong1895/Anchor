use crate::application::{ApplicationError, RunApplication};

impl RunApplication {
    pub(in crate::application) async fn release_parent_after_recorded_delivery(
        &self,
        run: &str,
        _graph: &str,
    ) -> Result<(), ApplicationError> {
        self.kernel_child_finished(run).await
    }

    pub(crate) async fn dispatch_detached(&self, run_id: &str) -> Result<(), ApplicationError> {
        self.kernel_dispatch_detached(run_id).await
    }

    pub(crate) async fn begin_wait_child(
        &self,
        run_id: &str,
        _graph: &str,
        cancellation: anchor_runtime::Cancellation,
    ) -> Result<anchor_graph_host::ChildRunControl, ApplicationError> {
        self.kernel_owner(run_id)?
            .ok_or(ApplicationError::Missing)?;
        Ok(anchor_graph_host::ChildRunControl { cancellation })
    }

    pub(crate) async fn end_wait_child(&self, _run_id: &str) {}

    pub(in crate::application) fn executor(
        &self,
    ) -> Result<&tokio::runtime::Handle, ApplicationError> {
        self.executor.as_ref().ok_or_else(|| {
            ApplicationError::Invalid("Run execution requires the owning Host executor".into())
        })
    }
}
