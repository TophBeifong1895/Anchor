//! Explicit operator abandonment of a whole Run.
//!
//! `stop` parks a Run in `Stopped`, which stays resumable and therefore still
//! counts as unfinished when a Graph's Plugin resources are replaced. `abandon`
//! is the one-way control action for that case: it cancels in-flight node
//! execution through the ordinary stop chain, then records the Run as terminal
//! `Aborted` with an audited reason.
//!
//! Nothing is deleted. The frozen snapshot, results, recovery facts, workspace,
//! artifacts, Session Turn history and the assistant binding all stay exactly
//! as they were, so a channel instance can still be handed over to a new Run by
//! the existing (automatic or explicit) recovery path.

pub(crate) use super::run_access::AbandonResult;
use super::{ApplicationError, RunApplication, storage};
use crate::assistant::{read_json, save_immutable};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::atomic::Ordering, time::Duration};

pub(crate) const REASON_OPERATOR: &str = "operator";
pub(crate) const REASON_PLUGIN_UPDATE: &str = "plugin_update";

/// Directory of durable abandon intents, relative to the Host state root.
pub(super) const ABANDON_DIR: &str = "run-abandons";
/// How long an abandon request waits for an active Run to observe cancellation
/// before reporting that the terminal transition is still in flight.
const CANCEL_POLL: Duration = Duration::from_millis(50);
const CANCEL_POLLS: usize = 600;

/// Durable, immutable record of one accepted abandon request.
///
/// Its only roles are auditability, idempotency, and telling a completion or a
/// restart that this Run must become terminal `Aborted`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AbandonIntent {
    format: u32,
    pub(crate) run_id: String,
    pub(crate) reason: String,
    pub(crate) requested_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AbandonAction {
    /// This Run is durably `Aborted` because of a recorded abandon request.
    Abandoned,
    /// The Run was already terminal `Aborted` without a recorded abandon
    /// request (for example a node-level recovery abort).
    AlreadyAborted,
    /// Cancellation was accepted but the Run had not released its execution
    /// slot yet; the recorded intent makes it terminal without further action.
    Abandoning,
}

impl AbandonAction {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Abandoned => "abandoned",
            Self::AlreadyAborted => "already_aborted",
            Self::Abandoning => "abandoning",
        }
    }
}

fn parse_reason(reason: Option<&str>) -> Result<String, ApplicationError> {
    match reason {
        None => Ok(REASON_OPERATOR.into()),
        Some(value) if value == REASON_OPERATOR || value == REASON_PLUGIN_UPDATE => {
            Ok(value.into())
        }
        Some(_) => Err(ApplicationError::Invalid(format!(
            "unknown abandon reason; expected `{REASON_OPERATOR}` or `{REASON_PLUGIN_UPDATE}`"
        ))),
    }
}

impl RunApplication {
    fn abandon_intent_path(&self, run_id: &str) -> PathBuf {
        self.data_root
            .join(ABANDON_DIR)
            .join(format!("{run_id}.json"))
    }

    /// A Run id that is safe to use as one path component.
    pub(crate) fn valid_run_component(run_id: &str) -> bool {
        !run_id.is_empty()
            && run_id != "."
            && run_id != ".."
            && run_id
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "-_.".contains(character))
    }

    pub(crate) fn abandon_intent(
        &self,
        run_id: &str,
    ) -> Result<Option<AbandonIntent>, ApplicationError> {
        if !Self::valid_run_component(run_id) {
            return Err(ApplicationError::Invalid("invalid Run id".into()));
        }
        let intent =
            read_json::<AbandonIntent>(&self.abandon_intent_path(run_id)).map_err(storage)?;
        if let Some(intent) = &intent
            && (intent.format != 1 || intent.run_id != run_id)
        {
            return Err(ApplicationError::Storage(
                "abandon intent identity changed".into(),
            ));
        }
        Ok(intent)
    }

    async fn request_active_cancellation(&self, run_id: &str) {
        let active = self.active.lock().await;
        if let Some(run) = active.get(run_id) {
            run.control.cancellation.store(true, Ordering::Release);
        }
    }

    /// Abandon one whole Run: cancel its execution through the existing stop
    /// chain and record it as terminal `Aborted`.
    ///
    /// The request is idempotent: repeating it with the same reason returns the
    /// same result, and repeating it with a different reason is a conflict. A
    /// Run that already completed or failed cannot be abandoned.
    pub(crate) async fn abandon_run(
        &self,
        run_id: &str,
        reason: Option<&str>,
    ) -> Result<AbandonResult, ApplicationError> {
        if !Self::valid_run_component(run_id) {
            return Err(ApplicationError::Invalid("invalid Run id".into()));
        }
        let reason = parse_reason(reason)?;
        let record = self.read_run(run_id)?.ok_or(ApplicationError::Missing)?;
        let recorded = self.abandon_intent(run_id)?;
        if let Some(intent) = &recorded
            && intent.reason != reason
        {
            return Err(ApplicationError::Conflict(format!(
                "Run was already abandoned for reason `{}`; requested reason `{reason}` conflicts",
                intent.reason
            )));
        }
        if let Some(result) = record.existing_abandon_result(recorded.as_ref())? {
            return Ok(result);
        }
        // Durable before any cancellation: a Host that dies mid-request still
        // makes this Run terminal on its next start.
        let intent = match recorded {
            Some(intent) => intent,
            None => {
                let intent = AbandonIntent {
                    format: 1,
                    run_id: run_id.into(),
                    reason: reason.clone(),
                    requested_at: chrono::DateTime::<chrono::Utc>::from(
                        std::time::SystemTime::now(),
                    )
                    .to_rfc3339(),
                };
                save_immutable(&self.abandon_intent_path(run_id), &intent).map_err(storage)?;
                intent
            }
        };
        // Session calls own a delivery lifecycle: settle it through its own
        // chain first so a waiting parent is released exactly as a stop does.
        if self
            .metadata(run_id)?
            .is_some_and(|metadata| metadata.session_call.is_some())
        {
            match self.stop_session_call(run_id).await {
                Ok(()) | Err(ApplicationError::Conflict(_)) => {}
                Err(error) => return Err(error),
            }
        }
        let mut polls = 0;
        while self.run_is_active(run_id).await {
            self.request_active_cancellation(run_id).await;
            if polls >= CANCEL_POLLS {
                let record = self.read_run(run_id)?.ok_or(ApplicationError::Missing)?;
                return Ok(record.abandoning_result(&intent));
            }
            polls += 1;
            tokio::time::sleep(CANCEL_POLL).await;
        }
        let record = self.read_run(run_id)?.ok_or(ApplicationError::Missing)?;
        if record.waits_for_children() {
            // Nothing executes this Run any more, but its waiting children can.
            // Stop them exactly as `stop` does before this Run turns terminal.
            let wait_children = record.abandon_wait_children();
            self.stop_wait_children(run_id, wait_children).await?;
        }
        self.finish_abandon(run_id, intent).await
    }
}
