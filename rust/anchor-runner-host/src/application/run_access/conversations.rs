use super::RunView;
use crate::application::{
    ActiveRun, ApplicationError, ChannelRunSource, ConversationAdmission, RunApplication,
    RunMetadata, WECOM_REPLY_KEY_PREFIX, WECOM_REPLY_KIND, metadata, new_control, storage,
};
use crate::{create_durable_directory, execution::PreparedExecution};
use anchor_graph_host::LoadedGraphBundle;
use anchor_kernel::{Outcome, Phase, Waiter};
use anchor_platform_session::{ChannelDeliveryRequest, SessionError, SessionStore, TurnStatus};
use anchor_runtime::graph::{GraphRunRecord, RunLease, RunStatus, RunStore};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeSet, HashMap},
    path::Path,
};

fn channel_turn_status(phase: &Phase, assistant: bool) -> Option<TurnStatus> {
    match phase {
        Phase::Settled {
            result: Outcome::Done,
        } => (!assistant).then_some(TurnStatus::Completed),
        Phase::Settled { .. } => Some(TurnStatus::Failed),
        Phase::Waiting {
            waiter: Waiter::Human { reason },
        } if !matches!(reason.as_str(), "paused" | "budget") => Some(TurnStatus::Stopped),
        _ => None,
    }
}

