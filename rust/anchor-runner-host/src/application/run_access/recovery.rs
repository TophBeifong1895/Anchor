use super::RunView;
use crate::application::{ActiveRun, new_control};
use crate::{
    HostControl,
    application::{ApplicationError, RunApplication, RunMetadata},
    execution::PreparedExecution,
};
use anchor_runtime::graph::RunStore;
use anchor_runtime::graph::{GraphRunRecord, InvocationKey, RecoveryDecision, RunStatus};

enum RecoveryTargetMismatch {
    Cursor,
    Attempt,
    Phase,
}

impl RunView<'_> {
    pub(crate) fn recorded_recovery_decision(
        &self,
        key: &InvocationKey,
        attempt_id: i64,
    ) -> Option<&RecoveryDecision> {
        self.record
            .recovery_submissions
            .iter()
            .find(|submission| submission.key == *key && submission.attempt_id == attempt_id)
            .map(|submission| &submission.decision)
    }

    pub(crate) fn needs_recovery_replay(&self) -> bool {
        self.record.status == RunStatus::Running && !self.active()
    }

    fn has_recovery_cursor(&self, key: &InvocationKey) -> bool {
        self.record
            .cursor
            .as_ref()
            .is_some_and(|cursor| &cursor.key == key)
            || self.record.parallel.as_ref().is_some_and(|activation| {
                activation.branches.iter().any(|branch| {
                    branch
                        .cursor
                        .as_ref()
                        .is_some_and(|cursor| &cursor.key == key)
                })
            })
    }

    fn has_pending_attempt(&self, key: &InvocationKey, attempt_id: i64) -> bool {
        self.record
            .recovery
            .iter()
            .any(|pending| &pending.key == key && pending.attempt.attempt_id == attempt_id)
    }

    fn recovery_target(
        &self,
        key: &InvocationKey,
        attempt_id: i64,
    ) -> Result<(), RecoveryTargetMismatch> {
        if !self.has_recovery_cursor(key) {
            return Err(RecoveryTargetMismatch::Cursor);
        }
        if !self.has_pending_attempt(key, attempt_id) {
            return Err(RecoveryTargetMismatch::Attempt);
        }
        if !self.has_pending_recovery() {
            return Err(RecoveryTargetMismatch::Phase);
        }
        Ok(())
    }

    pub(crate) fn validate_recovery_target(
        &self,
        key: &InvocationKey,
        attempt_id: i64,
    ) -> Result<(), ApplicationError> {
        self.recovery_target(key, attempt_id).map_err(|mismatch| {
            ApplicationError::Conflict(
                match mismatch {
                    RecoveryTargetMismatch::Cursor => {
                        "recovery invocation does not match an active cursor in this Run"
                    }
                    RecoveryTargetMismatch::Attempt => {
                        "attempt is not pending recovery for this Run"
                    }
                    RecoveryTargetMismatch::Phase => "Run is not waiting for a recovery decision",
                }
                .into(),
            )
        })?;
        if !self
            .progress()
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

    pub(crate) fn validate_leased_recovery_target(
        &self,
        key: &InvocationKey,
        attempt_id: i64,
    ) -> Result<(), ApplicationError> {
        self.recovery_target(key, attempt_id).map_err(|_| {
            ApplicationError::Conflict(
                "Run recovery state changed before the decision was accepted".into(),
            )
        })
    }
}

