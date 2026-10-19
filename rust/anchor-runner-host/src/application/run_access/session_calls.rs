use super::RunView;
use crate::application::{
    ApplicationError, ConversationSource, RunApplication, RunMetadata, metadata, new_control,
};
use anchor_runtime::graph::{GraphRunRecord, RecoveryDecision};

impl RunView<'_> {
    fn can_continue_session_call(&self) -> bool {
        (!self.is_settled() || self.has_succeeded())
            && !matches!(
                self.phase(),
                super::RunPhase::Waiting {
                    reason: Some("budget"),
                    ..
                }
            )
    }
}

impl RunApplication {
    pub(crate) fn settled_session_call_successors(
        &self,
        metadata: &RunMetadata,
    ) -> Result<Vec<GraphRunRecord>, ApplicationError> {
        let Some(call) = &metadata.session_call else {
            return Ok(Vec::new());
        };
        if call.status != "pending" {
            return Ok(Vec::new());
        }
        let binding = metadata.conversation.as_ref().ok_or_else(|| {
            ApplicationError::Conflict("Session call has no execution binding".into())
        })?;
        if metadata.graph_call.is_none()
            || call.context.session != binding.session
            || call.context.reply_node != binding.reply_node
        {
            return Err(ApplicationError::Conflict(
                "Session call execution identity changed".into(),
            ));
        }
        let chain = self.conversation_chain(binding, &metadata.bundle_source)?;
        let mut next = chain.first().map(|(entry, _)| entry.run_id.as_str());
        let mut successors = Vec::new();
        while let Some(id) = next {
            if id == metadata.run_id {
                return Ok(successors);
            }
            let (entry, record) = chain
                .iter()
                .find(|(entry, _)| entry.run_id == id)
                .ok_or_else(|| ApplicationError::Conflict("Session successor is missing".into()))?;
            if !record.has_stopped_execution() {
                return Err(ApplicationError::Conflict(
                    "Session successor is not settled".into(),
                ));
            }
            successors.push(record.record.clone().into_owned());
            next = entry
                .conversation
                .as_ref()
                .and_then(|binding| binding.previous_run.as_deref());
        }
        Err(ApplicationError::Conflict(
            "Session call is outside the admitted conversation chain".into(),
        ))
    }

    pub(crate) async fn stop_session_call(&self, run_id: &str) -> Result<(), ApplicationError> {
        let active = self.admission.lock().await;
        let mut metadata = self.metadata(run_id)?.ok_or(ApplicationError::Missing)?;
        let call = metadata
            .session_call
            .as_mut()
            .ok_or_else(|| ApplicationError::Invalid("Run is not a Session call".into()))?;
        if call.status != "pending" {
            return Err(ApplicationError::Conflict(
                "Session delivery already settled".into(),
            ));
        }
        let runs = self
            .kernel_owner(run_id)?
            .ok_or(ApplicationError::Missing)?;
        let view = self.read_run(run_id)?.ok_or(ApplicationError::Missing)?;
        if !view.is_settled() {
            runs.control(run_id, "stop", None)?;
        }
        call.status = "failed".into();
        call.error = "background call stopped".into();
        metadata::save(&self.data_root, &metadata)?;
        drop(active);
        self.kernel_child_finished(run_id).await
    }

    pub(crate) async fn yield_session_call(&self, run_id: &str) -> Result<(), ApplicationError> {
        let metadata = self.metadata(run_id)?.ok_or(ApplicationError::Missing)?;
        if metadata.session_call.is_none() {
            return Err(ApplicationError::Invalid(
                "Run is not a Session call".into(),
            ));
        }
        let runs = self
            .kernel_owner(run_id)?
            .ok_or(ApplicationError::Missing)?;
        if !self
            .read_run(run_id)?
            .ok_or(ApplicationError::Missing)?
            .is_settled()
        {
            runs.control(run_id, "stop", None)?;
        }
        Ok(())
    }

