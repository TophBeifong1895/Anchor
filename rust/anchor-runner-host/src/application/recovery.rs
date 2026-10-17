use super::{ActiveRun, ApplicationError, RunApplication, RunMetadata, new_control};
use crate::{HostControl, execution::PreparedExecution};
use anchor_runtime::graph::{GraphRunRecord, InvocationKey, RecoveryDecision, RunStatus, RunStore};

impl RunApplication {
    pub(crate) async fn recover(
        &self,
        run_id: &str,
        node_id: &str,
        invocation: u64,
        attempt_id: i64,
        decision: RecoveryDecision,
    ) -> Result<(), ApplicationError> {
        if self.kernel_owner(run_id)?.is_some() {
            return Err(ApplicationError::Conflict("P probe: recording Goose attempt recovery decisions is Unsupported; verify the execution backend before resume, or abandon".into()));
        }
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
        if metadata.graph_digest != record.graph_digest {
            return Err(ApplicationError::Invalid(
                "Run Graph identity metadata does not match its frozen snapshot".into(),
            ));
        }
        let key = InvocationKey {
            run_id: run_id.to_owned(),
            graph_digest: record.graph_digest.clone(),
            node_id: node_id.to_owned(),
            invocation,
        };
        if let Some(existing) = record
            .recovery_submissions
            .iter()
            .find(|submission| submission.key == key && submission.attempt_id == attempt_id)
        {
            if existing.decision != decision {
                return Err(ApplicationError::Conflict(
                    "a different decision is already recorded for this attempt".into(),
                ));
            }
            // The response may have been lost after the Run was committed as
            // Running but before its task entered the active map. Replaying the
            // same decision must be able to kick that same durable cursor.
            if record.status == RunStatus::Running && !active.contains_key(run_id) {
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
        validate_recovery_target(&record, &key, attempt_id)?;
        let graph_path = Self::graph_identity(&metadata.bundle_source)?;
        let _graph_lease = self.graph_lease(&metadata.bundle_source)?;
        let _lease = self.store().acquire_lease(run_id)?;
        record = self
            .store()
            .load(run_id)?
            .ok_or(ApplicationError::Missing)?;
        if let Some(existing) = record
            .recovery_submissions
            .iter()
            .find(|submission| submission.key == key && submission.attempt_id == attempt_id)
        {
            return if existing.decision == decision {
                Ok(())
            } else {
                Err(ApplicationError::Conflict(
                    "a different decision is already recorded for this attempt".into(),
                ))
            };
        }
        if !has_recovery_cursor(&record, &key)
            || !has_pending_attempt(&record, &key, attempt_id)
            || record.status != RunStatus::WaitingRecovery
        {
            return Err(ApplicationError::Conflict(
                "Run recovery state changed before the decision was accepted".into(),
            ));
        }
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
        if !record.recovery.is_empty() {
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

    pub(super) fn prepare_resume(
        &self,
        record: &GraphRunRecord,
        metadata: &RunMetadata,
        control: HostControl,
    ) -> Result<PreparedExecution, ApplicationError> {
        self.ensure_current_assistant(metadata)?;
        // Resuming never re-checks the Plugin: the next node call uses whatever is
        // installed now, and records which view that was.
        PreparedExecution::prepare_with_catalog(
            record,
            control,
            record.plugin_bindings.clone(),
            self.catalog_root.clone(),
            self.clone(),
        )
        .map_err(ApplicationError::Invalid)
    }
}

fn has_recovery_cursor(record: &GraphRunRecord, key: &InvocationKey) -> bool {
    record
        .cursor
        .as_ref()
        .is_some_and(|cursor| &cursor.key == key)
        || record.parallel.as_ref().is_some_and(|activation| {
            activation.branches.iter().any(|branch| {
                branch
                    .cursor
                    .as_ref()
                    .is_some_and(|cursor| &cursor.key == key)
            })
        })
}

fn has_pending_attempt(record: &GraphRunRecord, key: &InvocationKey, attempt_id: i64) -> bool {
    record
        .recovery
        .iter()
        .any(|pending| &pending.key == key && pending.attempt.attempt_id == attempt_id)
}

fn validate_recovery_target(
    record: &GraphRunRecord,
    key: &InvocationKey,
    attempt_id: i64,
) -> Result<(), ApplicationError> {
    if !has_recovery_cursor(record, key) {
        return Err(ApplicationError::Conflict(
            "recovery invocation does not match an active cursor in this Run".into(),
        ));
    }
    if !has_pending_attempt(record, key, attempt_id) {
        return Err(ApplicationError::Conflict(
            "attempt is not pending recovery for this Run".into(),
        ));
    }
    if record.status != RunStatus::WaitingRecovery {
        return Err(ApplicationError::Conflict(
            "Run is not waiting for a recovery decision".into(),
        ));
    }
    if !record
        .snapshot
        .nodes
        .iter()
        .any(|node| node.id == key.node_id && node.agent.is_some())
    {
        return Err(ApplicationError::Conflict(
            "only an AgentNode can own an io-harness recovery attempt".into(),
        ));
    }
    Ok(())
}
