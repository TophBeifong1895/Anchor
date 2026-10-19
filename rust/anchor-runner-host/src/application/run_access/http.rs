use crate::HostArtifacts;
use crate::application::abandon::{AbandonAction, AbandonResult};
use crate::application::{ApplicationError, RunApplication};
use serde_json::{Value, json};
use std::path::Path;

#[derive(Debug)]
pub(crate) enum RunHttpError {
    Application(ApplicationError),
    Missing,
    Internal(String),
    Unprocessable(String),
}

impl From<ApplicationError> for RunHttpError {
    fn from(error: ApplicationError) -> Self {
        Self::Application(error)
    }
}

impl RunApplication {
    pub(crate) async fn http_run_detail(
        &self,
        id: &str,
        workspace_root: &Path,
    ) -> Result<Value, RunHttpError> {
        let record = self
            .load_run(id)
            .map_err(|error| match error {
                ApplicationError::Storage(message) => RunHttpError::Internal(message),
                error => RunHttpError::Application(error),
            })?
            .ok_or(RunHttpError::Missing)?;
        let updated = self.run_updated(id)?;
        let artifacts = HostArtifacts::new(
            self.data_root.join("artifacts"),
            workspace_root.to_path_buf(),
        );
        let mut nodes = serde_json::Map::new();
        for (name, results) in &record.results {
            if let Some(result) = results.last() {
                let files = artifacts
                    .list_files(&result.commit)
                    .map_err(|error| RunHttpError::Unprocessable(error.to_string()))?;
                nodes.insert(
                    name.clone(),
                    json!({
                        "node_id":name,
                        "pass_number":result.key.invocation,
                        "submission":result.completion.submission,
                        "files":files.into_iter().map(|(path, _)| path).collect::<Vec<_>>(),
                        "submitted":result.interruption.is_none(),
                        "exit_status":result.interruption.as_ref().map(|_| "interrupted"),
                        "interruption":result.interruption,
                        "route":result.completion.route,
                        "commit":result.commit.id,
                        "inputs":[]
                    }),
                );
            }
        }
        let mut plugins = record
            .snapshot
            .nodes
            .iter()
            .map(|node| {
                let bindings = node
                    .plugins
                    .iter()
                    .filter_map(|reference| record.plugin_bindings.get(&reference.id))
                    .map(|binding| {
                        json!({
                            "id":binding.id,
                            "name":binding.id,
                            "description":"",
                            "digest":binding.digest
                        })
                    })
                    .collect::<Vec<_>>();
                (node.id.clone(), json!(bindings))
            })
            .collect::<serde_json::Map<_, _>>();
        let decided = record
            .decided
            .iter()
            .map(|(key, decision)| (key.clone(), json!([decision.selected, decision.sequence])))
            .collect::<serde_json::Map<_, _>>();
        let cursor = record
            .cursor
            .as_ref()
            .map(|cursor| json!({"node":cursor.node_id,"pass":cursor.key.invocation,"dir":""}));
        let executed = record.executed_nodes();
        let mut traces = serde_json::Map::new();
        let mut trace_keys = record
            .results
            .values()
            .flatten()
            .map(|result| result.key.clone())
            .collect::<Vec<_>>();
        if let Some(cursor) = &record.cursor {
            trace_keys.push(cursor.key.clone());
        }
        if let Some(parallel) = &record.parallel {
            trace_keys.extend(
                parallel
                    .branches
                    .iter()
                    .filter_map(|branch| branch.cursor.as_ref().map(|cursor| cursor.key.clone())),
            );
        }
        trace_keys.sort_by_key(|key| key.durable_key());
        trace_keys.dedup();
        for key in trace_keys {
            let messages = crate::goose_acp::trace_messages(&self.data_root, &key)
                .map_err(RunHttpError::Internal)?;
            if !messages.is_empty() {
                let trace_key = serde_json::to_string(&(&key.node_id, key.invocation))
                    .map_err(|error| RunHttpError::Internal(error.to_string()))?;
                traces.insert(trace_key, Value::Array(messages));
            }
        }
        let active = self.active_runs(None).await;
        let metadata = self.metadata(id)?.ok_or_else(|| {
            RunHttpError::Unprocessable("Run has no immutable Graph identity metadata".into())
        })?;
        if metadata.assistant.is_some()
            && let Some(policy) =
                crate::assistant_plugins::latest_policy(&self.data_root, &record, &metadata)
                    .map_err(RunHttpError::Internal)?
        {
            plugins.insert(
                policy.node,
                json!(
                    policy
                        .bindings
                        .values()
                        .map(|binding| json!({
                            "id":binding.id,"name":binding.id,"description":"","digest":binding.digest
                        }))
                        .collect::<Vec<_>>()
                ),
            );
        }
        let mut calls = Vec::new();
        for call in record.graph_calls.values() {
            if let Some(child_id) = &call.child_run_id {
                let child = self.load_run(child_id).map_err(RunHttpError::Application)?;
                let child_metadata = self.metadata(child_id)?;
                let source = child_metadata
                    .as_ref()
                    .and_then(|metadata| metadata.graph_call.as_ref());
                let projection = if let Some(child) = &child {
                    Some(self.project_run(child, child_metadata.as_ref()).await?)
                } else {
                    None
                };
                calls.push(json!({
                    "run":child_id,
                    "child_id":child_id,
                    "graph":call.graph,
                    "node":call.identity.node_id,
                    "op":call.identity.node_id,
                    "target":call.graph,
                    "invocation":call.identity.invocation,
                    "mode":call.mode,
                    "active":active.contains(child_id),
                    "projection":projection,
                    "root_run":source.map(|source| &source.root_run),
                }));
            }
        }
        for child_metadata in self.child_metadata(id)? {
            let Some(source) = child_metadata.graph_call.as_ref() else {
                continue;
            };
            if calls
                .iter()
                .any(|call| call["run"] == child_metadata.run_id)
            {
                continue;
            }
            let child = self
                .load_run(&child_metadata.run_id)
                .map_err(RunHttpError::Application)?;
            let projection = if let Some(child) = &child {
                Some(self.project_run(child, Some(&child_metadata)).await?)
            } else {
                None
            };
            calls.push(json!({
                "run":child_metadata.run_id,
                "child_id":child_metadata.run_id,
                "graph":child_metadata.graph,
                "node":source.node,
                "op":source.node,
                "target":child_metadata.graph,
                "invocation":source.invocation,
                "mode":source.mode,
                "active":active.contains(&child_metadata.run_id),
                "projection":projection,
                "root_run":source.root_run,
            }));
        }
        let mut trigger = json!({"source":metadata.trigger_source,"schedule":metadata.schedule,"scheduled_at":metadata.scheduled_at});
        if let Some(source) = metadata.graph_call.as_ref() {
            trigger["graph"] = json!(source.parent_graph);
            trigger["run"] = json!(source.parent_run);
            trigger["node"] = json!(source.node);
            trigger["invocation"] = json!(source.invocation);
            trigger["mode"] = json!(source.mode);
            trigger["root_run"] = json!(source.root_run);
        }
        if let Some(source) = metadata.conversation.as_ref() {
            trigger["session"] = json!(source.session);
            trigger["reply_node"] = json!(source.reply_node);
            trigger["previous_run"] = json!(source.previous_run);
        }
        if let Some(source) = metadata.channel.as_ref() {
            trigger["channel"] = json!({
                "session": source.session,
                "inbound": source.inbound,
                "turn": source.turn,
            });
        }
        let channel_reply = metadata.conversation.is_some()
            && self
                .data_root
                .join("channel-replies")
                .join(format!("{id}.json"))
                .try_exists()
                .map_err(|error| RunHttpError::Internal(error.to_string()))?;
        let projection = self.project_run(&record, Some(&metadata)).await?;
        let definition = crate::application::run_projection::run_definition(&record.snapshot);
        Ok(
            json!({"session_call":metadata.session_call,"channel_reply":channel_reply,"graph":metadata.graph,"run":id,"attachments":metadata.attachments,"plugins":plugins,"definition":definition,"state":{"objective":record.snapshot.objective,"started":metadata.created,"updated":updated,"trigger":trigger,"input":record.input,"cursor":cursor,"passes":record.passes,"decided":decided,"nodes":nodes,"executed":executed,"skipped":[],"error":record.error.unwrap_or_default(),"parallel":record.parallel,"recovery":record.recovery},"calls":calls,"traces":traces,"nodes":record.snapshot.nodes.iter().map(|node|node.id.clone()).collect::<Vec<_>>(),"active":projection.active,"projection":projection}),
        )
    }
}

