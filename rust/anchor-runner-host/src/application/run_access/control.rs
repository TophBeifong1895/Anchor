use crate::application::{ApplicationError, RunApplication};

impl RunApplication {
    pub(crate) async fn control(
        &self,
        run_id: &str,
        operation: &str,
    ) -> Result<(), ApplicationError> {
        match operation {
            "stop" => self.stop(run_id).await,
            "pause" => self.pause(run_id).await,
            "resume" => self.resume(run_id).await,
            _ => Err(ApplicationError::Invalid("unknown Run control".into())),
        }
    }

    pub(crate) async fn stop(&self, run_id: &str) -> Result<(), ApplicationError> {
        self.control_expected(run_id, "stop", None).await
    }

    pub(crate) async fn pause(&self, run_id: &str) -> Result<(), ApplicationError> {
        self.control_expected(run_id, "pause", None).await
    }

    pub(crate) async fn resume(&self, run_id: &str) -> Result<(), ApplicationError> {
        self.control_expected(run_id, "resume", None).await
    }

    /// Continue the current assistant instance in its own Run after a restart.
    ///
    /// Only an instance that was idle at its input wait qualifies: it is still
    /// executing with nothing running it, its single frontier is the wait node,
    /// no Turn is bound there and no attempt is unresolved, so continuing
    /// replays nothing and needs no human release. Returns `false` when the
    /// instance does not qualify; the caller then hands it over instead.
    pub(crate) async fn resume_idle_assistant(
        &self,
        run_id: &str,
    ) -> Result<bool, ApplicationError> {
        let Some(metadata) = self.metadata(run_id)? else {
            return Ok(false);
        };
        let Some(source) = metadata.assistant.as_ref() else {
            return Ok(false);
        };
        let Some(view) = self.read_run(run_id)? else {
            return Ok(false);
        };
        if view.active() || !view.can_dispatch() || view.has_pending_recovery() {
            return Ok(false);
        }
        let [cursor] = view.dispatches() else {
            return Ok(false);
        };
        if cursor.node_id != source.wait_node
            || crate::assistant::binding(&self.data_root, &cursor.key)
                .map_err(ApplicationError::Storage)?
                .is_some()
            || self.abandon_intent(run_id)?.is_some()
        {
            return Ok(false);
        }
        self.resume(run_id).await?;
        Ok(true)
    }

    /// Stop every child Run a parked parent still waits on, plus their pending
    /// Session deliveries, exactly as an explicit `stop` does.
    pub(in crate::application) async fn stop_wait_children(
        &self,
        run_id: &str,
        wait_children: Vec<String>,
    ) -> Result<(), ApplicationError> {
        self.stop_wait_session_children(run_id).await?;
        for child_id in wait_children {
            if self
                .read_run(&child_id)?
                .is_some_and(|child| !child.has_stopped_execution())
            {
                let _ = Box::pin(self.control(&child_id, "stop")).await;
            }
        }
        Ok(())
    }

    pub(in crate::application::run_access) async fn control_expected(
        &self,
        run_id: &str,
        operation: &str,
        waiting: Option<(&str, u64)>,
    ) -> Result<(), ApplicationError> {
        let runs = self
            .kernel_owner(run_id)?
            .ok_or(ApplicationError::Missing)?;
        if let Some((node, invocation)) = waiting
            && (runs.active(run_id)
                || !self
                    .read_run(run_id)?
                    .is_some_and(|parent| parent.waits_for_invocation(node, invocation)))
        {
            return Ok(());
        }
        if self
            .metadata(run_id)?
            .is_some_and(|metadata| metadata.session_call.is_some())
        {
            return match operation {
                "stop" => self.stop_session_call(run_id).await,
                "pause" | "resume" => Err(ApplicationError::Conflict(
                    "Session calls are coordinated through the Session host".into(),
                )),
                _ => Err(ApplicationError::Invalid("unknown Run control".into())),
            };
        }
        let children = if operation == "stop" {
            self.read_run(run_id)?
                .filter(|view| view.waits_for_children())
                .map(|view| super::wait_child_ids(&view.record))
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        self.kernel_control(runs, run_id, operation).await?;
        if operation == "stop" {
            self.stop_wait_children(run_id, children).await?;
            self.settle_channel_run(run_id)?;
        }
        Ok(())
    }
}
