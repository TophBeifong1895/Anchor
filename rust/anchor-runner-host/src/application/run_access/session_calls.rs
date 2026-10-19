use super::RunView;
use crate::application::{
    ActiveRun, ApplicationError, ConversationSource, RunApplication, RunMetadata, metadata,
    new_control,
};
use anchor_runtime::graph::{GraphRunRecord, RecoveryDecision, RunStatus, RunStore};
use std::sync::atomic::Ordering;

impl RunView<'_> {
    fn can_continue_session_call(&self) -> bool {
        (!self.is_settled() || self.has_succeeded())
            && self.record.status != RunStatus::BudgetStopped
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
        let active = self.active.lock().await;
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
        let lease = if active.contains_key(run_id) {
            None
        } else {
            Some(self.store().acquire_lease(run_id)?)
        };
        let mut record = self
            .store()
            .load(run_id)?
            .ok_or(ApplicationError::Missing)?;
        call.status = "failed".into();
        call.error = "background call stopped".into();
        metadata::save(&self.data_root, &metadata)?;
        if let Some(run) = active.get(run_id) {
            run.control.cancellation.store(true, Ordering::Release);
        } else if !RunView::inactive(&record).is_settled() {
            record.status = RunStatus::Stopped;
            self.store().save(&record)?;
        }
        drop(lease);
        drop(active);
        self.child_finished(run_id, &metadata.graph, RunStatus::Completed)
            .await
    }

    pub(crate) async fn yield_session_call(&self, run_id: &str) -> Result<(), ApplicationError> {
        let active = self.active.lock().await;
        let metadata = self.metadata(run_id)?.ok_or(ApplicationError::Missing)?;
        if metadata.session_call.is_none() {
            return Err(ApplicationError::Invalid(
                "Run is not a Session call".into(),
            ));
        }
        if let Some(run) = active.get(run_id) {
            run.control.cancellation.store(true, Ordering::Release);
            return Ok(());
        }
        let _lease = self.store().acquire_lease(run_id)?;
        let mut record = self
            .store()
            .load(run_id)?
            .ok_or(ApplicationError::Missing)?;
        if !RunView::inactive(&record).is_settled() {
            record.status = RunStatus::Stopped;
            self.store().save(&record)?;
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
        let mut active = self.active.lock().await;
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
        if active.contains_key(run_id) {
            return Ok(());
        }
        let _graph_lease = self.graph_admission_lease(&metadata.bundle_source)?;
        let lease = self.store().acquire_lease(run_id)?;
        let mut record = self
            .store()
            .load(run_id)?
            .ok_or(ApplicationError::Missing)?;
        if RunView::inactive(&record).has_succeeded() {
            return Ok(());
        }
        if !RunView::inactive(&record).can_continue_session_call() {
            return Err(ApplicationError::Conflict(
                "Session call execution cannot be continued".into(),
            ));
        }
        if metadata.graph_digest != record.graph_digest {
            return Err(ApplicationError::Conflict(
                "Session call Graph identity changed".into(),
            ));
        }
        let graph_path = Self::graph_identity(&metadata.bundle_source)?;
        for (id, other) in self.raw_records()? {
            if id == run_id || RunView::inactive(&other).is_settled() {
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
        let execution = self.prepare_resume(&record, &metadata, control.clone())?;
        // Ordinary recovery records an unknown effect as context; it never replays a tool.
        for pending in std::mem::take(&mut record.recovery) {
            let decision = RecoveryDecision::Completed {
                observation: format!(
                    "The previous {} tool call was interrupted before its result was recorded. Its effect is unknown. Inspect the actual state before continuing; do not blindly repeat it.",
                    pending.attempt.tool
                ),
            };
            execution
                .record_recovery_decision(
                    &pending.key,
                    pending.attempt.attempt_id,
                    decision.clone(),
                )
                .map_err(ApplicationError::Conflict)?;
            record
                .recovery_submissions
                .push(anchor_runtime::graph::RecoverySubmission {
                    key: pending.key,
                    attempt_id: pending.attempt.attempt_id,
                    decision,
                });
        }
        record.status = RunStatus::Running;
        self.store().save(&record)?;
        active.insert(
            run_id.into(),
            ActiveRun::new(Self::graph_identity(&metadata.bundle_source)?, control),
        );
        drop(lease);
        drop(_graph_lease);
        self.spawn(record, execution)?;
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
        let active = self.active.lock().await;
        if active.contains_key(run_id) {
            return Err(ApplicationError::Conflict(
                "Session execution has not settled".into(),
            ));
        }
        let lease = self.store().acquire_lease(run_id)?;
        let record = self
            .store()
            .load(run_id)?
            .ok_or(ApplicationError::Missing)?;
        if status == "delivered" && !RunView::inactive(&record).has_succeeded() {
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
        self.child_finished(run_id, &metadata.graph, RunStatus::Completed)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anchor_runtime::graph::GraphSnapshot;
    use serde_json::json;

    #[test]
    fn session_continuation_preserves_budget_and_completion_distinctions() {
        let snapshot = GraphSnapshot::from_authoring(json!({
            "entry":"work", "agents":{}, "ops":{"work":{"run":"true"}},
            "nodes":[{"id":"work","op":"work"}], "edges":[]
        }))
        .unwrap();
        let mut record =
            GraphRunRecord::create_with_id(snapshot, json!({}), "session-call").unwrap();
        for (status, allowed) in [
            (RunStatus::Ready, true),
            (RunStatus::Running, true),
            (RunStatus::Paused, true),
            (RunStatus::Stopped, true),
            (RunStatus::BudgetStopped, false),
            (RunStatus::WaitingCall, true),
            (RunStatus::WaitingRecovery, true),
            (RunStatus::Completed, true),
            (RunStatus::Failed, false),
            (RunStatus::Aborted, false),
        ] {
            record.status = status;
            assert_eq!(
                RunView::inactive(&record).can_continue_session_call(),
                allowed,
                "{status:?}"
            );
        }
    }
}