pub(crate) fn abandon_response(run: &str, result: &AbandonResult) -> Value {
    json!({
        "run": run,
        "action": result.action.as_str(),
        "abandoned": matches!(result.action, AbandonAction::Abandoned),
        "reason": result.reason,
        "requested_at": result.requested_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::{RunMetadata, metadata};
    use anchor_runtime::graph::RunStore;
    use anchor_runtime::graph::{
        GraphRunRecord, InvocationKey, PendingRecovery, RecoveryAttempt, RunCursor, RunStatus,
    };
    use std::time::{Duration, UNIX_EPOCH};

    fn fixture() -> (tempfile::TempDir, RunApplication, GraphRunRecord) {
        let root = tempfile::TempDir::new().unwrap();
        let application = RunApplication::new(root.path().into(), root.path().join("catalog"));
        let record = GraphRunRecord::create_with_id(
            serde_json::from_value(json!({
                "objective":"test", "entry":"worker",
                "agents":{"role":{"model":"fixture","instructions":"work"}},
                "nodes":[{"id":"worker","agent":"role"}], "edges":[]
            }))
            .unwrap(),
            json!({"message":"test input"}),
            "http-run",
        )
        .unwrap();
        (root, application, record)
    }

    #[tokio::test]
    async fn detail_preserves_phase_json_bytes_for_each_legacy_fixture() {
        for (status, phase_fields, reason, resume, stop, abandon) in [
            (
                RunStatus::Ready,
                r#""phase":"executing""#,
                "null",
                true,
                true,
                true,
            ),
            (
                RunStatus::Running,
                r#""phase":"executing""#,
                "null",
                true,
                true,
                true,
            ),
            (
                RunStatus::Paused,
                r#""phase":"waiting","waiting_for":"human""#,
                r#""paused""#,
                true,
                false,
                true,
            ),
            (
                RunStatus::BudgetStopped,
                r#""phase":"waiting","waiting_for":"human""#,
                r#""budget""#,
                true,
                false,
                true,
            ),
            (
                RunStatus::WaitingCall,
                r#""phase":"waiting","waiting_for":"child""#,
                r#""recorded error""#,
                false,
                false,
                true,
            ),
            (
                RunStatus::WaitingRecovery,
                r#""phase":"waiting","waiting_for":"effect""#,
                r#""recorded error""#,
                false,
                false,
                true,
            ),
            (
                RunStatus::Completed,
                r#""phase":"settled","outcome":"done""#,
                "null",
                false,
                false,
                false,
            ),
            (
                RunStatus::Aborted,
                r#""phase":"settled","outcome":"aborted""#,
                r#""recorded error""#,
                false,
                false,
                false,
            ),
            (
                RunStatus::Stopped,
                r#""phase":"waiting","waiting_for":"human""#,
                r#""recorded error""#,
                true,
                false,
                true,
            ),
            (
                RunStatus::Failed,
                r#""phase":"settled","outcome":"failed""#,
                r#""recorded error""#,
                false,
                false,
                false,
            ),
        ] {
            let (root, application, mut record) = fixture();
            record.status = status;
            record.error = Some("recorded error".into());
            let mut cursor = "null";
            let mut passes = "{}";
            if matches!(
                status,
                RunStatus::BudgetStopped | RunStatus::WaitingRecovery
            ) {
                let key = InvocationKey {
                    run_id: record.run_id.clone(),
                    graph_digest: record.graph_digest.clone(),
                    node_id: "worker".into(),
                    invocation: 1,
                };
                record.invocations.insert("worker".into(), 1);
                record.passes.insert("worker".into(), 1);
                record.cursor = Some(RunCursor {
                    node_id: "worker".into(),
                    key: key.clone(),
                    input_commits: Vec::new(),
                    prepared_input: Value::Null,
                });
                cursor = r#"{"node":"worker","pass":1,"dir":""}"#;
                passes = r#"{"worker":1}"#;
                if status == RunStatus::WaitingRecovery {
                    record.recovery.push(PendingRecovery {
                        key,
                        attempt: RecoveryAttempt {
                            attempt_id: 1,
                            step: 1,
                            tool: "fixture-effect".into(),
                            started_at: "2026-10-19T00:00:00+00:00".into(),
                        },
                    });
                }
            }
            application.store().save(&record).unwrap();
            std::fs::File::open(root.path().join("runs/http-run.json"))
                .unwrap()
                .set_modified(UNIX_EPOCH + Duration::from_secs(1))
                .unwrap();
            let mut metadata = RunMetadata::new(
                record.run_id.clone(),
                "fixture".into(),
                record.graph_digest.clone(),
                root.path(),
            )
            .unwrap();
            metadata.created = "2026-10-19T00:00:00+00:00".into();
            metadata::save(root.path(), &metadata).unwrap();
            let projection = format!(
                r#"{{{phase_fields},"reason":{reason},"active":false,"actions":{{"pause":false,"stop":{stop},"resume":{resume},"abandon":{abandon}}},"updated":"1970-01-01T00:00:01+00:00"}}"#
            );
            let recovery = serde_json::to_string(&record.recovery).unwrap();
            let expected = format!(
                r#"{{"session_call":null,"channel_reply":false,"graph":"fixture","run":"http-run","attachments":[],"plugins":{{"worker":[]}},"definition":{{"entry":"worker","nodes":[{{"id":"worker","plugins":[],"agent":"role"}}],"edges":[],"ops":{{}}}},"state":{{"objective":"test","started":"2026-10-19T00:00:00+00:00","updated":"1970-01-01T00:00:01+00:00","trigger":{{"source":"manual","schedule":null,"scheduled_at":null}},"input":{{"message":"test input"}},"cursor":{cursor},"passes":{passes},"decided":{{}},"nodes":{{}},"executed":[],"skipped":[],"error":"recorded error","parallel":null,"recovery":{recovery}}},"calls":[],"traces":{{}},"nodes":["worker"],"active":false,"projection":{projection}}}"#
            );
            let detail = application
                .http_run_detail(&record.run_id, &root.path().join("workspaces"))
                .await
                .unwrap();
            assert_eq!(
                serde_json::to_vec(&detail).unwrap(),
                expected.as_bytes(),
                "{status:?}"
            );
        }
    }

    #[test]
    fn abandon_response_preserves_phase_contract_bytes_and_action_distinctions() {
        for (action, wire, abandoned) in [
            (AbandonAction::Abandoned, "abandoned", true),
            (AbandonAction::AlreadyAborted, "already_aborted", false),
            (AbandonAction::Abandoning, "abandoning", false),
        ] {
            let result = AbandonResult {
                action,
                status: RunStatus::Aborted,
                reason: Some("operator".into()),
                requested_at: Some("2026-10-19T00:00:00+00:00".into()),
            };
            let expected = format!(
                r#"{{"run":"http-run","action":"{wire}","abandoned":{abandoned},"reason":"operator","requested_at":"2026-10-19T00:00:00+00:00"}}"#
            );
            assert_eq!(
                serde_json::to_vec(&abandon_response("http-run", &result)).unwrap(),
                expected.as_bytes()
            );
        }
    }

    #[tokio::test]
    async fn detail_keeps_store_and_application_errors_distinct_and_ordered() {
        let (root, application, record) = fixture();
        let workspace = root.path().join("workspaces");
        assert!(matches!(
            application
                .http_run_detail(&record.run_id, &workspace)
                .await,
            Err(RunHttpError::Missing)
        ));
        application.store().save(&record).unwrap();
        assert!(matches!(
            application.http_run_detail(&record.run_id, &workspace).await,
            Err(RunHttpError::Unprocessable(message))
                if message == "Run has no immutable Graph identity metadata"
        ));
        std::fs::create_dir_all(root.path().join("run-metadata")).unwrap();
        std::fs::write(root.path().join("run-metadata/http-run.json"), "{}").unwrap();
        assert!(matches!(
            application
                .http_run_detail(&record.run_id, &workspace)
                .await,
            Err(RunHttpError::Application(ApplicationError::Storage(_)))
        ));
        std::fs::write(root.path().join("runs/http-run.json"), "{}").unwrap();
        let expected = application
            .store()
            .load(&record.run_id)
            .unwrap_err()
            .to_string();
        assert!(matches!(
            application.http_run_detail(&record.run_id, &workspace).await,
            Err(RunHttpError::Internal(message)) if message == expected
        ));
    }
}
