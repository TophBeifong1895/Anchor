use super::{RunView, wait_child_ids};
use crate::application::{
    ApplicationError, RunApplication,
    abandon::{ABANDON_DIR, AbandonAction, AbandonIntent},
    storage,
};
use anchor_runtime::graph::{RunStatus, RunStore};
use std::borrow::Cow;

#[derive(Debug, Clone)]
pub(crate) struct AbandonResult {
    pub(crate) action: AbandonAction,
    pub(in crate::application::run_access) status: RunStatus,
    pub(crate) reason: Option<String>,
    pub(crate) requested_at: Option<String>,
}

impl RunView<'_> {
    fn is_abandoned(&self) -> bool {
        self.is_settled() && self.can_abandon()
    }

    pub(crate) fn existing_abandon_result(
        &self,
        recorded: Option<&AbandonIntent>,
    ) -> Result<Option<AbandonResult>, ApplicationError> {
        if !self.can_abandon() {
            return Err(ApplicationError::Conflict(
                "terminal Run cannot be abandoned".into(),
            ));
        }
        if !self.is_abandoned() {
            return Ok(None);
        }
        // Terminal already: report the existing state and never rewrite
        // it. A recorded request answers exactly like the call that
        // recorded it, so repeating the request is idempotent.
        let action = if recorded.is_some() {
            AbandonAction::Abandoned
        } else {
            AbandonAction::AlreadyAborted
        };
        Ok(Some(AbandonResult {
            action,
            status: self.record.status,
            reason: recorded.map(|intent| intent.reason.clone()),
            requested_at: recorded.map(|intent| intent.requested_at.clone()),
        }))
    }

    pub(crate) fn abandoning_result(&self, intent: &AbandonIntent) -> AbandonResult {
        AbandonResult {
            action: AbandonAction::Abandoning,
            status: self.record.status,
            reason: Some(intent.reason.clone()),
            requested_at: Some(intent.requested_at.clone()),
        }
    }

    pub(crate) fn abandon_wait_children(&self) -> Vec<String> {
        wait_child_ids(&self.record)
    }
}

impl RunApplication {
    /// Apply a recorded abandon request once the Run holds no execution slot.
    ///
    /// `Ok(None)` means no request was ever recorded; the caller keeps the
    /// status the Runner produced.
    pub(crate) async fn finalize_recorded_abandon(
        &self,
        run_id: &str,
    ) -> Result<Option<RunStatus>, ApplicationError> {
        let Some(intent) = self.abandon_intent(run_id)? else {
            return Ok(None);
        };
        Ok(Some(
            self.finalize_abandoned_run(run_id, &intent.reason, &intent.requested_at)
                .await?
                .record
                .status,
        ))
    }

