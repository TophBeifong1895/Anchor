use super::*;
use crate::assistant::AssistantPlan;
use anchor_graph_host::LoadedGraphBundle;
use anchor_platform_session::{SessionError, SessionStore};

pub(crate) struct AssistantAdmission {
    pub(crate) graph: String,
    pub(crate) session: String,
    pub(crate) owner: String,
    pub(crate) oauth_owner: String,
    pub(crate) run: String,
    pub(crate) previous_run: Option<String>,
}

impl RunApplication {
    pub(crate) async fn retire_assistant(
        &self,
        owner: &str,
        session: &str,
        run: &str,
    ) -> Result<(), ApplicationError> {
        let _admission = self.admission.lock().await;
        let metadata = self.metadata(run)?.ok_or(ApplicationError::Missing)?;
        let source = metadata.assistant.ok_or(ApplicationError::Missing)?;
        if source.owner != owner || source.session != session {
            return Err(ApplicationError::Missing);
        }
        let record = self.read_run(run)?.ok_or(ApplicationError::Missing)?;
        if !record.is_quiescent() {
            return Err(ApplicationError::Conflict(
                "stop the assistant before retiring its instance".into(),
            ));
        }
        SessionStore::open(self.data_root.join("platform/sessions.sqlite"))
            .map_err(assistant_session_error)?
            .retire_channel_assistant(owner, session, run)
            .map_err(assistant_session_error)
    }

    pub(super) fn ensure_current_assistant(
        &self,
        metadata: &RunMetadata,
    ) -> Result<(), ApplicationError> {
        let Some(source) = metadata.assistant.as_ref() else {
            return Ok(());
        };
        let current = SessionStore::open(self.data_root.join("platform/sessions.sqlite"))
            .map_err(assistant_session_error)?
            .get_channel_assistant(&source.owner, &source.session)
            .map_err(assistant_session_error)?;
        if current.is_none_or(|current| {
            current.run_id != metadata.run_id
                || current.wait_node != source.wait_node
                || current.work_node != source.work_node
                || current.reply_node != source.reply_node
        }) {
            return Err(ApplicationError::Conflict(
                "this assistant instance is retired; start a new instance explicitly".into(),
            ));
        }
        self.ensure_channel_session_eligible(&source.owner, &source.session)
    }

    pub(super) fn retire_assistant_for_deletion(&self, run: &str) -> Result<(), ApplicationError> {
        let Some(source) = self.metadata(run)?.and_then(|metadata| metadata.assistant) else {
            return Ok(());
        };
        match SessionStore::open(self.data_root.join("platform/sessions.sqlite"))
            .map_err(assistant_session_error)?
            .retire_channel_assistant(&source.owner, &source.session, run)
        {
            Ok(()) | Err(SessionError::Missing) => Ok(()),
            Err(error) => Err(assistant_session_error(error)),
        }
    }

    pub(crate) async fn admit_assistant(
        &self,
        request: AssistantAdmission,
        source: &Path,
        bundle: LoadedGraphBundle,
        graph_lease: Box<dyn RunLease>,
        plan: AssistantPlan,
    ) -> Result<String, ApplicationError> {
        self.ensure_channel_session_eligible(&request.owner, &request.session)?;
        self.admit_assistant_run(request, source, bundle, graph_lease, plan)
            .await
    }
}

pub(super) fn assistant_session_error(error: SessionError) -> ApplicationError {
    match error {
        SessionError::Missing => ApplicationError::Missing,
        SessionError::Conflict(message) => ApplicationError::Conflict(message),
        SessionError::Invalid(message) => ApplicationError::Invalid(message),
        error => storage(error),
    }
}
