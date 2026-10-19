//! Attempt observations and human decisions are projections of Kernel history.
//! The execution port owns effect facts; a human control releases the Run only
//! after that port has durably recorded the observation. No tool is replayed.
use super::*;
use anchor_kernel::{Evidence, Waiter};
use anchor_runtime::graph::{
    InvocationKey, PendingRecovery, RecoveryAttempt, RecoveryDecision, RecoverySubmission,
};

impl KernelRuns {
    pub(crate) fn recovery_facts(
        &self,
        id: &str,
    ) -> Result<(Vec<PendingRecovery>, Vec<RecoverySubmission>), ApplicationError> {
        let resource = self.load(id)?.ok_or(ApplicationError::Missing)?;
        let record = record_of(&resource)?;
        let frontier = GraphValidator::frontier(&resource)?;
        let mut pending = Vec::new();
        let mut decisions: Vec<RecoverySubmission> = Vec::new();
        for entry in self.kernel.history(&resource.id).map_err(kernel_error)? {
            if !entry.verdict.is_accepted() {
                continue;
            }
            let content = &entry.submission.content;
            if entry.stamp.principal().id == OPERATOR_ID
                && entry.stamp.principal().kind == PrincipalKind::Human
                && let Some(value) = content.get("recovery_decision")
            {
                let decision: RecoverySubmission = serde_json::from_value(value.clone())
                    .map_err(|e| ApplicationError::Storage(e.to_string()))?;
                decisions.push(decision);
            }
            if entry.stamp.principal().id != HOST_ID
                || entry.stamp.principal().kind != PrincipalKind::Code
                || content["control"] != "stop"
            {
                continue;
            }
            if let (Some(key), Some(attempts)) = (
                content.get("invocation"),
                content.get("attempts").filter(|a| a.is_array()),
            ) {
                let key: InvocationKey = serde_json::from_value(key.clone())
                    .map_err(|e| ApplicationError::Storage(e.to_string()))?;
                let attempts: Vec<RecoveryAttempt> = serde_json::from_value(attempts.clone())
                    .map_err(|e| ApplicationError::Storage(e.to_string()))?;
                if key.run_id == record.run_id
                    && key.graph_digest == record.graph_digest
                    && frontier
                        .iter()
                        .any(|d| d.node.id == key.node_id && d.invocation == key.invocation)
                {
                    for attempt in attempts {
                        pending.retain(|p: &PendingRecovery| {
                            !(p.key == key && p.attempt.attempt_id == attempt.attempt_id)
                        });
                        pending.push(PendingRecovery {
                            key: key.clone(),
                            attempt,
                        });
                    }
                }
            }
        }
        pending.retain(|p| {
            !decisions
                .iter()
                .any(|d| d.key == p.key && d.attempt_id == p.attempt.attempt_id)
        });
        Ok((pending, decisions))
    }

    /// Returns whether an inactive, released Run needs driving. Holding the
    /// worker registry serializes decisions with controls, deletion and drive.
    pub(crate) fn recover_attempt(
        &self,
        key: &InvocationKey,
        attempt_id: i64,
        decision: RecoveryDecision,
        persist: impl FnOnce() -> Result<(), GraphError>,
    ) -> Result<bool, ApplicationError> {
        if decision == RecoveryDecision::Retry {
            return Err(ApplicationError::Conflict("unknown effects cannot be automatically replayed; inspect and record an observation, or abandon".into()));
        }
        let active = self.active.lock().unwrap();
        self.ensure_not_deleting(&key.run_id)?;
        let resource = self.load(&key.run_id)?.ok_or(ApplicationError::Missing)?;
        let (pending, decisions) = self.recovery_facts(&key.run_id)?;
        if let Some(existing) = decisions
            .iter()
            .find(|d| d.key == *key && d.attempt_id == attempt_id)
        {
            if existing.decision != decision {
                return Err(ApplicationError::Conflict(
                    "a different decision is already recorded for this attempt".into(),
                ));
            }
            return Ok(
                resource.phase() == Some(&Phase::Executing) && !active.contains_key(&key.run_id)
            );
        }
        if active.contains_key(&key.run_id) {
            return Err(ApplicationError::Conflict(
                "Kernel execution is still draining".into(),
            ));
        }
        if !matches!(
            resource.phase(),
            Some(Phase::Waiting {
                waiter: Waiter::Human { .. }
            })
        ) || !pending
            .iter()
            .any(|p| p.key == *key && p.attempt.attempt_id == attempt_id)
        {
            return Err(ApplicationError::Conflict(
                "attempt is not pending recovery for this Run".into(),
            ));
        }
        if let RecoveryDecision::Completed { observation } = &decision
            && observation.trim().is_empty()
        {
            return Err(ApplicationError::Invalid(
                "recovery observation must be nonempty".into(),
            ));
        }
        // The port's durable fact comes first. A crash before the following
        // submission safely resubmits exactly the same idempotent decision.
        persist()?;
        let operation = if decision == RecoveryDecision::Abort {
            "abandon"
        } else if pending.len() > 1 {
            "stop"
        } else {
            "resume"
        };
        let submission = Submission::new(&resource.id, resource.version, Action::Request)
            .content(serde_json::json!({"control":operation,"reason":"operator recovery decision","recovery_decision":RecoverySubmission { key:key.clone(), attempt_id, decision }}))
            .evidence(Evidence::Observation { id:format!("recovery-decision/{}/{attempt_id}", key.durable_key()) });
        let verdict = submit_with_retry(
            &self.kernel,
            &Principal::new(OPERATOR_ID, PrincipalKind::Human),
            0,
            submission,
            Some(&self.observations),
        )?;
        if !verdict.is_accepted() {
            return Err(ApplicationError::Conflict(format!(
                "Kernel recovery control refused: {:?}",
                verdict.reason
            )));
        }
        Ok(operation == "resume")
    }
}
