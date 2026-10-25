use crate::{
    HostControl,
    application::{ApplicationError, RunApplication, new_control},
    execution::PreparedExecution,
    run_metadata::RunMetadata,
};
use anchor_runtime::graph::{GraphRunRecord, InvocationKey, RecoveryDecision};

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
        let runs = self
            .kernel_owner(run_id)?
            .ok_or(ApplicationError::Missing)?;
        self.reject_superseded_conversation_execution(run_id)?;
        let resource = runs.load(run_id)?.ok_or(ApplicationError::Missing)?;
        let record = super::kernel_runs::KernelRuns::record(&resource)?;
        let metadata = self.metadata(run_id)?.ok_or(ApplicationError::Missing)?;
        super::validate_control_identity(&record, &metadata)?;
        let active = self.admission.lock().await;
        self.check_execution_scope(&metadata, &active)?;
        let key = InvocationKey {
            run_id: run_id.into(),
            graph_digest: record.graph_digest.clone(),
            node_id: node_id.into(),
            invocation,
        };
        let control = new_control();
        // Preparation queries Run views, so it must precede the worker-registry
        // lock held by recover_attempt. Preserve validation/error priority by
        // propagating a preparation failure only when its port is required.
        let prepared = self.prepare_resume(&record, &metadata, control.clone());
        let drive = runs.recover_attempt(&key, attempt_id, decision.clone(), || {
            let execution = prepared.as_ref().map_err(|error| {
                anchor_runtime::graph::GraphError::CorruptRun(format!("recovery port: {error:?}"))
            })?;
            execution
                .record_recovery_decision(&key, attempt_id, decision)
                .map_err(|error| {
                    anchor_runtime::graph::GraphError::CorruptRun(error.to_string())
                })?;
            Ok(())
        })?;
        if drive {
            self.spawn_kernel(record, prepared?, control)?;
        }
        Ok(())
    }

    /// An explicit human resume authorizes the Agent to inspect its existing
    /// native Session. It never asserts tool success or replays a pending tool.
    pub(in crate::application::run_access) fn inspect_interrupted_attempts(
        &self,
        runs: &super::kernel_runs::KernelRuns,
        run_id: &str,
        execution: &PreparedExecution,
    ) -> Result<(), ApplicationError> {
        let native = execution.pending_recovery(run_id)?;
        for pending in native {
            runs.observe_recovery_attempts(&pending.key, vec![pending.attempt])?;
        }
        for pending in runs.recovery_facts(run_id)?.0 {
            let decision = RecoveryDecision::Completed {
                observation: format!(
                    "The previous `{}` tool call (step {}) was interrupted before its result was recorded. Its external effect is unknown. Inspect the workspace, artifacts, and any available external state before deciding what to do; do not blindly repeat the call.",
                    pending.attempt.tool, pending.attempt.step
                ),
            };
            runs.recover_attempt(
                &pending.key,
                pending.attempt.attempt_id,
                decision.clone(),
                || {
                    execution
                        .record_recovery_decision(
                            &pending.key,
                            pending.attempt.attempt_id,
                            decision,
                        )
                        .map_err(anchor_runtime::graph::GraphError::CorruptRun)
                },
            )?;
        }
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
            self.clone(),
        )
        .map_err(ApplicationError::Invalid)
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixtures;
    use super::*;
    use anchor_runtime::graph::{GraphSnapshot, RecoveryAttempt};
    use serde_json::json;

    fn attempts() -> Vec<RecoveryAttempt> {
        vec![RecoveryAttempt {
            attempt_id: 17,
            step: 1,
            tool: "fixture".into(),
            started_at: "2026-10-19T00:00:00Z".into(),
        }]
    }

    fn pending(application: &RunApplication) -> InvocationKey {
        fixtures::create(application, &fixtures::work_record("recovery-view"));
        fixtures::recovery(application, "recovery-view", "worker", attempts())
    }

    #[tokio::test]
    async fn recovery_validation_keeps_invalid_input_before_missing_run() {
        let root = tempfile::tempdir().unwrap();
        let application = RunApplication::new(root.path().join("state"), root.path().into());
        for (node, invocation, attempt) in [("", 1, 17), ("worker", 0, 17), ("worker", 1, 0)] {
            assert!(matches!(application.record_recovery_decision(
                "missing", node, invocation, attempt, RecoveryDecision::Abort
            ).await, Err(ApplicationError::Invalid(message))
                if message == "node_id, invocation, and positive attempt_id are required"));
        }
        assert!(matches!(
            application
                .record_recovery_decision("missing", "worker", 1, 17, RecoveryDecision::Abort)
                .await,
            Err(ApplicationError::Missing)
        ));
    }

    #[test]
    fn recovery_target_requires_the_entire_current_key_and_attempt_without_port_writes() {
        let root = tempfile::tempdir().unwrap();
        let application = RunApplication::new(root.path().join("state"), root.path().into());
        let key = pending(&application);
        let runs = application.kernel_owner(&key.run_id).unwrap().unwrap();
        let before = runs.load(&key.run_id).unwrap().unwrap();
        for other in [
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
            assert!(
                runs.recover_attempt(&other, 17, RecoveryDecision::Abort, || panic!(
                    "invalid target must never reach the port"
                ))
                .is_err()
            );
        }
        assert!(
            runs.recover_attempt(&key, 18, RecoveryDecision::Abort, || panic!(
                "unknown attempt must never reach the port"
            ))
            .is_err()
        );
        assert_eq!(runs.load(&key.run_id).unwrap().unwrap(), before);
    }

    #[test]
    fn only_an_agent_node_can_own_an_io_harness_recovery_attempt() {
        let root = tempfile::tempdir().unwrap();
        let application = RunApplication::new(root.path().join("state"), root.path().into());
        let record = GraphRunRecord::create_with_id(
            GraphSnapshot::from_authoring(json!({
                "entry":"work", "agents":{}, "ops":{"work":{"run":"true"}},
                "nodes":[{"id":"work","op":"work"}], "edges":[]
            }))
            .unwrap(),
            json!({}),
            "op-recovery",
        )
        .unwrap();
        fixtures::create(&application, &record);
        let key = fixtures::recovery(&application, "op-recovery", "work", attempts());
        let runs = application.kernel_owner(&key.run_id).unwrap().unwrap();
        let before = runs.load(&key.run_id).unwrap().unwrap();
        assert!(
            matches!(runs.recover_attempt(&key, 17, RecoveryDecision::Abort,
            || panic!("an Op attempt cannot reach the io-harness port")),
            Err(ApplicationError::Conflict(message))
                if message == "only an AgentNode can own an io-harness recovery attempt")
        );
        assert_eq!(runs.load(&key.run_id).unwrap().unwrap(), before);
    }

    #[test]
    fn recovery_validation_accepts_a_matching_parallel_frontier() {
        let root = tempfile::tempdir().unwrap();
        let application = RunApplication::new(root.path().join("state"), root.path().into());
        let record = GraphRunRecord::create_with_id(
            GraphSnapshot::from_authoring(json!({
                "entry":"fork", "agents":{"role":{"model":"fixture","instructions":"work"}},
                "ops":{"fork":{"fanout":{"join":"join"}},"join":{"join":{}}},
                "nodes":[{"id":"fork","op":"fork"},{"id":"left","agent":"role"},
                    {"id":"right","agent":"role"},{"id":"join","op":"join"}],
                "edges":[{"from":"fork","to":"left"},{"from":"fork","to":"right"},
                    {"from":"left","to":"join"},{"from":"right","to":"join"}]
            }))
            .unwrap(),
            json!({}),
            "parallel-recovery",
        )
        .unwrap();
        fixtures::create(&application, &record);
        fixtures::complete(
            &application,
            "parallel-recovery",
            "fork",
            fixtures::completion(json!({})),
            None,
        );
        let key = fixtures::recovery(&application, "parallel-recovery", "left", attempts());
        let runs = application.kernel_owner(&key.run_id).unwrap().unwrap();
        assert_eq!(runs.recovery_facts(&key.run_id).unwrap().0[0].key, key);
        assert!(
            !runs
                .recover_attempt(&key, 17, RecoveryDecision::Abort, || Ok(()))
                .unwrap()
        );
        assert!(fixtures::view(&application, &key.run_id).is_settled());
        assert!(
            !fixtures::record(&application, &key.run_id)
                .results
                .contains_key("left")
        );
        assert!(
            !fixtures::record(&application, &key.run_id)
                .results
                .contains_key("right")
        );
    }

    #[test]
    fn recovery_submission_lookup_matches_the_entire_key_and_attempt() {
        let root = tempfile::tempdir().unwrap();
        let application = RunApplication::new(root.path().join("state"), root.path().into());
        let key = pending(&application);
        let runs = application.kernel_owner(&key.run_id).unwrap().unwrap();
        runs.recover_attempt(&key, 17, RecoveryDecision::Abort, || Ok(()))
            .unwrap();
        let (_, decisions) = runs.recovery_facts(&key.run_id).unwrap();
        let lookup = |target: &InvocationKey, attempt| {
            decisions
                .iter()
                .find(|decision| decision.key == *target && decision.attempt_id == attempt)
                .map(|decision| &decision.decision)
        };
        assert_eq!(lookup(&key, 17), Some(&RecoveryDecision::Abort));
        assert_eq!(lookup(&key, 18), None);
        for other in [
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
            assert_eq!(lookup(&other, 17), None);
        }
        assert!(
            !runs
                .recover_attempt(&key, 17, RecoveryDecision::Abort, || panic!(
                    "duplicate decision must reuse durable fact"
                ))
                .unwrap()
        );
    }
}
