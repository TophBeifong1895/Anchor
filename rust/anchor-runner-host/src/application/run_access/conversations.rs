use super::RunView;
use crate::application::{
    ApplicationError, ChannelRunSource, ConversationAdmission, RunApplication, RunMetadata,
    WECOM_REPLY_KEY_PREFIX, WECOM_REPLY_KIND, metadata, new_control,
};
use crate::execution::PreparedExecution;
use anchor_graph_host::LoadedGraphBundle;
use anchor_kernel::{Outcome, Phase, Waiter};
use anchor_platform_session::{ChannelDeliveryRequest, SessionError, SessionStore, TurnStatus};
use anchor_runtime::graph::{GraphRunRecord, RunLease};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, path::Path};

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

    pub(crate) fn settle_channel_run(&self, run_id: &str) -> Result<(), ApplicationError> {
        let Some(metadata) = self.metadata(run_id)? else {
            return Ok(());
        };
        let view = self.read_run(run_id)?.ok_or(ApplicationError::Missing)?;
        if view.has_pending_recovery() {
            return Ok(());
        }
        let phase = view.lifecycle.phase;
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
        active: &(),
    ) -> Result<(), ApplicationError> {
        self.ensure_wait_closure_settled(record, active, false)
    }

    pub(in crate::application) fn ensure_conversation_delete_settled<'record>(
        &self,
        record: impl Into<RunView<'record>>,
        active: &(),
    ) -> Result<(), ApplicationError> {
        self.ensure_wait_closure_settled(record, active, true)
    }

    fn ensure_wait_closure_settled<'record>(
        &self,
        record: impl Into<RunView<'record>>,
        _active: &(),
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
            let worker = current.active();
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
            if turn_held || record.has_pending_recovery() || !record.is_quiescent() {
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
        let mut reading = BTreeSet::new();
        for dispatch in predecessor.dispatches() {
            // An open slot has not necessarily executed. Protect uncommitted
            // history only when the execution port has durably claimed it.
            if !matches!(
                crate::node_host::load_host_completion_fact(
                    &self.data_root.join("facts"),
                    &dispatch.key,
                )?,
                anchor_runtime::graph::CompletionFact::NotStarted
            ) || crate::goose_acp::has_invocation_fact(&self.data_root, &dispatch.key)?
            {
                reading.insert(dispatch.node_id.clone());
            }
        }
        let predecessor = predecessor.progress();
        let successor = successor.progress();
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
        let active = self.admission.lock().await;
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
        let record = GraphRunRecord::create_with_plugins(
            bundle.snapshot,
            request.input,
            request.run.clone(),
            plugin_bindings.into_values().collect(),
        )?;
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
        let lease = self.run_leases().acquire_lease(&request.run)?;
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
        self.kernel_runs(true)?
            .expect("Run store initialized")
            .create(&record, u64::MAX)?;
        drop(active);
        drop(lease);
        drop(plugin_lease);
        drop(graph_lease);
        self.spawn_kernel(record, execution, control)?;
        Ok(request.run)
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixtures;
    use super::*;
    use anchor_runtime::graph::GraphSnapshot;
    use serde_json::json;

    fn record(run_id: &str) -> GraphRunRecord {
        GraphRunRecord::create_with_id(
            GraphSnapshot::from_authoring(json!({
                "entry":"work", "agents":{}, "ops":{"work":{"run":"true"}},
                "nodes":[{"id":"work","op":"work"},{"id":"later","op":"work"}],
                "edges":[{"from":"work","to":"later"}]
            }))
            .unwrap(),
            json!({}),
            run_id,
        )
        .unwrap()
    }

    #[test]
    fn channel_settlement_preserves_assistant_completion_distinction() {
        for (phase, normal, assistant) in [
            (Phase::Executing, None, None),
            (
                Phase::Waiting {
                    waiter: Waiter::Human {
                        reason: "paused".into(),
                    },
                },
                None,
                None,
            ),
            (
                Phase::Waiting {
                    waiter: Waiter::Human {
                        reason: "budget".into(),
                    },
                },
                None,
                None,
            ),
            (
                Phase::Waiting {
                    waiter: Waiter::Human {
                        reason: "stopped".into(),
                    },
                },
                Some(TurnStatus::Stopped),
                Some(TurnStatus::Stopped),
            ),
            (
                Phase::Waiting {
                    waiter: Waiter::Child {
                        task: "child".into(),
                    },
                },
                None,
                None,
            ),
            (
                Phase::Settled {
                    result: Outcome::Done,
                },
                Some(TurnStatus::Completed),
                None,
            ),
            (
                Phase::Settled {
                    result: Outcome::Failed {
                        reason: anchor_kernel::Reason::Unclassified,
                        detail: "known failure".into(),
                    },
                },
                Some(TurnStatus::Failed),
                Some(TurnStatus::Failed),
            ),
            (
                Phase::Settled {
                    result: Outcome::Aborted {
                        detail: "operator".into(),
                    },
                },
                Some(TurnStatus::Failed),
                Some(TurnStatus::Failed),
            ),
        ] {
            assert_eq!(channel_turn_status(&phase, false), normal, "{phase:?}");
            assert_eq!(channel_turn_status(&phase, true), assistant, "{phase:?}");
        }
    }

    #[tokio::test]
    async fn predecessor_checks_keep_kernel_workers_and_parked_states_distinct() {
        let root = tempfile::tempdir().unwrap();
        let application = RunApplication::new(root.path().into(), root.path().join("catalog"));
        for id in fixtures::scenarios(&application) {
            let view = fixtures::view(&application, id);
            let expected = matches!(id, "stopped" | "done" | "failed" | "aborted");
            assert_eq!(
                application.ensure_predecessor_settled(&view, &()).is_ok(),
                expected,
                "{id}"
            );
            assert_eq!(
                application
                    .ensure_conversation_delete_settled(&view, &())
                    .is_ok(),
                expected,
                "{id}"
            );
        }
        fixtures::create(&application, &fixtures::work_record("draining"));
        let drive = fixtures::gated_drive(&application, "draining").await;
        fixtures::control(&application, "draining", "stop", "stopped");
        let view = fixtures::view(&application, "draining");
        assert!(view.active());
        assert!(application.ensure_predecessor_settled(&view, &()).is_err());
        assert!(
            application
                .ensure_conversation_delete_settled(&view, &())
                .is_err()
        );
        drive.drain(&application, "draining").await;
        assert!(application.ensure_predecessor_settled(&view, &()).is_ok());
        assert!(
            application
                .ensure_conversation_delete_settled(&view, &())
                .is_ok()
        );
    }

    #[test]
    fn kernel_predecessor_closure_uses_authoritative_phase_and_session_turn_holds() {
        let root = tempfile::tempdir().unwrap();
        let application =
            RunApplication::new(root.path().join("state"), root.path().join("catalog"));
        fixtures::create(&application, &record("kernel-predecessor"));
        let stale = fixtures::view(&application, "kernel-predecessor");
        assert!(application.ensure_predecessor_settled(&stale, &()).is_err());
        for (reason, allowed) in [("paused", false), ("budget", false), ("stopped", true)] {
            fixtures::control(&application, "kernel-predecessor", "stop", reason);
            assert_eq!(
                application.ensure_predecessor_settled(&stale, &()).is_ok(),
                allowed,
                "{reason}"
            );
            assert_eq!(
                application
                    .ensure_conversation_delete_settled(&stale, &())
                    .is_ok(),
                allowed,
                "{reason}"
            );
            fixtures::control(&application, "kernel-predecessor", "resume", "");
        }
    }

    #[test]
    fn kernel_successor_retention_refreshes_authoritative_settlement() {
        let root = tempfile::tempdir().unwrap();
        let application =
            RunApplication::new(root.path().join("state"), root.path().join("catalog"));
        fixtures::create(&application, &record("predecessor"));
        fixtures::complete(
            &application,
            "predecessor",
            "work",
            fixtures::completion(json!({})),
            None,
        );
        let predecessor = fixtures::view(&application, "predecessor");
        let successor = record("kernel-successor");
        fixtures::create(&application, &successor);
        let stale = fixtures::view(&application, "kernel-successor");
        let metadata = RunMetadata::new(
            successor.run_id.clone(),
            "fixture".into(),
            successor.graph_digest.clone(),
            root.path(),
        )
        .unwrap();
        assert!(
            application
                .successor_reads_predecessor(&predecessor, &metadata, Some(&stale))
                .unwrap()
        );
        fixtures::control(&application, "kernel-successor", "abandon", "operator");
        assert!(
            !application
                .successor_reads_predecessor(&predecessor, &metadata, Some(&stale))
                .unwrap()
        );
    }

    #[test]
    fn successor_retains_uncommitted_goose_history_but_not_unstarted_slots() {
        for (directory, version) in [("goose-acp", 2), ("goose-acp-spike", 1)] {
            let root = tempfile::tempdir().unwrap();
            let application = RunApplication::new(root.path().into(), root.path().join("catalog"));
            let snapshot = GraphSnapshot::from_authoring(json!({
                "entry":"work", "agents":{"role":{"model":"fixture","instructions":"work"}},
                "nodes":[{"id":"work","agent":"role"}], "edges":[]
            }))
            .unwrap();
            for id in ["predecessor", "successor"] {
                let record =
                    GraphRunRecord::create_with_id(snapshot.clone(), json!({}), id).unwrap();
                fixtures::create(&application, &record);
            }
            let predecessor = fixtures::view(&application, "predecessor");
            let successor = fixtures::view(&application, "successor");
            let metadata = RunMetadata::new(
                "successor".into(),
                "fixture".into(),
                successor.record.graph_digest.clone(),
                root.path(),
            )
            .unwrap();
            assert!(predecessor.record.invocations.is_empty());
            assert!(predecessor.record.results.is_empty());
            assert!(
                !application
                    .successor_reads_predecessor(&predecessor, &metadata, Some(&successor))
                    .unwrap()
            );
            let key = &predecessor.dispatches()[0].key;
            let path = root.path().join(directory).join(format!(
                "{:x}.json",
                Sha256::digest(key.durable_key().as_bytes())
            ));
            crate::create_durable_directory(path.parent().unwrap()).unwrap();
            crate::write_durable(
                &path,
                &serde_json::to_vec(&json!({
                    "version":version,"key":key,"binary_sha256":"fixture",
                    "session_id":"native-history","completion":null,"reason":null
                }))
                .unwrap(),
            )
            .unwrap();
            assert!(
                application
                    .successor_reads_predecessor(&predecessor, &metadata, Some(&successor))
                    .unwrap()
            );
            fixtures::complete(
                &application,
                "successor",
                "work",
                fixtures::completion(json!({})),
                None,
            );
            assert!(
                !application
                    .successor_reads_predecessor(&predecessor, &metadata, Some(&successor))
                    .unwrap()
            );
        }
    }

    #[test]
    fn successor_retention_keeps_missing_and_first_invocation_evidence_distinct() {
        let root = tempfile::tempdir().unwrap();
        let application = RunApplication::new(root.path().into(), root.path().join("catalog"));
        fixtures::create(&application, &record("predecessor"));
        fixtures::complete(
            &application,
            "predecessor",
            "work",
            fixtures::completion(json!({})),
            None,
        );
        let predecessor = fixtures::view(&application, "predecessor");
        let successor = record("successor");
        fixtures::create(&application, &successor);
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
        let stale = fixtures::view(&application, "successor");
        assert!(
            application
                .successor_reads_predecessor(&predecessor, &metadata, Some(&stale))
                .unwrap()
        );
        fixtures::control(&application, "successor", "stop", "paused");
        assert!(
            application
                .successor_reads_predecessor(&predecessor, &metadata, Some(&stale))
                .unwrap()
        );
        fixtures::control(&application, "successor", "resume", "");
        fixtures::complete(
            &application,
            "successor",
            "work",
            fixtures::completion(json!({"inherited":true})),
            None,
        );
        assert!(!fixtures::view(&application, "successor").is_settled());
        assert!(
            !application
                .successor_reads_predecessor(&predecessor, &metadata, Some(&stale))
                .unwrap()
        );
        fixtures::control(&application, "successor", "stop", "stopped");
        assert!(
            !application
                .successor_reads_predecessor(&predecessor, &metadata, Some(&stale))
                .unwrap()
        );
    }
}
