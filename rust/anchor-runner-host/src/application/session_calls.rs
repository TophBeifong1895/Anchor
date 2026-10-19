//! Session authority belongs to the platform host; execution belongs to this application.
use super::*;
use anchor_runtime::graph::{CallIdentity, GraphCallOutcome};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionContext {
    pub session: String,
    pub reply_node: String,
    pub conversation_id: String,
    pub channel: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionCall {
    pub context: SessionContext,
    pub status: String,
    #[serde(default)]
    pub error: String,
}

pub(crate) async fn resolve(
    identity: &CallIdentity,
    session: &str,
    graph: &str,
) -> Result<Value, GraphError> {
    let url = std::env::var("ANCHOR_SESSION_HOST_URL").map_err(|_| {
        GraphError::Unsupported("call.session requires a configured Session host".into())
    })?;
    let token = std::env::var("ANCHOR_SESSION_HOST_TOKEN").map_err(|_| {
        GraphError::Unsupported("call.session requires Session host authentication".into())
    })?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|_| GraphError::Unsupported("Session host client unavailable".into()))?;
    let response = client
        .post(format!(
            "{}/v1/runtime/session-calls/resolve",
            url.trim_end_matches('/')
        ))
        .bearer_auth(token)
        .json(&serde_json::json!({
            "parent_run":identity.parent_run_id,"graph":graph,"session":session,
        }))
        .send()
        .await
        .map_err(|_| GraphError::Unsupported("Session host unavailable".into()))?;
    if !response.status().is_success() {
        return Err(GraphError::Unsupported(
            "Session host rejected call.session authorization".into(),
        ));
    }
    let context: SessionContext = response
        .json()
        .await
        .map_err(|_| GraphError::Unsupported("Session host returned invalid context".into()))?;
    if context.session != session || context.reply_node.is_empty() || !context.channel.is_object() {
        return Err(GraphError::Unsupported(
            "Session host returned a different identity".into(),
        ));
    }
    serde_json::to_value(context).map_err(GraphError::SnapshotDecode)
}

impl RunApplication {
    pub(crate) fn reject_pending_session_delivery(
        &self,
        run_id: &str,
    ) -> Result<(), ApplicationError> {
        if self.metadata(run_id)?.is_some_and(|metadata| {
            metadata
                .session_call
                .is_some_and(|call| call.status == "pending")
        }) {
            return Err(ApplicationError::Conflict(
                "Run has pending Session execution or delivery".into(),
            ));
        }
        Ok(())
    }

    pub(super) async fn stop_wait_session_children(
        &self,
        parent: &str,
    ) -> Result<(), ApplicationError> {
        for child in self.child_metadata(parent)? {
            if child
                .graph_call
                .as_ref()
                .is_some_and(|source| source.mode == "wait")
                && child
                    .session_call
                    .as_ref()
                    .is_some_and(|call| call.status == "pending")
                && self.read_run(&child.run_id)?.is_some()
            {
                self.stop_session_call(&child.run_id).await?;
            }
        }
        Ok(())
    }

    pub(crate) fn record_session_call(
        &self,
        run_id: &str,
        context: &Value,
    ) -> Result<(), ApplicationError> {
        let context: SessionContext = serde_json::from_value(context.clone()).map_err(storage)?;
        let mut metadata = self.metadata(run_id)?.ok_or(ApplicationError::Missing)?;
        let call = SessionCall {
            context,
            status: "pending".into(),
            error: String::new(),
        };
        if let Some(existing) = &metadata.session_call {
            if existing.context != call.context {
                return Err(ApplicationError::Conflict(
                    "session call identity changed".into(),
                ));
            }
            return Ok(());
        }
        metadata.session_call = Some(call);
        metadata::save(&self.data_root, &metadata)
    }

