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
use std::path::PathBuf;

pub(crate) const REASON_OPERATOR: &str = "operator";
pub(crate) const REASON_PLUGIN_UPDATE: &str = "plugin_update";

/// Directory of durable abandon intents, relative to the Host state root.
pub(super) const ABANDON_DIR: &str = "run-abandons";
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
}

impl AbandonAction {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Abandoned => "abandoned",
            Self::AlreadyAborted => "already_aborted",
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

    pub(crate) fn abandon_intent(
        &self,
        run_id: &str,
    ) -> Result<Option<AbandonIntent>, ApplicationError> {
        if !anchor_runtime::identifier::is_name(run_id) {
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

    /// Abandon one whole Run: cancel its execution through the existing stop
    /// chain and record it as terminal `Aborted`.
    ///
    /// The request is idempotent: repeating it with the same reason returns the
    /// same result, and repeating it with a different reason is a conflict. A
    /// Run that already completed or failed cannot be abandoned. Every success,
    /// including a repeated request, waits for its execution to drain.
    pub(crate) async fn abandon_run(
        &self,
        run_id: &str,
        reason: Option<&str>,
    ) -> Result<AbandonResult, ApplicationError> {
        if !anchor_runtime::identifier::is_name(run_id) {
            return Err(ApplicationError::Invalid("invalid Run id".into()));
        }
        let reason = parse_reason(reason)?;
        let view = self.read_run(run_id)?.ok_or(ApplicationError::Missing)?;
        let recorded = self.abandon_intent(run_id)?;
        if let Some(intent) = &recorded
            && intent.reason != reason
        {
            return Err(ApplicationError::Conflict(format!(
                "Run was already abandoned for reason `{}`; requested reason `{reason}` conflicts",
                intent.reason
            )));
        }
        if let super::run_access::RunPhase::Settled { outcome, .. } = view.phase() {
            if outcome != super::run_access::RunOutcome::Aborted {
                return Err(ApplicationError::Conflict(
                    "terminal Run cannot be abandoned".into(),
                ));
            }
            // Settlement can precede the worker's final exit. Idempotency
            // preserves the accepted control without bypassing its drain.
            self.wait_abandon_drain(run_id).await?;
            return Ok(AbandonResult {
                action: if recorded.is_some() {
                    AbandonAction::Abandoned
                } else {
                    AbandonAction::AlreadyAborted
                },
                reason: recorded.as_ref().map(|intent| intent.reason.clone()),
                requested_at: recorded.as_ref().map(|intent| intent.requested_at.clone()),
            });
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
        if view.waits_for_children() {
            let children = self.abandon_wait_children(run_id)?;
            self.stop_wait_children(run_id, children).await?;
        }
        self.finish_kernel_abandon(run_id, &intent).await
    }
}
