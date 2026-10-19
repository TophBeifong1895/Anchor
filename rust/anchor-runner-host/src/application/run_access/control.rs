use super::{
    RunView, reject_terminal_control, validate_control_identity, validate_inactive_control,
    validate_resume_budget, wait_child_ids,
};
use crate::application::{ActiveRun, ApplicationError, RunApplication, RunMetadata, new_control};
use crate::execution::PreparedExecution;
use anchor_runtime::graph::{GraphRunRecord, RunStatus, RunStore};
use std::sync::atomic::Ordering;

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
        if operation == "resume" && self.abandon_intent(run_id)?.is_some() {
            // Abandonment is one-way: a resumed Run would continue the round the
            // operator gave up. The intent is durable, so this holds across a
            // restart and while the terminal write is still in flight.
            return Err(ApplicationError::Conflict(
                "that Run was abandoned; start a new Run instead of resuming it".into(),
            ));
        }
        if self
            .metadata(run_id)?
            .is_some_and(|metadata| metadata.session_call.is_some())
        {
            match operation {
                "resume" | "pause" => {
                    return Err(ApplicationError::Conflict(
                        "Session calls are coordinated through the Session host".into(),
                    ));
                }
                "stop" => return self.stop_session_call(run_id).await,
                _ => unreachable!(),
            }
        }
        if operation == "resume" && self.resume_interrupted_attempts(run_id).await? {
            return Ok(());
        }
        let mut active = self.active.lock().await;
        if let Some((node, invocation)) = waiting {
            let expected = self
                .read_run(run_id)?
                .is_some_and(|parent| parent.waits_for_invocation(node, invocation));
            if !expected || active.contains_key(run_id) {
                return Ok(());
            }
        }
        if operation == "resume" {
            self.reject_superseded_conversation_execution(run_id)?;
        }
        if let Some(run) = active.get(run_id) {
            match operation {
                "pause" => run.control.pause.store(true, Ordering::Relaxed),
                "stop" => run.control.cancellation.store(true, Ordering::Relaxed),
                _ => {
                    return Err(ApplicationError::Conflict(
                        "that Run is already active".into(),
                    ));
                }
            }
            if operation == "stop" {
                drop(active);
                self.stop_wait_session_children(run_id).await?;
            }
            return Ok(());
        }
        let record = self
            .store()
            .load(run_id)?
            .ok_or(ApplicationError::Missing)?;
        validate_inactive_control(&record, operation)?;
        let metadata = self.control_metadata(&record)?;
        let is_child = metadata.trigger_source == "graph_call";
        let graph_path = Self::graph_identity(&metadata.bundle_source)?;
        if operation == "resume" {
            self.ensure_current_assistant(&metadata)?;
            self.check_execution_scope(&metadata, &active)?;
        }
        let graph_lease = if is_child {
            None
        } else {
            Some(self.graph_lease(&metadata.bundle_source)?)
        };
        let lease = self.store().acquire_lease(run_id)?;
        let mut record = self
            .store()
            .load(run_id)?
            .ok_or(ApplicationError::Missing)?;
        reject_terminal_control(&record)?;
        if operation == "stop" && RunView::inactive(&record).waits_for_children() {
            record.status = RunStatus::Stopped;
            record.touch();
            self.store().save(&record)?;
            let wait_children = wait_child_ids(&record);
            drop(active);
            drop(lease);
            self.stop_wait_children(run_id, wait_children).await?;
            let lease = self.store().acquire_lease(run_id)?;
            if let Some(mut current) = self.store().load(run_id)?
                && RunView::inactive(&current).waits_for_children()
            {
                current.status = RunStatus::Stopped;
                current.touch();
                self.store().save(&current)?;
            }
            drop(lease);
            return Ok(());
        }
        if operation == "stop" {
            // Inactive stop preserves cursors and all completion facts; it dispatches no work.
            record.status = RunStatus::Stopped;
            record.touch();
            self.store().save(&record)?;
            self.settle_channel_run(run_id, RunStatus::Stopped)?;
            return Ok(());
        }
        validate_resume_budget(&record)?;
        // No Plugin re-check on resume: a Run uses the *current* Plugin
        // (standard invariant 6). The bindings the record carries are the
        // observation of what it used, not a gate that a later Plugin edit breaks.
        let control = new_control();
        let execution = PreparedExecution::prepare_with_catalog(
            &record,
            control.clone(),
            record.plugin_bindings.clone(),
            self.catalog_root.clone(),
            self.clone(),
        )
        .map_err(ApplicationError::Invalid)?;
        self.check_store()?;
        active.insert(run_id.to_owned(), ActiveRun::new(graph_path, control));
        drop(lease);
        drop(graph_lease);
        self.spawn(record, execution)?;
        Ok(())
    }

    fn control_metadata(&self, record: &GraphRunRecord) -> Result<RunMetadata, ApplicationError> {
        let metadata = self.metadata(&record.run_id)?.ok_or_else(|| {
            ApplicationError::Invalid(
                "Run has no immutable Graph identity metadata; it cannot be resumed".into(),
            )
        })?;
        validate_control_identity(record, &metadata)?;
        Ok(metadata)
    }
}