    pub(crate) fn session_call_outcome(
        &self,
        run_id: &str,
        mode: &str,
    ) -> Result<Option<GraphCallOutcome>, ApplicationError> {
        let metadata = self.metadata(run_id)?.ok_or(ApplicationError::Missing)?;
        let call = metadata
            .session_call
            .ok_or_else(|| ApplicationError::Invalid("missing Session call context".into()))?;
        if mode == "detach" {
            return Ok(Some(GraphCallOutcome::Detached {
                child_run_id: run_id.into(),
            }));
        }
        Ok(match call.status.as_str() {
            "delivered" => None,
            "failed" => Some(GraphCallOutcome::Failed {
                child_run_id: Some(run_id.into()),
                reason: call.error,
            }),
            _ => Some(GraphCallOutcome::Waiting {
                child_run_id: run_id.into(),
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anchor_runtime::graph::GraphSnapshot;
    use serde_json::json;

    fn seed(
        application: &RunApplication,
        path: &Path,
        id: &str,
        mode: Option<&str>,
        status: RunStatus,
    ) {
        let snapshot = GraphSnapshot::from_authoring(json!({
            "entry":"work","agents":{},"ops":{"work":{"run":"true"}},
            "nodes":[{"id":"work","op":"work"}],"edges":[],
        }))
        .unwrap();
        let mut record = GraphRunRecord::create_with_id(snapshot, json!({}), id).unwrap();
        record.status = status;
        let mut meta = RunMetadata::new(
            id.into(),
            "fixture".into(),
            record.graph_digest.clone(),
            path,
        )
        .unwrap();
        if let Some(mode) = mode {
            meta.trigger_source = "graph_call".into();
            meta.graph_call = Some(metadata::GraphCallSource {
                parent_run: "parent".into(),
                parent_graph: "fixture".into(),
                parent_graph_digest: record.graph_digest.clone(),
                node: id.into(),
                invocation: 1,
                mode: mode.into(),
                root_run: "parent".into(),
            });
            meta.session_call = Some(SessionCall {
                context: SessionContext {
                    session: "alice".into(),
                    reply_node: "work".into(),
                    conversation_id: "alice".into(),
                    channel: json!({"source":"wecom","sender_id":"alice"}),
                },
                status: "pending".into(),
                error: String::new(),
            });
        }
        metadata::save(&application.data_root, &meta).unwrap();
        application.store().save(&record).unwrap();
    }

    async fn stopped_parent_cancels_wait_delivery(active_parent: bool) {
        let root = tempfile::tempdir().unwrap();
        let application = RunApplication::new(root.path().join("state"), root.path().into());
        seed(
            &application,
            root.path(),
            "parent",
            None,
            if active_parent {
                RunStatus::Running
            } else {
                RunStatus::WaitingCall
            },
        );
        seed(
            &application,
            root.path(),
            "wait-child",
            Some("wait"),
            RunStatus::Completed,
        );
        seed(
            &application,
            root.path(),
            "detach-child",
            Some("detach"),
            RunStatus::Completed,
        );
        let original = application.store().load("wait-child").unwrap().unwrap();
        if active_parent {
            application.active.lock().await.insert(
                "parent".into(),
                ActiveRun::new(root.path().into(), new_control()),
            );
        }
        application.control("parent", "stop").await.unwrap();
        assert_eq!(
            application
                .metadata("wait-child")
                .unwrap()
                .unwrap()
                .session_call
                .unwrap()
                .status,
            "failed"
        );
        assert_eq!(
            application
                .metadata("detach-child")
                .unwrap()
                .unwrap()
                .session_call
                .unwrap()
                .status,
            "pending"
        );
        assert_eq!(
            serde_json::to_value(application.store().load("wait-child").unwrap().unwrap()).unwrap(),
            serde_json::to_value(original).unwrap()
        );
        if active_parent {
            let parent = application.store().load("parent").unwrap().unwrap();
            let metadata = application.metadata("parent").unwrap();
            assert_eq!(
                application
                    .project_run(&parent, metadata.as_ref())
                    .await
                    .unwrap()
                    .phase,
                "executing"
            );
        } else {
            assert_eq!(
                application.store().load("parent").unwrap().unwrap().status,
                RunStatus::Stopped
            );
            tokio::task::yield_now().await;
            assert!(application.active_runs(None).await.is_empty());
        }
    }

    #[tokio::test]
    async fn active_parent_stop_cancels_wait_session_delivery_but_not_detach() {
        stopped_parent_cancels_wait_delivery(true).await;
    }

    #[tokio::test]
    async fn waiting_parent_stop_cancels_completed_pending_session_child_without_waking_parent() {
        stopped_parent_cancels_wait_delivery(false).await;
    }
}
