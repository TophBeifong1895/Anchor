use super::RunView;
use crate::application::{
    ActiveRun, ApplicationError, RunApplication, RunMetadata,
    assistants::{AssistantAdmission, assistant_session_error},
    metadata::{self, AssistantSource, ConversationSource},
    new_control, storage,
};
use crate::{HostArtifacts, assistant::AssistantPlan, execution::PreparedExecution};
use anchor_graph_host::LoadedGraphBundle;
use anchor_platform_session::SessionStore;
use anchor_runtime::graph::{GraphRunRecord, RunLease, RunStore};
use std::{collections::HashMap, path::Path};

impl RunView<'_> {
    pub(in crate::application) fn delete_owned_run_data(
        &self,
        data_root: &Path,
        workspace_root: &Path,
    ) -> Result<(), String> {
        crate::run_data::delete_run_data(&self.record, data_root, workspace_root)
    }
}

impl RunApplication {
    pub(in crate::application) async fn admit_assistant_run(
        &self,
        request: AssistantAdmission,
        source: &Path,
        bundle: LoadedGraphBundle,
        graph_lease: Box<dyn RunLease>,
        plan: AssistantPlan,
    ) -> Result<String, ApplicationError> {
        let AssistantAdmission {
            graph,
            session,
            owner,
            oauth_owner,
            run,
            previous_run,
        } = request;
        let plugin_lease = if bundle.plugins.is_empty() {
            None
        } else {
            Some(self.plugin_reference_lease().await?)
        };
        let mut active = self.active.lock().await;
        let graph_path = Self::graph_identity(source)?;
        if let Some(saved) = self.metadata(&run)? {
            let binding = saved.assistant.ok_or_else(|| {
                ApplicationError::Conflict("Run is not an assistant instance".into())
            })?;
            if binding.owner != owner
                || binding.session != session
                || saved.bundle_source != graph_path
            {
                return Err(ApplicationError::Conflict(
                    "assistant instance identity changed".into(),
                ));
            }
            let Some(record) = self.read_run(&run)? else {
                return Err(ApplicationError::Conflict(
                    "assistant admission is incomplete; inspect retained facts".into(),
                ));
            };
            if record.progress().graph_digest != saved.graph_digest {
                return Err(ApplicationError::Invalid(
                    "assistant admission Graph identity changed".into(),
                ));
            }
            SessionStore::open(self.data_root.join("platform/sessions.sqlite"))
                .map_err(assistant_session_error)?
                .bind_channel_assistant(
                    &owner,
                    &session,
                    &run,
                    &binding.wait_node,
                    &binding.work_node,
                    &binding.reply_node,
                )
                .map_err(assistant_session_error)?;
            return Ok(run);
        }
        for (run_id, record) in self.run_views()? {
            if !record.is_settled() {
                let metadata = self.metadata(&run_id)?.ok_or_else(|| {
                    ApplicationError::Conflict("unfinished Run has no immutable identity".into())
                })?;
                if !record.has_stopped_execution()
                    && metadata
                        .assistant
                        .as_ref()
                        .is_some_and(|source| source.owner == owner && source.session == session)
                {
                    return Err(ApplicationError::Conflict(format!(
                        "assistant admission retains unfinished Run `{}`; resume that instance before creating another",
                        run_id
                    )));
                }
                if Self::graph_identity(&metadata.bundle_source)? == graph_path
                    && metadata.conversation.is_none()
                    && metadata.session_call.is_none()
                {
                    return Err(ApplicationError::Conflict(
                        "Graph has an unfinished non-conversation Run".into(),
                    ));
                }
            }
        }
        if let Some(previous) = previous_run.as_deref() {
            let record = self.read_run(previous)?.ok_or(ApplicationError::Missing)?;
            self.ensure_predecessor_settled(&record, &active)?;
            let metadata = self.metadata(previous)?.ok_or(ApplicationError::Missing)?;
            if metadata.conversation.as_ref().is_none_or(|binding| {
                binding.session != session || binding.reply_node != plan.work_node
            }) || metadata.bundle_source != graph_path
            {
                return Err(ApplicationError::Conflict(
                    "assistant predecessor belongs to another conversation".into(),
                ));
            }
        }
        let plugin_bindings = self.bundle_plugin_bindings(&bundle)?;
        let mut record = GraphRunRecord::create_with_id(
            bundle.snapshot,
            serde_json::json!({"session":session}),
            run.clone(),
        )?;
        record.plugin_bindings = plugin_bindings;
        record.plugin_bindings_initialized = true;
        let mut metadata = RunMetadata::new(
            run.clone(),
            graph.clone(),
            record.graph_digest.clone(),
            source,
        )?;
        metadata.trigger_source = "channel".into();
        metadata.oauth_owner = Some(oauth_owner);
        metadata.conversation = Some(ConversationSource {
            session: session.clone(),
            reply_node: plan.work_node.clone(),
            previous_run,
        });
        metadata.assistant = Some(AssistantSource {
            owner: owner.clone(),
            session: session.clone(),
            wait_node: plan.wait_node.clone(),
            work_node: plan.work_node.clone(),
            reply_node: plan.reply_node.clone(),
        });
        let lease = self.store().acquire_lease(&run)?;
        metadata::save(&self.data_root, &metadata)?;
        let artifacts = HostArtifacts::new(
            self.data_root.join("artifacts"),
            crate::env_path("ANCHOR_RUNNER_WORKSPACE_ROOT").map_err(ApplicationError::Invalid)?,
        );
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
            .bind_local_inputs(&record, &graph)
            .map_err(ApplicationError::Invalid)?;
        for node in &record.snapshot.nodes {
            artifacts.bind_node_workspace(&run, &record.graph_digest, &node.id)?;
        }
        self.store().save(&record)?;
        SessionStore::open(self.data_root.join("platform/sessions.sqlite"))
            .map_err(storage)?
            .bind_channel_assistant(
                &owner,
                &session,
                &run,
                &plan.wait_node,
                &plan.work_node,
                &plan.reply_node,
            )
            .map_err(storage)?;
        active.insert(run.clone(), ActiveRun::new(graph_path, control));
        drop(lease);
        drop(plugin_lease);
        drop(graph_lease);
        self.spawn(record, execution)?;
        Ok(run)
    }