    /// The authenticated Session host holds the same foreground/background lease used by
    /// message admission. Never expose this capability through ordinary resume.
    pub(crate) async fn execute_session_call(
        &self,
        run_id: &str,
        session: &str,
        previous: Option<String>,
    ) -> Result<(), ApplicationError> {
        let active = self.admission.lock().await;
        let mut metadata = self.metadata(run_id)?.ok_or(ApplicationError::Missing)?;
        let call = metadata
            .session_call
            .as_ref()
            .ok_or_else(|| ApplicationError::Invalid("Run is not a Session call".into()))?;
        if call.status != "pending" || call.context.session != session {
            return Err(ApplicationError::Conflict(
                "Session call is no longer pending for this Session".into(),
            ));
        }
        let binding = ConversationSource {
            session: session.into(),
            reply_node: call.context.reply_node.clone(),
            previous_run: previous,
        };
        if metadata
            .conversation
            .as_ref()
            .is_some_and(|existing| existing != &binding)
        {
            return Err(ApplicationError::Conflict(
                "Session execution binding changed".into(),
            ));
        }
        let runs = self
            .kernel_owner(run_id)?
            .ok_or(ApplicationError::Missing)?;
        if runs.active(run_id) {
            return Ok(());
        }
        let _graph_lease = self.graph_admission_lease(&metadata.bundle_source)?;
        let lease = self.run_leases().acquire_lease(run_id)?;
        let view = self.read_run(run_id)?.ok_or(ApplicationError::Missing)?;
        if view.has_succeeded() {
            return Ok(());
        }
        if !view.can_continue_session_call() {
            return Err(ApplicationError::Conflict(
                "Session call execution cannot be continued".into(),
            ));
        }
        let record = &view.record;
        if metadata.graph_digest != record.graph_digest {
            return Err(ApplicationError::Conflict(
                "Session call Graph identity changed".into(),
            ));
        }
        let graph_path = Self::graph_identity(&metadata.bundle_source)?;
        for (id, other) in self.run_views()? {
            if id == run_id || other.is_settled() {
                continue;
            }
            if let Some(identity) = self.metadata(&id)?
                && Self::graph_identity(&identity.bundle_source)? == graph_path
                && identity.conversation.is_none()
                && identity.session_call.is_none()
            {
                return Err(ApplicationError::Conflict(
                    "Graph has unfinished execution outside this Session".into(),
                ));
            }
        }
        let chain = self.conversation_chain(&binding, &metadata.bundle_source)?;
        for (_, other) in &chain {
            if other.progress().run_id != run_id {
                self.ensure_predecessor_settled(other, &active)?;
            }
        }
        if metadata.conversation.is_none() {
            if chain.first().map(|(head, _)| head.run_id.as_str())
                != binding.previous_run.as_deref()
            {
                return Err(ApplicationError::Conflict(
                    "previous_run must name the latest conversation Run".into(),
                ));
            }
            if !record
                .snapshot
                .nodes
                .iter()
                .any(|node| node.id == binding.reply_node)
            {
                return Err(ApplicationError::Invalid(
                    "reply node is absent from the accepted Graph".into(),
                ));
            }
            metadata.conversation = Some(binding);
            metadata::save(&self.data_root, &metadata)?;
        }
        let control = new_control();
        let execution = self.prepare_resume(record, &metadata, control.clone())?;
        // The authenticated Session host releases its existing invocation for
        // inspection. Persist the native observation through the same recovery
        // authority as ordinary Runs; an unknown effect is never replayed here.
        for pending in runs.recovery_facts(run_id)?.0 {
            let decision = RecoveryDecision::Completed {
                observation: format!(
                    "The previous {} tool call was interrupted before its result was recorded. Its effect is unknown. Inspect the actual state before continuing; do not blindly repeat it.",
                    pending.attempt.tool
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
        let resumed = self.read_run(run_id)?.ok_or(ApplicationError::Missing)?;
        if !matches!(resumed.phase(), super::RunPhase::Executing) {
            runs.control(run_id, "resume", None)?;
        }
        let record = record.clone().into_owned();
        drop(lease);
        drop(_graph_lease);
        drop(active);
        self.spawn_kernel(record, execution, control)?;
        Ok(())
    }

    pub(crate) async fn settle_session_call(
        &self,
        run_id: &str,
        status: &str,
        error: &str,
    ) -> Result<(), ApplicationError> {
        if !matches!(status, "delivered" | "failed") {
            return Err(ApplicationError::Invalid(
                "invalid Session delivery status".into(),
            ));
        }
        let active = self.admission.lock().await;
        if self.read_run(run_id)?.is_some_and(|view| view.active()) {
            return Err(ApplicationError::Conflict(
                "Session execution has not settled".into(),
            ));
        }
        let lease = self.run_leases().acquire_lease(run_id)?;
        let view = self.read_run(run_id)?.ok_or(ApplicationError::Missing)?;
        if status == "delivered" && !view.has_succeeded() {
            return Err(ApplicationError::Conflict(
                "only completed execution can be delivered".into(),
            ));
        }
        let mut metadata = self.metadata(run_id)?.ok_or(ApplicationError::Missing)?;
        let call = metadata
            .session_call
            .as_mut()
            .ok_or_else(|| ApplicationError::Invalid("Run is not a Session call".into()))?;
        if call.status != "pending" && call.status != status {
            return Err(ApplicationError::Conflict(
                "Session delivery already settled differently".into(),
            ));
        }
        call.status = status.into();
        call.error = error.chars().take(1000).collect();
        metadata::save(&self.data_root, &metadata)?;
        drop(lease);
        drop(active);
        self.kernel_child_finished(run_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixtures;
    use super::*;

    #[test]
    fn session_continuation_preserves_budget_recovery_and_completion_distinctions() {
        let root = tempfile::tempdir().unwrap();
        let application = RunApplication::new(root.path().join("state"), root.path().into());
        for id in fixtures::scenarios(&application) {
            assert_eq!(
                fixtures::view(&application, id).can_continue_session_call(),
                !matches!(id, "budget" | "failed" | "aborted"),
                "{id}"
            );
        }
    }
}