impl RunApplication {
    pub(in crate::application) async fn record_recovery_decision(
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
        if let Some(runs) = self.kernel_owner(run_id)? {
            self.reject_superseded_conversation_execution(run_id)?;
            let resource = runs.load(run_id)?.ok_or(ApplicationError::Missing)?;
            let record = super::kernel_runs::KernelRuns::record(&resource)?;
            let metadata = self.metadata(run_id)?.ok_or(ApplicationError::Missing)?;
            super::validate_control_identity(&record, &metadata)?;
            let active = self.active.lock().await;
            self.check_execution_scope(&metadata, &active)?;
            let key = InvocationKey {
                run_id: run_id.into(),
                graph_digest: record.graph_digest.clone(),
                node_id: node_id.into(),
                invocation,
            };
            let control = new_control();
            let execution = self.prepare_resume(&record, &metadata, control.clone())?;
            let drive = runs.recover_attempt(&key, attempt_id, decision.clone(), || {
                execution
                    .record_recovery_decision(&key, attempt_id, decision)
                    .map_err(|e| anchor_runtime::graph::GraphError::CorruptRun(e.to_string()))
            })?;
            if drive {
                self.spawn_kernel(record, execution, control)?;
            }
            return Ok(());
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
        super::validate_control_identity(&record, &metadata)?;
        let key = InvocationKey {
            run_id: run_id.to_owned(),
            graph_digest: super::RunView::inactive(&record)
                .progress()
                .graph_digest
                .to_owned(),
            node_id: node_id.to_owned(),
            invocation,
        };
        if let Some(existing) =
            super::RunView::inactive(&record).recorded_recovery_decision(&key, attempt_id)
        {
            if existing != &decision {
                return Err(ApplicationError::Conflict(
                    "a different decision is already recorded for this attempt".into(),
                ));
            }
            // The response may have been lost after the Run was committed as
            // Running but before its task entered the active map. Replaying the
            // same decision must be able to kick that same durable cursor.
            if super::RunView::inactive(&record)
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
        super::RunView::inactive(&record).validate_recovery_target(&key, attempt_id)?;
        let graph_path = Self::graph_identity(&metadata.bundle_source)?;
        let _graph_lease = self.graph_lease(&metadata.bundle_source)?;
        let _lease = self.store().acquire_lease(run_id)?;
        record = self
            .store()
            .load(run_id)?
            .ok_or(ApplicationError::Missing)?;
        if let Some(existing) =
            super::RunView::inactive(&record).recorded_recovery_decision(&key, attempt_id)
        {
            return if existing == &decision {
                Ok(())
            } else {
                Err(ApplicationError::Conflict(
                    "a different decision is already recorded for this attempt".into(),
                ))
            };
        }
        super::RunView::inactive(&record).validate_leased_recovery_target(&key, attempt_id)?;
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
        if super::RunView::inactive(&record).has_pending_recovery() {
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

    pub(in crate::application::run_access) fn prepare_resume(
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

#[cfg(test)]
mod tests {
    use super::*;
    use anchor_runtime::graph::{
        GraphSnapshot, ParallelActivation, ParallelBranchRecord, ParallelBranchStatus,
        PendingRecovery, RecoveryAttempt, RecoverySubmission, RunCursor,
    };
    use serde_json::json;

    fn pending_record() -> (GraphRunRecord, InvocationKey) {
        let snapshot = GraphSnapshot::admit(json!({
            "objective":"recovery fixture", "entry":"worker",
            "agents":{"role":{"model":"fixture","instructions":"work"}},
            "nodes":[{"id":"worker","agent":"role"}], "edges":[]
        }))
        .unwrap();
        let mut record =
            GraphRunRecord::create_with_id(snapshot, json!({}), "recovery-view").unwrap();
        let key = InvocationKey {
            run_id: record.run_id.clone(),
            graph_digest: record.graph_digest.clone(),
            node_id: "worker".into(),
            invocation: 1,
        };
        record.cursor = Some(RunCursor {
            node_id: key.node_id.clone(),
            key: key.clone(),
            input_commits: vec![],
            prepared_input: json!({}),
        });
        record.status = RunStatus::WaitingRecovery;
        record.recovery.push(PendingRecovery {
            key: key.clone(),
            attempt: RecoveryAttempt {
                attempt_id: 17,
                step: 1,
                tool: "fixture".into(),
                started_at: "fixture".into(),
            },
        });
        (record, key)
    }

    fn conflict(result: Result<(), ApplicationError>) -> String {
        match result {
            Err(ApplicationError::Conflict(message)) => message,
            unexpected => panic!("expected a conflict, got {unexpected:?}"),
        }
    }

    #[test]
    fn recovery_validation_keeps_initial_error_priority_and_lease_conflict() {
        let (mut record, key) = pending_record();
        assert!(
            RunView::inactive(&record)
                .validate_recovery_target(&key, 17)
                .is_ok()
        );
        assert!(
            RunView::inactive(&record)
                .validate_leased_recovery_target(&key, 17)
                .is_ok()
        );
        let cursor = record.cursor.take();
        let pending = std::mem::take(&mut record.recovery);
        record.status = RunStatus::Stopped;
        assert_eq!(
            conflict(RunView::inactive(&record).validate_recovery_target(&key, 17)),
            "recovery invocation does not match an active cursor in this Run"
        );
        assert_eq!(
            conflict(RunView::inactive(&record).validate_leased_recovery_target(&key, 17)),
            "Run recovery state changed before the decision was accepted"
        );
        record.cursor = cursor;
        assert_eq!(
            conflict(RunView::inactive(&record).validate_recovery_target(&key, 17)),
            "attempt is not pending recovery for this Run"
        );
        record.recovery = pending;
        assert_eq!(
            conflict(RunView::inactive(&record).validate_recovery_target(&key, 17)),
            "Run is not waiting for a recovery decision"
        );
        assert_eq!(
            conflict(RunView::inactive(&record).validate_leased_recovery_target(&key, 17)),
            "Run recovery state changed before the decision was accepted"
        );
    }

    #[test]
    fn only_initial_validation_checks_the_agent_node_after_recovery_facts() {
        let (mut record, key) = pending_record();
        record.snapshot.nodes[0].agent = None;
        assert_eq!(
            conflict(RunView::inactive(&record).validate_recovery_target(&key, 17)),
            "only an AgentNode can own an io-harness recovery attempt"
        );
        assert!(
            RunView::inactive(&record)
                .validate_leased_recovery_target(&key, 17)
                .is_ok()
        );
        record.recovery.clear();
        assert_eq!(
            conflict(RunView::inactive(&record).validate_recovery_target(&key, 17)),
            "attempt is not pending recovery for this Run"
        );
    }

    #[test]
    fn recovery_validation_accepts_a_matching_parallel_cursor() {
        let (mut record, key) = pending_record();
        record.parallel = Some(ParallelActivation {
            activation_id: "activation".into(),
            fanout_node: "fanout".into(),
            join_node: "join".into(),
            fanout_invocation: 1,
            branches: vec![ParallelBranchRecord {
                branch_id: "branch".into(),
                entry: "worker".into(),
                nodes: vec!["worker".into()],
                next_index: 0,
                status: ParallelBranchStatus::Running,
                cursor: record.cursor.take(),
                completed: vec![],
                error: None,
            }],
        });
        assert!(
            RunView::inactive(&record)
                .validate_recovery_target(&key, 17)
                .is_ok()
        );
        assert!(
            RunView::inactive(&record)
                .validate_leased_recovery_target(&key, 17)
                .is_ok()
        );
        let other_key = InvocationKey {
            invocation: 2,
            ..key
        };
        assert_eq!(
            conflict(RunView::inactive(&record).validate_recovery_target(&other_key, 17)),
            "recovery invocation does not match an active cursor in this Run"
        );
    }

    #[test]
    fn recovery_submission_lookup_matches_the_entire_key_and_attempt() {
        let (mut record, key) = pending_record();
        record.recovery_submissions.push(RecoverySubmission {
            key: key.clone(),
            attempt_id: 17,
            decision: RecoveryDecision::Abort,
        });
        let view = RunView::inactive(&record);
        assert_eq!(
            view.recorded_recovery_decision(&key, 17),
            Some(&RecoveryDecision::Abort)
        );
        assert_eq!(view.recorded_recovery_decision(&key, 18), None);
        for other_key in [
            InvocationKey {
                run_id: "other".into(),
                ..key.clone()
            },
            InvocationKey {
                graph_digest: "other".into(),
                ..key.clone()
            },
            InvocationKey {
                node_id: "other".into(),
                ..key.clone()
            },
            InvocationKey {
                invocation: 2,
                ..key.clone()
            },
        ] {
            assert_eq!(view.recorded_recovery_decision(&other_key, 17), None);
        }
    }

    #[test]
    fn recovery_replay_only_kicks_running_records_without_a_registered_executor() {
        let (mut record, _) = pending_record();
        for status in [
            RunStatus::Ready,
            RunStatus::Running,
            RunStatus::Paused,
            RunStatus::Stopped,
            RunStatus::BudgetStopped,
            RunStatus::WaitingCall,
            RunStatus::WaitingRecovery,
            RunStatus::Completed,
            RunStatus::Failed,
            RunStatus::Aborted,
        ] {
            record.status = status;
            for active in [false, true] {
                assert_eq!(
                    RunView::inactive(&record)
                        .with_active(active)
                        .needs_recovery_replay(),
                    status == RunStatus::Running && !active,
                    "{status:?}, active={active}"
                );
            }
        }
    }
}