    pub(in crate::application) fn delete_inactive_run(
        &self,
        run_id: &str,
        workspace_root: &Path,
        active: &HashMap<String, ActiveRun>,
    ) -> Result<(), ApplicationError> {
        let store = self.store();
        let record = store.load(run_id)?.ok_or(ApplicationError::Missing)?;
        let metadata = self.metadata(run_id)?;
        self.reject_pending_session_delivery(run_id)?;
        if let Some(metadata) = metadata.as_ref() {
            self.ensure_channel_delivery_settled(metadata)?;
        }
        match metadata
            .as_ref()
            .filter(|metadata| metadata.conversation.is_some())
        {
            // A conversation Run may only go once every later Run has stopped
            // reading it; see `ensure_conversation_delete_allowed`.
            Some(metadata) => {
                self.ensure_conversation_delete_allowed(&record, metadata, active)?;
            }
            None => self.reject_conversation_successor(run_id)?,
        }
        if !record.graph_calls.is_empty() {
            return Err(ApplicationError::Conflict(
                "Run is referenced by or owns a Graph call".into(),
            ));
        }
        for (other_id, other) in self.raw_records()? {
            if other_id != run_id
                && other.graph_calls.values().any(|call| {
                    call.child_run_id.as_deref() == Some(run_id)
                        && call.status != anchor_runtime::graph::GraphCallStatus::Deleted
                })
            {
                return Err(ApplicationError::Conflict(
                    "Run is referenced by another Graph call".into(),
                ));
            }
        }
        let _lease = store.acquire_lease(run_id)?;
        let artifacts = crate::HostArtifacts::new(
            self.data_root.join("artifacts"),
            workspace_root.to_path_buf(),
        );
        for result in record.results.values().flatten() {
            if self
                .data_root
                .join("artifacts")
                .join(&result.commit.id)
                .exists()
            {
                artifacts
                    .list_files(&result.commit)
                    .map_err(|error| ApplicationError::Invalid(error.to_string()))?;
            }
            if result.key.run_id != run_id
                || result.key.graph_digest != record.graph_digest
                || result.commit.node_id != result.key.node_id
                || result.commit.invocation != result.key.invocation
            {
                return Err(ApplicationError::Invalid(
                    "Run artifact identity mismatch".into(),
                ));
            }
        }
        // Durable before anything is removed: a Host that dies in the middle of
        // this cleanup leaves an explained lineage gap, not corruption.
        if let Some(metadata) = metadata.as_ref()
            && let Some(source) = metadata.conversation.as_ref()
        {
            crate::conversation_resources::retain(&self.data_root, &record, metadata)?;
            crate::run_deletions::save(
                &self.data_root,
                &crate::run_deletions::RunDeletion::new(
                    run_id,
                    &Self::graph_identity(&metadata.bundle_source)?.to_string_lossy(),
                    &source.session,
                    &source.reply_node,
                    source.previous_run.as_deref(),
                ),
            )
            .map_err(ApplicationError::Storage)?;
            crate::conversation_resources::release_unreferenced(
                self,
                &self.data_root,
                &Self::graph_identity(&metadata.bundle_source)?,
                run_id,
                workspace_root,
            )?;
        }
        crate::run_data::delete_run_data(&record, &self.data_root, workspace_root)
            .map_err(ApplicationError::Storage)?;
        crate::run_data::remove_run_files(&self.data_root, run_id)
            .map_err(ApplicationError::Storage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anchor_runtime::graph::{
        CallIdentity, GraphCallRecord, GraphCallStatus, GraphSnapshot, RunStatus,
    };
    use serde_json::{Value, json};

    fn record(run_id: &str) -> GraphRunRecord {
        let snapshot = GraphSnapshot::from_authoring(json!({
            "entry":"call", "agents":{},
            "ops":{"call":{"call":{"graph":"child","mode":"wait"}}},
            "nodes":[{"id":"call","op":"call"}], "edges":[]
        }))
        .unwrap();
        GraphRunRecord::create_with_id(snapshot, Value::Null, run_id).unwrap()
    }

    #[tokio::test]
    async fn graph_cascade_allows_missing_wait_child_but_handover_refuses() {
        let root = tempfile::tempdir().unwrap();
        let application = RunApplication::new(root.path().join("state"), root.path().into());
        let mut parent = record("parent");
        parent.status = RunStatus::Stopped;
        application.store().save(&parent).unwrap();
        let child = record("missing-child");
        let metadata = RunMetadata::graph_call_child(
            child.run_id.clone(),
            "graph".into(),
            child.graph_digest,
            root.path(),
            metadata::GraphCallSource {
                parent_run: parent.run_id.clone(),
                parent_graph: "graph".into(),
                parent_graph_digest: parent.graph_digest.clone(),
                node: "call".into(),
                invocation: 1,
                mode: "wait".into(),
                root_run: parent.run_id.clone(),
            },
        )
        .unwrap();
        metadata::save(&application.data_root, &metadata).unwrap();
        let active = application.active.lock().await;
        let view = application.read_run(&parent.run_id).unwrap().unwrap();
        application
            .ensure_conversation_delete_settled(&view, &active)
            .unwrap();
        assert!(matches!(
            application.ensure_predecessor_settled(&view, &active),
            Err(ApplicationError::Conflict(reason))
                if reason == "previous conversation has a missing wait child"
        ));
        assert_eq!(
            application.store().load(&parent.run_id).unwrap(),
            Some(parent)
        );
        assert!(application.read_run("missing-child").unwrap().is_none());
    }

    #[tokio::test]
    async fn individual_delete_does_not_require_an_inactive_unreferenced_run_to_settle() {
        let root = tempfile::tempdir().unwrap();
        let application = RunApplication::new(root.path().join("state"), root.path().into());
        for status in [
            RunStatus::Ready,
            RunStatus::Running,
            RunStatus::Paused,
            RunStatus::WaitingCall,
            RunStatus::Stopped,
            RunStatus::Completed,
            RunStatus::Failed,
            RunStatus::Aborted,
        ] {
            let mut record = record("individual");
            record.status = status;
            application.store().save(&record).unwrap();
            application
                .delete_run(&record.run_id, root.path())
                .await
                .unwrap();
            assert!(
                application.read_run(&record.run_id).unwrap().is_none(),
                "{status:?}"
            );
        }
    }

    #[tokio::test]
    async fn deleted_graph_call_marker_only_relaxes_the_incoming_reference() {
        let root = tempfile::tempdir().unwrap();
        let application = RunApplication::new(root.path().join("state"), root.path().into());
        let target = record("target");
        application.store().save(&target).unwrap();
        let mut parent = record("parent");
        parent.status = RunStatus::Stopped;
        for status in [
            GraphCallStatus::Prepared,
            GraphCallStatus::Waiting,
            GraphCallStatus::Completed,
            GraphCallStatus::Detached,
            GraphCallStatus::Failed,
            GraphCallStatus::Uncertain,
            GraphCallStatus::Deleted,
        ] {
            let call = GraphCallRecord {
                identity: CallIdentity {
                    parent_run_id: parent.run_id.clone(),
                    parent_graph_digest: parent.graph_digest.clone(),
                    node_id: "call".into(),
                    invocation: 1,
                    call_spec_digest: "fixture".into(),
                },
                graph: "child".into(),
                child_run_id: Some(target.run_id.clone()),
                mode: "wait".into(),
                status,
                output: None,
                error: None,
            };
            parent.graph_calls.insert(call.identity.durable_key(), call);
            application.store().save(&parent).unwrap();
            let deleted = application.delete_run(&target.run_id, root.path()).await;
            if status == GraphCallStatus::Deleted {
                deleted.unwrap();
                assert!(application.read_run(&target.run_id).unwrap().is_none());
            } else {
                assert!(
                    matches!(
                        deleted,
                        Err(ApplicationError::Conflict(reason))
                            if reason == "Run is referenced by another Graph call"
                    ),
                    "{status:?}"
                );
                assert_eq!(
                    application.store().load(&target.run_id).unwrap(),
                    Some(target.clone())
                );
            }
            assert_eq!(
                application.store().load(&parent.run_id).unwrap(),
                Some(parent.clone())
            );
        }
        assert!(matches!(
            application.delete_run(&parent.run_id, root.path()).await,
            Err(ApplicationError::Conflict(reason))
                if reason == "Run is referenced by or owns a Graph call"
        ));
        assert_eq!(
            application.store().load(&parent.run_id).unwrap(),
            Some(parent)
        );
    }
}