    /// Make the Run terminal `Aborted` unless it is still executing (or already
    /// terminal). Holds the execution lock so a concurrent resume cannot slip
    /// between the liveness check and the durable write.
    async fn finalize_abandoned_run(
        &self,
        run_id: &str,
        reason: &str,
        requested_at: &str,
    ) -> Result<RunView<'static>, ApplicationError> {
        let active = self.active.lock().await;
        if active.contains_key(run_id) {
            return self.read_run(run_id)?.ok_or(ApplicationError::Missing);
        }
        let _lease = self.store().acquire_lease(run_id)?;
        let mut record = self
            .store()
            .load(run_id)?
            .ok_or(ApplicationError::Missing)?;
        if !RunView::inactive(&record).is_settled() {
            record.status = RunStatus::Aborted;
            record.error = Some(format!(
                "Run abandoned by operator (reason {reason}, requested at {requested_at})"
            ));
            self.store().save(&record)?;
        }
        Ok(RunView {
            record: Cow::Owned(record),
            active: false,
            requested: None,
            session_coordinated: false,
        })
    }

    pub(in crate::application) async fn finish_abandon(
        &self,
        run_id: &str,
        intent: AbandonIntent,
    ) -> Result<AbandonResult, ApplicationError> {
        let record = self
            .finalize_abandoned_run(run_id, &intent.reason, &intent.requested_at)
            .await?;
        if record.is_abandoned() {
            self.settle_channel_run(run_id, RunStatus::Aborted)?;
            return Ok(AbandonResult {
                action: AbandonAction::Abandoned,
                status: record.record.status,
                reason: Some(intent.reason),
                requested_at: Some(intent.requested_at),
            });
        }
        if !record.can_abandon() {
            return Err(ApplicationError::Conflict(
                "Run finished before it was abandoned".into(),
            ));
        }
        // A resume that won the race before the request was recorded keeps
        // executing; cancellation will settle it into the requested state.
        Ok(record.abandoning_result(&intent))
    }

    /// Make every recorded, still-unfinished abandon request terminal. Called at
    /// startup, before channel assistant recovery hands instances over, so an
    /// interrupted abandon cannot keep blocking Graph/Plugin replacement.
    pub(crate) async fn finalize_recorded_abandons_at_startup(
        &self,
    ) -> Result<(), ApplicationError> {
        let root = self.data_root.join(ABANDON_DIR);
        let entries = match std::fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(storage(error)),
        };
        for entry in entries {
            let entry = entry.map_err(storage)?;
            let Some(run_id) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.strip_suffix(".json"))
                .map(str::to_owned)
            else {
                continue;
            };
            let intent = match self.abandon_intent(&run_id) {
                Ok(Some(intent)) => intent,
                Ok(None) => continue,
                Err(error) => {
                    eprintln!("Run {run_id} abandon intent is unreadable: {error:?}");
                    continue;
                }
            };
            match self
                .finalize_abandoned_run(&run_id, &intent.reason, &intent.requested_at)
                .await
            {
                Ok(record) if record.is_abandoned() => {
                    if let Err(error) = self.settle_channel_run(&run_id, RunStatus::Aborted) {
                        eprintln!("Run {run_id} abandon settlement failed: {error:?}");
                    }
                }
                Ok(_) => {}
                Err(error) => eprintln!("Run {run_id} abandon finalization failed: {error:?}"),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anchor_runtime::graph::{GraphRunRecord, GraphSnapshot};
    use serde_json::json;

    fn record(status: RunStatus) -> GraphRunRecord {
        let snapshot = GraphSnapshot::admit(json!({
            "objective":"abandon fixture", "entry":"worker",
            "agents":{"role":{"model":"fixture","instructions":"work"}},
            "nodes":[{"id":"worker","agent":"role"}], "edges":[]
        }))
        .unwrap();
        let mut record =
            GraphRunRecord::create_with_id(snapshot, json!({}), "abandon-view").unwrap();
        super::super::tests::set_valid_state(&mut record, status);
        record.updated = Some("2026-10-01T00:00:00.000Z".into());
        record.error = Some("previous observation".into());
        record
    }

    fn intent() -> AbandonIntent {
        serde_json::from_value(json!({
            "format":1,
            "run_id":"abandon-view",
            "reason":"operator",
            "requested_at":"2026-10-19T00:00:00Z"
        }))
        .unwrap()
    }

    fn statuses() -> [RunStatus; 10] {
        [
            RunStatus::Ready,
            RunStatus::Running,
            RunStatus::Paused,
            RunStatus::Stopped,
            RunStatus::BudgetStopped,
            RunStatus::WaitingCall,
            RunStatus::WaitingRecovery,
            RunStatus::Completed,
            RunStatus::Failed,
            RunStatus::Aborted,
        ]
    }

    #[test]
    fn initial_abandon_outcomes_preserve_all_legacy_states_and_intent_details() {
        let intent = intent();
        for status in statuses() {
            let record = record(status);
            let view = RunView::inactive(&record);
            for recorded in [None, Some(&intent)] {
                let result = view.existing_abandon_result(recorded);
                match status {
                    RunStatus::Aborted => {
                        let result = result.unwrap().unwrap();
                        assert_eq!(result.status, RunStatus::Aborted);
                        assert_eq!(
                            result.action,
                            if recorded.is_some() {
                                AbandonAction::Abandoned
                            } else {
                                AbandonAction::AlreadyAborted
                            }
                        );
                        assert_eq!(
                            result.reason.as_deref(),
                            recorded.map(|intent| intent.reason.as_str())
                        );
                        assert_eq!(
                            result.requested_at.as_deref(),
                            recorded.map(|intent| intent.requested_at.as_str())
                        );
                    }
                    RunStatus::Completed | RunStatus::Failed => {
                        assert!(matches!(result, Err(ApplicationError::Conflict(message))
                            if message == "terminal Run cannot be abandoned"));
                    }
                    _ => assert!(result.unwrap().is_none(), "{status:?}"),
                }
                let pending = view.abandoning_result(&intent);
                assert_eq!(pending.action, AbandonAction::Abandoning);
                assert_eq!(pending.status, status);
                assert_eq!(pending.reason.as_deref(), Some("operator"));
                assert_eq!(
                    pending.requested_at.as_deref(),
                    Some(intent.requested_at.as_str())
                );
            }
        }
    }

    #[tokio::test]
    async fn finalization_preserves_timestamp_and_only_changes_the_original_fields() {
        let root = tempfile::tempdir().unwrap();
        let application = RunApplication::new(root.path().join("state"), root.path().into());
        let intent = intent();
        let path = application.data_root.join("runs/abandon-view.json");
        for status in statuses() {
            let record = record(status);
            application.store().save(&record).unwrap();
            let before = std::fs::read(&path).unwrap();
            let result = application
                .finalize_abandoned_run(&record.run_id, &intent.reason, &intent.requested_at)
                .await
                .unwrap();
            let mut expected = record.clone();
            if !matches!(
                status,
                RunStatus::Completed | RunStatus::Failed | RunStatus::Aborted
            ) {
                expected.status = RunStatus::Aborted;
                expected.error = Some(format!(
                    "Run abandoned by operator (reason operator, requested at {})",
                    intent.requested_at
                ));
            } else {
                assert_eq!(std::fs::read(&path).unwrap(), before);
            }
            assert_eq!(result.record.as_ref(), &expected);
            assert_eq!(
                application.store().load(&record.run_id).unwrap().unwrap(),
                expected
            );
            assert_eq!(result.record.updated, record.updated);
        }
    }

    #[tokio::test]
    async fn active_finalization_does_not_acquire_a_lease_or_write_the_record() {
        let root = tempfile::tempdir().unwrap();
        let application = RunApplication::new(root.path().join("state"), root.path().into());
        let record = record(RunStatus::Running);
        application.store().save(&record).unwrap();
        let before = std::fs::read(application.data_root.join("runs/abandon-view.json")).unwrap();
        let _lease = application.store().acquire_lease(&record.run_id).unwrap();
        application.active.lock().await.insert(
            record.run_id.clone(),
            crate::application::ActiveRun::new(
                root.path().into(),
                crate::application::new_control(),
            ),
        );
        let result = application
            .finalize_abandoned_run(&record.run_id, "operator", "fixture")
            .await
            .unwrap();
        assert_eq!(result.record.as_ref(), &record);
        assert!(application.active.lock().await.contains_key(&record.run_id));
        assert_eq!(
            std::fs::read(application.data_root.join("runs/abandon-view.json")).unwrap(),
            before
        );
    }

    #[tokio::test]
    async fn inactive_finalization_rejects_a_busy_lease_without_writing() {
        let root = tempfile::tempdir().unwrap();
        let application = RunApplication::new(root.path().join("state"), root.path().into());
        let record = record(RunStatus::Stopped);
        application.store().save(&record).unwrap();
        let _lease = application.store().acquire_lease(&record.run_id).unwrap();
        assert!(matches!(
            application
                .finalize_abandoned_run(&record.run_id, "operator", "fixture")
                .await,
            Err(ApplicationError::Conflict(_))
        ));
        assert_eq!(
            application.store().load(&record.run_id).unwrap().unwrap(),
            record
        );
    }

    #[tokio::test]
    async fn finalization_keeps_the_race_error_distinct_from_initial_rejection() {
        let root = tempfile::tempdir().unwrap();
        let application = RunApplication::new(root.path().join("state"), root.path().into());
        let record = record(RunStatus::Completed);
        application.store().save(&record).unwrap();
        assert!(matches!(
            application.finish_abandon(&record.run_id, intent()).await,
            Err(ApplicationError::Conflict(message)) if message == "Run finished before it was abandoned"
        ));
        assert_eq!(
            application.store().load(&record.run_id).unwrap().unwrap(),
            record
        );
    }
}
