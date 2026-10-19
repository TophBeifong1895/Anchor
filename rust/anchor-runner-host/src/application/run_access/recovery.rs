use super::RunView;
use crate::{
    HostControl,
    application::{ApplicationError, RunApplication, RunMetadata},
    execution::PreparedExecution,
};
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
    pub(in crate::application) fn prepare_resume(
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
