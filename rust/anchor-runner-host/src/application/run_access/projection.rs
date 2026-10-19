use super::{RunView, legacy_status, validate_control_identity};
use crate::application::{ApplicationError, RunApplication, RunMetadata};
use anchor_runtime::graph::{GraphRunRecord, RunStatus};
use serde::Serialize;
use serde_json::{Value, json};
use std::sync::atomic::Ordering;

pub(crate) fn projected_run_is_unfinished(run: &Value) -> bool {
    run.get("status")
        .filter(|status| status.is_string())
        .and_then(|status| serde_json::from_value::<RunStatus>(status.clone()).ok())
        .is_some_and(|status| !super::is_settled(status))
}

#[derive(Debug, Serialize)]
pub(crate) struct RunActions {
    pub(crate) pause: bool,
    pub(crate) stop: bool,
    pub(crate) resume: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct RunProjection {
    pub(crate) phase: String,
    pub(crate) active: bool,
    pub(crate) control_requested: Option<&'static str>,
    pub(crate) terminal: bool,
    pub(crate) quiescent: bool,
    pub(crate) needs_attention: bool,
    pub(crate) actions: RunActions,
}

impl RunApplication {
    pub(crate) fn pilot_session_runs(
        &self,
        run_ids: &[String],
    ) -> Result<Vec<Value>, ApplicationError> {
        Ok(self
            .raw_records()?
            .into_iter()
            .filter(|(id, _)| run_ids.contains(id))
            .map(|(id, record)| json!({"run":id,"record":record}))
            .collect())
    }

    pub(crate) async fn project_run<'record>(
        &self,
        record: impl Into<RunView<'record>>,
        metadata: Option<&RunMetadata>,
    ) -> Result<RunProjection, ApplicationError> {
        let view = record.into();
        let record = view.record.as_ref();
        let active = self.active.lock().await;
        let executor = active.get(&record.run_id);
        let requested = executor.and_then(|run| {
            if run.control.cancellation.load(Ordering::Relaxed) {
                Some("stop")
            } else if run.control.pause.load(Ordering::Relaxed) {
                Some("pause")
            } else {
                None
            }
        });
        let session_coordinated = metadata.is_some_and(|value| value.session_call.is_some());
        let mut projection = observed(record, executor.is_some(), requested, session_coordinated);
        let identity =
            metadata.filter(|metadata| validate_control_identity(record, metadata).is_ok());
        if !projection.active && !session_coordinated && identity.is_none() {
            projection.actions.stop = false;
            projection.actions.resume = false;
        }
        if projection.actions.resume {
            projection.actions.resume = identity
                .is_some_and(|metadata| self.ensure_current_assistant(metadata).is_ok())
                && self.abandon_intent(&record.run_id)?.is_none()
                && self
                    .reject_superseded_conversation_execution(&record.run_id)
                    .is_ok();
        }
        Ok(projection)
    }

    pub(crate) async fn projected_runs(&self) -> Result<Vec<Value>, ApplicationError> {
        let mut runs = Vec::new();
        for (id, record) in self.raw_records()? {
            let updated = self.run_updated(&id)?;
            let metadata = self.metadata(&id)?;
            let (graph, created, mut trigger) = metadata
                .as_ref()
                .map(|metadata| (
                    metadata.graph.clone(),
                    metadata.created.clone(),
                    json!({"source":metadata.trigger_source,"schedule":metadata.schedule,"scheduled_at":metadata.scheduled_at}),
                ))
                .unwrap_or_else(|| (
                    "unknown".into(),
                    String::new(),
                    json!({"source":"unknown"}),
                ));
            if let Some(source) = metadata
                .as_ref()
                .and_then(|metadata| metadata.graph_call.as_ref())
            {
                trigger["graph"] = json!(source.parent_graph);
                trigger["run"] = json!(source.parent_run);
                trigger["node"] = json!(source.node);
                trigger["invocation"] = json!(source.invocation);
                trigger["mode"] = json!(source.mode);
                trigger["root_run"] = json!(source.root_run);
            }
            if let Some(source) = metadata
                .as_ref()
                .and_then(|metadata| metadata.conversation.as_ref())
            {
                trigger["session"] = json!(source.session);
                trigger["reply_node"] = json!(source.reply_node);
                trigger["previous_run"] = json!(source.previous_run);
            }
            if let Some(source) = metadata
                .as_ref()
                .and_then(|metadata| metadata.channel.as_ref())
            {
                trigger["channel"] = json!({
                    "session": source.session,
                    "inbound": source.inbound,
                    "turn": source.turn,
                });
            }
            let view = RunView::inactive(&record);
            let progress = view.progress();
            let executed = progress.executed_nodes();
            let projection = self.project_run(&record, metadata.as_ref()).await?;
            runs.push(json!({"session_call":metadata.as_ref().and_then(|m| m.session_call.as_ref()),"run":id,"graph":graph,"status":legacy_status(record.status),"running":projection.active,"projection":projection,"started":created,"updated":updated,"executed":executed,"objective":progress.snapshot.objective,"trigger":trigger}));
        }
        Ok(runs)
    }

    pub(crate) async fn pilot_run_list(&self) -> Result<Value, ApplicationError> {
        let mut runs = Vec::new();
        for (id, record) in self.raw_records()? {
            let metadata = self.metadata(&id)?;
            let projection = self.project_run(&record, metadata.as_ref()).await?;
            runs.push(
                json!({"run":id,"status":legacy_status(record.status),"projection":projection}),
            );
        }
        Ok(json!({"runs":runs}))
    }

    pub(crate) async fn pilot_run_status(&self, run: &str) -> Result<Value, ApplicationError> {
        let record = self
            .raw_records()?
            .into_iter()
            .find(|(id, _)| id == run)
            .ok_or(ApplicationError::Missing)?
            .1;
        let metadata = self.metadata(run)?;
        let projection = self.project_run(&record, metadata.as_ref()).await?;
        Ok(json!({"run":run,"record":record,"projection":projection}))
    }
}

pub(in crate::application) fn observed(
    record: &GraphRunRecord,
    active: bool,
    requested: Option<&'static str>,
    session_coordinated: bool,
) -> RunProjection {
    let view = RunView::observed(record, active, requested, session_coordinated);
    RunProjection {
        needs_attention: view.needs_attention(),
        phase: view.legacy_phase().into(),
        active: view.active(),
        control_requested: requested,
        terminal: view.is_settled(),
        quiescent: view.is_quiescent(),
        actions: RunActions {
            pause: view.can_pause(),
            stop: view.can_stop(),
            resume: view.can_resume(),
        },
    }
}
