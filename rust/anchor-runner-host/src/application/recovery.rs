use super::{ActiveRun, ApplicationError, RunApplication, new_control, run_access};
use anchor_runtime::graph::{InvocationKey, RecoveryDecision, RunStatus, RunStore};

impl RunApplication {
    pub(crate) async fn recover(
        &self,
        run_id: &str,
        node_id: &str,
        invocation: u64,
        attempt_id: i64,
        decision: RecoveryDecision,
    ) -> Result<(), ApplicationError> {
        if self
            .metadata(run_id)?
            .is_some_and(|metadata| metadata.session_call.is_some())
        {
            return Err(ApplicationError::Conflict(
                "Session calls recover through the Session host".into(),
            ));
        }
        if attempt_id <= 0 || invocation == 0 || node_id.is_empty() {
            return Err(ApplicationError::Invalid(
                "node_id, invocation, and positive attempt_id are required".into(),
            ));
        }
        if self.abandon_intent(run_id)?.is_some() {
            // A recorded abandon request is one-way: a recovery decision must
            // not restart the round the operator gave up on.
            return Err(ApplicationError::Conflict(
                "that Run was abandoned; start a new Run instead of recovering it".into(),
            ));
        }
        let mut active = self.active.lock().await;
        self.reject_superseded_conversation_execution(run_id)?;
        let mut record = self
            .store()
            .load(run_id)?
            .ok_or(ApplicationError::Missing)?;
        let metadata = self.metadata(run_id)?.ok_or_else(|| {
            ApplicationError::Invalid(
                "Run has no immutable Graph identity metadata; it cannot be recovered".into(),
            )
        })?;
        run_access::validate_control_identity(&record, &metadata)?;
        let key = InvocationKey {
            run_id: run_id.to_owned(),
            graph_digest: run_access::RunView::inactive(&record)
                .progress()
                .graph_digest
                .to_owned(),
            node_id: node_id.to_owned(),
            invocation,
        };
        if let Some(existing) =
            run_access::RunView::inactive(&record).recorded_recovery_decision(&key, attempt_id)
        {
            if existing != &decision {
                return Err(ApplicationError::Conflict(
                    "a different decision is already recorded for this attempt".into(),
                ));
            }
            // The response may have been lost after the Run was committed as
            // Running but before its task entered the active map. Replaying the
            // same decision must be able to kick that same durable cursor.
            if run_access::RunView::inactive(&record)
                .with_active(active.contains_key(run_id))
                .needs_recovery_replay()
            {
                drop(active);
                // The same decision may be submitted again after its response
                // was lost. Re-enter control through one heap indirection so
                // the idempotent kick does not create an infinitely sized
                // async future (control itself may handle a waiting record).
                Box::pin(self.control(run_id, "resume")).await?;
            }
            return Ok(());
        }
        self.check_execution_scope(&metadata, &active)?;
        if active.contains_key(run_id) {
            return Err(ApplicationError::Conflict("Run is already active".into()));
        }
        run_access::RunView::inactive(&record).validate_recovery_target(&key, attempt_id)?;
        let graph_path = Self::graph_identity(&metadata.bundle_source)?;
        let _graph_lease = self.graph_lease(&metadata.bundle_source)?;
        let _lease = self.store().acquire_lease(run_id)?;
        record = self
            .store()
            .load(run_id)?
            .ok_or(ApplicationError::Missing)?;
        if let Some(existing) =
            run_access::RunView::inactive(&record).recorded_recovery_decision(&key, attempt_id)
        {
            return if existing == &decision {
                Ok(())
            } else {
                Err(ApplicationError::Conflict(
                    "a different decision is already recorded for this attempt".into(),
                ))
            };
        }
        run_access::RunView::inactive(&record).validate_leased_recovery_target(&key, attempt_id)?;
        let control = new_control();
        let is_abort = decision == RecoveryDecision::Abort;
        // The Harness intent is the first durable side of the handoff. If the
        // following Graph write fails or the host dies, the still-pending Graph
        // attempt can safely resubmit this exact idempotent decision.
        let execution = self.prepare_resume(&record, &metadata, control.clone())?;
        execution
            .record_recovery_decision(&key, attempt_id, decision.clone())
            .map_err(ApplicationError::Conflict)?;
        record
            .recovery_submissions
            .push(anchor_runtime::graph::RecoverySubmission {
                key: key.clone(),
                attempt_id,
                decision: decision.clone(),
            });
        record
            .recovery
            .retain(|pending| !(pending.key == key && pending.attempt.attempt_id == attempt_id));
        if is_abort {
            // Abort is a Run-level stop: make the Run terminal before touching
            // any other branch. Remaining unresolved attempts are retained as
            // read-only facts and no cached Retry/Completed intent is executed.
            // The Harness abort intent and journal close are already durable.
            record.status = RunStatus::Aborted;
            record.error = Some(format!("{} aborted by operator", node_id));
            record.touch();
            self.store().save(&record)?;
            return Ok(());
        }
        if run_access::RunView::inactive(&record).has_pending_recovery() {
            record.status = RunStatus::WaitingRecovery;
            record.touch();
            self.store().save(&record)?;
            return Ok(());
        }
        record.status = RunStatus::Running;
        record.touch();
        self.store().save(&record)?;
        self.check_store()?;
        active.insert(run_id.to_owned(), ActiveRun::new(graph_path, control));
        drop(_lease);
        drop(_graph_lease);
        self.spawn(record, execution)?;
        Ok(())
    }
}