impl RunApplication {
    pub(crate) fn wecom_channel_delivery_request<'record>(
        metadata: &RunMetadata,
        record: impl Into<RunView<'record>>,
    ) -> Option<ChannelDeliveryRequest> {
        let record = record.into();
        let record = &record.record;
        let channel = metadata.channel.as_ref()?;
        let conversation = metadata.conversation.as_ref()?;
        if record
            .input
            .get("channel")
            .and_then(|channel| channel.get("source"))
            .and_then(Value::as_str)
            != Some("wecom")
        {
            return None;
        }
        let submission = record
            .results
            .get(&conversation.reply_node)
            .and_then(|results| results.last())
            .map(|result| result.completion.submission.as_str())
            .filter(|submission| !submission.trim().is_empty())?;
        let content_sha256 = format!("{:x}", Sha256::digest(submission.as_bytes()));
        Some(ChannelDeliveryRequest {
            key: format!(
                "{WECOM_REPLY_KEY_PREFIX}{}:{}",
                channel.session, channel.inbound
            ),
            kind: WECOM_REPLY_KIND.into(),
            content_sha256,
        })
    }

    pub(crate) fn settle_channel_run(
        &self,
        run_id: &str,
        status: RunStatus,
    ) -> Result<(), ApplicationError> {
        let Some(metadata) = self.metadata(run_id)? else {
            return Ok(());
        };
        // Kernel outcomes and waiting reasons are read from their authority;
        // the callback status is only the legacy fallback for missing Runs.
        let phase = self
            .read_run(run_id)?
            .map(|view| view.lifecycle.phase)
            .unwrap_or_else(|| super::legacy_run_view::phase(status, None));
        if let Some(source) = metadata.assistant.as_ref() {
            let Some(turn_status) = channel_turn_status(&phase, true) else {
                return Ok(());
            };
            let sessions = SessionStore::open(self.data_root.join("platform/sessions.sqlite"))
                .map_err(|error| ApplicationError::Storage(error.to_string()))?;
            if sessions
                .get_channel_assistant(&source.owner, &source.session)
                .map_err(|error| ApplicationError::Storage(error.to_string()))?
                .is_none_or(|assistant| assistant.run_id != run_id)
            {
                return Ok(());
            }
            for turn in sessions
                .list_turns(&source.owner, &source.session)
                .map_err(|error| ApplicationError::Storage(error.to_string()))?
            {
                if turn.status == TurnStatus::Running {
                    sessions
                        .finish_turn(
                            &source.owner,
                            &source.session,
                            &turn.id,
                            turn_status,
                            Some("assistant execution stopped"),
                        )
                        .map_err(|error| ApplicationError::Storage(error.to_string()))?;
                }
            }
            crate::assistant::signal(&source.session).notify_waiters();
            return Ok(());
        }
        let Some(channel) = metadata.channel.as_ref() else {
            return Ok(());
        };
        let Some(turn_status) = channel_turn_status(&phase, false) else {
            return Ok(());
        };
        let completed_record = if turn_status == TurnStatus::Completed {
            self.read_run(run_id)?
        } else {
            None
        };
        let sessions = SessionStore::open(self.data_root.join("platform/sessions.sqlite"))
            .map_err(|error| ApplicationError::Storage(error.to_string()))?;
        match sessions.get_channel_inbound(&channel.owner, &channel.session, &channel.inbound) {
            Ok(inbound) if inbound.relation.superseded_by_turn_id.is_some() => return Ok(()),
            Ok(_) | Err(SessionError::Missing) => {}
            Err(error) => return Err(ApplicationError::Storage(error.to_string())),
        }
        match sessions.finish_turn(
            &channel.owner,
            &channel.session,
            &channel.turn,
            turn_status,
            None,
        ) {
            Ok(_) | Err(SessionError::Missing) => {}
            Err(error) => return Err(ApplicationError::Storage(error.to_string())),
        };
        let Some(record) = completed_record else {
            return Ok(());
        };
        let Some(request) = Self::wecom_channel_delivery_request(&metadata, &record) else {
            return Ok(());
        };
        match sessions.admit_completed_channel_delivery(
            &channel.owner,
            &channel.session,
            &channel.turn,
            request,
        ) {
            Ok(_) | Err(SessionError::Missing) => Ok(()),
            Err(conflict @ SessionError::Conflict(_)) => {
                match sessions.get_channel_inbound(
                    &channel.owner,
                    &channel.session,
                    &channel.inbound,
                ) {
                    Ok(inbound) if inbound.relation.superseded_by_turn_id.is_some() => Ok(()),
                    Ok(_) => Err(ApplicationError::Storage(conflict.to_string())),
                    Err(error) => Err(ApplicationError::Storage(error.to_string())),
                }
            }
            Err(error) => Err(ApplicationError::Storage(error.to_string())),
        }
    }

    pub(in crate::application) fn retry_conversation(
        &self,
        request: &ConversationAdmission,
        graph_path: &Path,
        attachments: &crate::channel_inputs::PreparedAttachments,
        channel: Option<&ChannelRunSource>,
    ) -> Result<Option<String>, ApplicationError> {
        let Some(view) = self.read_run(&request.run)? else {
            return Ok(None);
        };
        let record = view.record;
        let metadata = self.metadata(&request.run)?.ok_or_else(|| {
            ApplicationError::Conflict("Run has no immutable conversation identity".into())
        })?;
        let repeated = GraphRunRecord::create_with_id(
            record.snapshot.clone(),
            request.input.clone(),
            request.run.clone(),
        )?;
        if metadata.graph != request.graph
            || Self::graph_identity(&metadata.bundle_source)? != Self::graph_identity(graph_path)?
            || metadata.graph_digest != record.graph_digest
            || metadata.conversation.as_ref() != Some(&request.source())
            || metadata.channel.as_ref() != channel
            || repeated.input != record.input
            || metadata.attachments != attachments.manifest()
        {
            return Err(ApplicationError::Conflict(
                "Run id was already used for different conversation input".into(),
            ));
        }
        crate::channel_inputs::verify(&self.data_root, &metadata)
            .map_err(ApplicationError::Conflict)?;
        Ok(Some(request.run.clone()))
    }

    pub(in crate::application) fn ensure_predecessor_settled<'record>(
        &self,
        record: impl Into<RunView<'record>>,
        active: &HashMap<String, ActiveRun>,
    ) -> Result<(), ApplicationError> {
        self.ensure_wait_closure_settled(record, active, false)
    }

    pub(in crate::application) fn ensure_conversation_delete_settled<'record>(
        &self,
        record: impl Into<RunView<'record>>,
        active: &HashMap<String, ActiveRun>,
    ) -> Result<(), ApplicationError> {
        self.ensure_wait_closure_settled(record, active, true)
    }

    fn ensure_wait_closure_settled<'record>(
        &self,
        record: impl Into<RunView<'record>>,
        active: &HashMap<String, ActiveRun>,
        allow_deleted_children: bool,
    ) -> Result<(), ApplicationError> {
        let mut pending = vec![record.into()];
        let mut seen = BTreeSet::new();
        while let Some(record) = pending.pop() {
            let run_id = record.progress().run_id.to_owned();
            if !seen.insert(run_id.clone()) {
                continue;
            }
            let current = match self.read_run(&run_id)? {
                Some(current) => current,
                None => record,
            };
            let worker = if current.kernel_owned {
                current.active()
            } else {
                active.contains_key(&run_id)
            };
            let record = current.with_active(worker);
            // A physically parked conversation still owns its current turn
            // while paused or held for budget. Session handoff requires an
            // explicit stop or settlement, as before the facade migration.
            let turn_held = matches!(
                record.phase(),
                super::RunPhase::Waiting {
                    for_what: super::RunWait::Human,
                    reason: Some("paused" | "budget"),
                }
            );
            if turn_held || !record.is_quiescent() {
                return Err(ApplicationError::Conflict(
                    "previous conversation execution has not stopped; wait for it to become inactive".into(),
                ));
            }
            let mut children = record
                .record
                .graph_calls
                .values()
                .filter(|call| call.mode == "wait")
                .filter_map(|call| call.child_run_id.clone())
                .collect::<BTreeSet<_>>();
            children.extend(
                self.child_metadata(&run_id)?
                    .into_iter()
                    .filter_map(|metadata| {
                        metadata
                            .graph_call
                            .as_ref()
                            .filter(|source| source.mode == "wait")
                            .map(|_| metadata.run_id.clone())
                    }),
            );
            for id in children {
                match self.read_run(&id)? {
                    Some(child) => pending.push(child),
                    None if allow_deleted_children => {}
                    None => {
                        return Err(ApplicationError::Conflict(
                            "previous conversation has a missing wait child".into(),
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// True while this later Run can still read the Run it names as its
    /// predecessor. Every such read happens while the later Run executes the
    /// first invocation of a node, so a recorded first invocation is the
    /// durable evidence that the inheritance is complete.
    pub(in crate::application) fn successor_reads_predecessor<'record>(
        &self,
        predecessor: impl Into<RunView<'record>>,
        successor_metadata: &RunMetadata,
        successor: Option<&RunView<'_>>,
    ) -> Result<bool, ApplicationError> {
        let predecessor = predecessor.into();
        let Some(successor) = successor else {
            // A handover that committed its binding and metadata but not its Run
            // record still needs this Run to be re-admitted, so its progress is
            // unknown and this Run stays retained.
            return Ok(true);
        };
        // Callers can hold an earlier projection. Refresh the authority before
        // deciding whether this successor can ever execute another inheritance.
        let current = self.read_run(successor.progress().run_id)?;
        let successor = match current.as_ref() {
            Some(current) => current,
            None => successor,
        };
        if successor.is_settled() {
            return Ok(false);
        }
        // A Run with its own successor can never resume or recover: the
        // conversation scope check refuses it, so its history is frozen.
        if self.has_conversation_successor(successor.progress().run_id)? {
            return Ok(false);
        }
        let predecessor = predecessor.progress();
        let successor = successor.progress();
        let mut reading = BTreeSet::new();
        for node in &predecessor.snapshot.nodes {
            let executed = predecessor.invocations.get(&node.id).copied().unwrap_or(0) > 0
                || predecessor
                    .results
                    .get(&node.id)
                    .is_some_and(|results| !results.is_empty());
            if executed {
                reading.insert(node.id.clone());
            }
        }
        if let Some(assistant) = successor_metadata.assistant.as_ref() {
            reading.insert(assistant.work_node.clone());
        }
        for node in reading {
            if !successor
                .snapshot
                .nodes
                .iter()
                .any(|candidate| candidate.id == node)
            {
                continue;
            }
            let injected = successor
                .results
                .get(&node)
                .is_some_and(|results| results.iter().any(|result| result.key.invocation == 1));
            if !injected {
                return Ok(true);
            }
        }
        Ok(false)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn admit_conversation(
        &self,
        request: ConversationAdmission,
        source: &Path,
        bundle: LoadedGraphBundle,
        graph_lease: Box<dyn RunLease>,
        attachments: &crate::channel_inputs::PreparedAttachments,
        channel: Option<ChannelRunSource>,
        oauth_owner: String,
    ) -> Result<String, ApplicationError> {
        request.validate().map_err(ApplicationError::Invalid)?;
        let graph_path = Self::graph_identity(source)?;
        let plugin_lease = if bundle.plugins.is_empty() {
            None
        } else {
            Some(self.plugin_reference_lease().await?)
        };
        let mut active = self.active.lock().await;
        if let Some(run) =
            self.retry_conversation(&request, &graph_path, attachments, channel.as_ref())?
        {
            return Ok(run);
        }
        if self.metadata(&request.run)?.is_some() {
            return Err(ApplicationError::Conflict(
                "Run identity was saved without a complete admission; inspect the retained facts"
                    .into(),
            ));
        }
        let conversation = request.source();
        let chain = self.conversation_chain(&conversation, &graph_path)?;
        if chain.first().map(|(metadata, _)| metadata.run_id.as_str())
            != request.previous_run.as_deref()
        {
            return Err(ApplicationError::Conflict(
                "previous_run must name the latest Run of this Session".into(),
            ));
        }
        for (_, record) in &chain {
            self.ensure_predecessor_settled(record, &active)?;
        }
        for (id, record) in self.run_views()? {
            if record.is_settled() {
                continue;
            }
            let metadata = self.metadata(&id)?.ok_or_else(|| {
                ApplicationError::Conflict(
                    "an unfinished Run has no immutable Graph identity; admission is quarantined"
                        .into(),
                )
            })?;
            if Self::graph_identity(&metadata.bundle_source)? == graph_path
                && metadata.conversation.is_none()
                && metadata.session_call.is_none()
            {
                return Err(ApplicationError::Conflict(
                    "this graph has an unfinished non-conversation Run".into(),
                ));
            }
        }
        if !bundle
            .snapshot
            .nodes
            .iter()
            .any(|node| node.id == request.reply_node)
        {
            return Err(ApplicationError::Invalid(
                "reply_node is not a node in this Graph".into(),
            ));
        }
        let plugin_bindings = self.bundle_plugin_bindings(&bundle)?;
        let mut record =
            GraphRunRecord::create_with_id(bundle.snapshot, request.input, request.run.clone())?;
        record.plugin_bindings = plugin_bindings;
        record.plugin_bindings_initialized = true;
        self.check_store()?;
        let mut metadata = RunMetadata::new(
            request.run.clone(),
            request.graph.clone(),
            record.graph_digest.clone(),
            source,
        )?;
        metadata.trigger_source = "channel".into();
        metadata.oauth_owner = Some(oauth_owner);
        metadata.conversation = Some(conversation);
        metadata.channel = channel;
        metadata.attachments = attachments.manifest();
        let lease = self.store().acquire_lease(&request.run)?;
        if chain.is_empty() {
            let workspaces = crate::env_path("ANCHOR_RUNNER_WORKSPACE_ROOT")
                .map_err(ApplicationError::Invalid)?;
            crate::conversation_resources::start_fresh(
                self,
                &self.data_root,
                &graph_path,
                &request.session,
                &workspaces,
            )?;
        }
        crate::channel_inputs::freeze(&self.data_root, &metadata, attachments)
            .map_err(ApplicationError::Conflict)?;
        // Resolvers read this host-owned context; an orphan metadata file is never an accepted Run.
        metadata::save(&self.data_root, &metadata)?;
        let control = new_control();
        let execution = PreparedExecution::prepare_with_catalog(
            &record,
            control.clone(),
            record.plugin_bindings.clone(),
            self.catalog_root.clone(),
            self.clone(),
        )
        .map_err(ApplicationError::Invalid)?;
        execution
            .bind_local_inputs(&record, &request.graph)
            .map_err(ApplicationError::Invalid)?;
        create_durable_directory(&self.data_root.join("runs")).map_err(storage)?;
        self.store().save(&record)?;
        active.insert(request.run.clone(), ActiveRun::new(graph_path, control));
        drop(lease);
        drop(plugin_lease);
        drop(graph_lease);
        self.spawn(record, execution)?;
        Ok(request.run)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anchor_runtime::graph::{
        CommitRef, GraphSnapshot, InvocationKey, NodeCompletion, RunResult,
    };
    use serde_json::json;

    fn record(run_id: &str) -> GraphRunRecord {
        GraphRunRecord::create_with_id(
            GraphSnapshot::from_authoring(json!({
                "entry":"work", "agents":{}, "ops":{"work":{"run":"true"}},
                "nodes":[{"id":"work","op":"work"}], "edges":[]
            }))
            .unwrap(),
            json!({}),
            run_id,
        )
        .unwrap()
    }

    #[test]
    fn channel_settlement_preserves_assistant_completion_distinction() {
        for (status, normal, assistant) in [
            (RunStatus::Ready, None, None),
            (RunStatus::Running, None, None),
            (RunStatus::Paused, None, None),
            (
                RunStatus::Stopped,
                Some(TurnStatus::Stopped),
                Some(TurnStatus::Stopped),
            ),
            (RunStatus::BudgetStopped, None, None),
            (RunStatus::WaitingCall, None, None),
            (RunStatus::WaitingRecovery, None, None),
            (RunStatus::Completed, Some(TurnStatus::Completed), None),
            (
                RunStatus::Failed,
                Some(TurnStatus::Failed),
                Some(TurnStatus::Failed),
            ),
            (
                RunStatus::Aborted,
                Some(TurnStatus::Failed),
                Some(TurnStatus::Failed),
            ),
        ] {
            let phase = super::super::legacy_run_view::phase(status, None);
            assert_eq!(channel_turn_status(&phase, false), normal, "{status:?}");
            assert_eq!(channel_turn_status(&phase, true), assistant, "{status:?}");
        }
    }

    #[test]
    fn predecessor_checks_keep_executor_registration_and_parked_states_distinct() {
        let root = tempfile::tempdir().unwrap();
        let application = RunApplication::new(root.path().into(), root.path().join("catalog"));
        let mut record = record("predecessor");
        for (status, inactive_allowed) in [
            (RunStatus::Ready, false),
            (RunStatus::Running, false),
            (RunStatus::Paused, false),
            (RunStatus::Stopped, true),
            (RunStatus::BudgetStopped, false),
            (RunStatus::WaitingCall, false),
            (RunStatus::WaitingRecovery, false),
            (RunStatus::Completed, true),
            (RunStatus::Failed, true),
            (RunStatus::Aborted, true),
        ] {
            record.status = status;
            for registered in [false, true] {
                let mut active = HashMap::new();
                if registered {
                    active.insert(
                        record.run_id.clone(),
                        ActiveRun::new(root.path().into(), new_control()),
                    );
                }
                let expected = inactive_allowed && !registered;
                assert_eq!(
                    application
                        .ensure_predecessor_settled(&record, &active)
                        .is_ok(),
                    expected,
                    "{status:?}, registered={registered}"
                );
                assert_eq!(
                    application
                        .ensure_conversation_delete_settled(&record, &active)
                        .is_ok(),
                    expected,
                    "{status:?}, registered={registered}"
                );
            }
        }
    }

    #[test]
    fn kernel_predecessor_closure_uses_authoritative_phase_and_session_turn_holds() {
        let root = tempfile::tempdir().unwrap();
        let application =
            RunApplication::new(root.path().join("state"), root.path().join("catalog"));
        let mut compatibility = record("kernel-predecessor");
        let runs = application.kernel_runs(true).unwrap().unwrap();
        runs.create(&compatibility, u64::MAX).unwrap();
        // An incoming lossy projection cannot turn an executing Kernel Run
        // into a predecessor that has stopped.
        compatibility.status = RunStatus::Completed;
        let mut active = HashMap::new();
        assert!(
            application
                .ensure_predecessor_settled(&compatibility, &active)
                .is_err()
        );
        for (reason, allowed) in [("paused", false), ("budget", false), ("stopped", true)] {
            runs.control(&compatibility.run_id, "stop", Some(reason))
                .unwrap();
            // Kernel activity comes from its worker registry, independent of
            // an old Host cancellation-forwarding registration.
            active.insert(
                compatibility.run_id.clone(),
                ActiveRun::new(root.path().into(), new_control()),
            );
            assert_eq!(
                application
                    .ensure_predecessor_settled(&compatibility, &active)
                    .is_ok(),
                allowed,
                "{reason}"
            );
            assert_eq!(
                application
                    .ensure_conversation_delete_settled(&compatibility, &active)
                    .is_ok(),
                allowed,
                "{reason}"
            );
            runs.control(&compatibility.run_id, "resume", None).unwrap();
        }
    }

    #[test]
    fn kernel_successor_retention_refreshes_authoritative_settlement() {
        let root = tempfile::tempdir().unwrap();
        let application =
            RunApplication::new(root.path().join("state"), root.path().join("catalog"));
        let mut predecessor = record("predecessor");
        predecessor.invocations.insert("work".into(), 1);
        let mut compatibility = record("kernel-successor");
        let runs = application.kernel_runs(true).unwrap().unwrap();
        runs.create(&compatibility, u64::MAX).unwrap();
        let metadata = RunMetadata::new(
            compatibility.run_id.clone(),
            "fixture".into(),
            compatibility.graph_digest.clone(),
            root.path(),
        )
        .unwrap();
        compatibility.status = RunStatus::Completed;
        assert!(
            application
                .successor_reads_predecessor(
                    &predecessor,
                    &metadata,
                    Some(&RunView::inactive(&compatibility))
                )
                .unwrap()
        );
        runs.control(&compatibility.run_id, "abandon", Some("operator"))
            .unwrap();
        compatibility.status = RunStatus::Ready;
        assert!(
            !application
                .successor_reads_predecessor(
                    &predecessor,
                    &metadata,
                    Some(&RunView::inactive(&compatibility))
                )
                .unwrap()
        );
    }

    #[test]
    fn successor_retention_keeps_missing_and_first_invocation_evidence_distinct() {
        let root = tempfile::tempdir().unwrap();
        let application = RunApplication::new(root.path().into(), root.path().join("catalog"));
        let mut predecessor = record("predecessor");
        predecessor.invocations.insert("work".into(), 1);
        let mut successor = record("successor");
        let metadata = RunMetadata::new(
            successor.run_id.clone(),
            "fixture".into(),
            successor.graph_digest.clone(),
            root.path(),
        )
        .unwrap();
        assert!(
            application
                .successor_reads_predecessor(&predecessor, &metadata, None)
                .unwrap()
        );
        for (status, retained) in [
            (RunStatus::Ready, true),
            (RunStatus::Running, true),
            (RunStatus::Paused, true),
            (RunStatus::Stopped, true),
            (RunStatus::BudgetStopped, true),
            (RunStatus::WaitingCall, true),
            (RunStatus::WaitingRecovery, true),
            (RunStatus::Completed, false),
            (RunStatus::Failed, false),
            (RunStatus::Aborted, false),
        ] {
            successor.status = status;
            assert_eq!(
                application
                    .successor_reads_predecessor(
                        &predecessor,
                        &metadata,
                        Some(&RunView::inactive(&successor))
                    )
                    .unwrap(),
                retained,
                "{status:?}"
            );
        }
        successor.status = RunStatus::Running;
        successor.results.insert(
            "work".into(),
            vec![RunResult {
                node_id: "work".into(),
                key: InvocationKey {
                    run_id: successor.run_id.clone(),
                    graph_digest: successor.graph_digest.clone(),
                    node_id: "work".into(),
                    invocation: 1,
                },
                plugin_views: Default::default(),
                plugin_digests: Default::default(),
                completion: NodeCompletion {
                    submission: String::new(),
                    route: None,
                    model_requests: 0,
                    output: json!({}),
                },
                interruption: None,
                commit: CommitRef {
                    id: "first".into(),
                    node_id: "work".into(),
                    invocation: 1,
                },
                sequence: 1,
            }],
        );
        assert!(
            !application
                .successor_reads_predecessor(
                    &predecessor,
                    &metadata,
                    Some(&RunView::inactive(&successor))
                )
                .unwrap()
        );
    }
}
