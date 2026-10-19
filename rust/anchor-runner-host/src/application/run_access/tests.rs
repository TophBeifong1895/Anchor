use super::*;
use serde_json::json;

fn record() -> GraphRunRecord {
    GraphRunRecord::create_with_id(
        serde_json::from_value(json!({
            "objective":"test", "entry":"worker",
            "agents":{"role":{"model":"fixture","instructions":"work"}},
            "nodes":[{"id":"worker","agent":"role"}], "edges":[]
        }))
        .unwrap(),
        json!({}),
        "facade-test",
    )
    .unwrap()
}

#[test]
fn phase_maps_each_legacy_state_without_exposing_its_vocabulary() {
    let mut record = record();
    record.error = Some("recorded reason".into());
    for (status, expected) in [
        (RunStatus::Ready, RunPhase::Executing),
        (RunStatus::Running, RunPhase::Executing),
        (
            RunStatus::Paused,
            RunPhase::Waiting {
                for_what: RunWait::Human,
                reason: Some("recorded reason"),
            },
        ),
        (
            RunStatus::Stopped,
            RunPhase::Waiting {
                for_what: RunWait::Human,
                reason: Some("recorded reason"),
            },
        ),
        (
            RunStatus::BudgetStopped,
            RunPhase::Waiting {
                for_what: RunWait::Human,
                reason: Some("recorded reason"),
            },
        ),
        (
            RunStatus::WaitingCall,
            RunPhase::Waiting {
                for_what: RunWait::Child,
                reason: Some("recorded reason"),
            },
        ),
        (
            RunStatus::WaitingRecovery,
            RunPhase::Waiting {
                for_what: RunWait::Effect,
                reason: Some("recorded reason"),
            },
        ),
        (
            RunStatus::Completed,
            RunPhase::Settled {
                outcome: RunOutcome::Done,
                reason: Some("recorded reason"),
            },
        ),
        (
            RunStatus::Failed,
            RunPhase::Settled {
                outcome: RunOutcome::Failed,
                reason: Some("recorded reason"),
            },
        ),
        (
            RunStatus::Aborted,
            RunPhase::Settled {
                outcome: RunOutcome::Aborted,
                reason: Some("recorded reason"),
            },
        ),
    ] {
        record.status = status;
        let view = RunView::inactive(&record);
        assert_eq!(view.phase(), expected, "{status:?}");
        assert_eq!(view.progress().snapshot.objective, "test");
        assert!(view.progress().executed_nodes().is_empty());
    }
}

#[test]
fn predicates_preserve_inactive_state_distinctions() {
    let mut record = record();
    for (status, settled, quiescent, attention, resume, stop, abandon) in [
        (RunStatus::Ready, false, false, false, true, true, true),
        (RunStatus::Running, false, false, true, true, true, true),
        (RunStatus::Paused, false, false, false, true, true, true),
        (RunStatus::Stopped, false, true, false, true, true, true),
        (
            RunStatus::BudgetStopped,
            false,
            false,
            true,
            false,
            true,
            true,
        ),
        (
            RunStatus::WaitingCall,
            false,
            false,
            false,
            true,
            true,
            true,
        ),
        (
            RunStatus::WaitingRecovery,
            false,
            false,
            true,
            false,
            true,
            true,
        ),
        (RunStatus::Completed, true, true, false, false, false, false),
        (RunStatus::Failed, true, true, true, false, false, false),
        (RunStatus::Aborted, true, true, true, false, false, true),
    ] {
        record.status = status;
        let view = RunView::inactive(&record);
        assert_eq!(view.is_settled(), settled, "{status:?}");
        assert_eq!(view.is_quiescent(), quiescent, "{status:?}");
        assert_eq!(view.needs_attention(), attention, "{status:?}");
        assert_eq!(view.can_resume(), resume, "{status:?}");
        assert_eq!(view.can_stop(), stop, "{status:?}");
        assert_eq!(view.can_abandon(), abandon, "{status:?}");
        assert!(!view.can_pause(), "{status:?}");
    }
}

#[test]
fn active_and_session_observations_do_not_reinterpret_durable_state() {
    let mut record = record();
    for status in [
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
    ] {
        record.status = status;
        for requested in [None, Some("pause"), Some("stop")] {
            for session_coordinated in [false, true] {
                let view = RunView::observed(&record, true, requested, session_coordinated);
                assert!(view.active());
                assert!(!view.is_quiescent(), "{status:?}");
                assert!(!view.can_resume(), "{status:?}");
                assert_eq!(view.can_stop(), requested != Some("stop"), "{status:?}");
                assert_eq!(
                    view.can_pause(),
                    !matches!(
                        status,
                        RunStatus::Completed | RunStatus::Failed | RunStatus::Aborted
                    ) && !session_coordinated
                        && requested.is_none(),
                    "{status:?}"
                );
                if requested.is_some() {
                    assert!(!view.needs_attention(), "{status:?}");
                }
            }
        }
        let session = RunView::observed(&record, false, None, true);
        assert!(!session.can_resume(), "{status:?}");
        assert!(!session.can_pause(), "{status:?}");
    }
}

#[test]
fn inactive_control_keeps_the_original_validation_order_and_messages() {
    let mut record = record();
    for (status, operation, message) in [
        (
            RunStatus::WaitingRecovery,
            "pause",
            "Run recovery context is not ready",
        ),
        (
            RunStatus::WaitingRecovery,
            "resume",
            "Run recovery context is not ready",
        ),
        (
            RunStatus::Completed,
            "pause",
            "terminal Run cannot be controlled",
        ),
        (
            RunStatus::Failed,
            "stop",
            "terminal Run cannot be controlled",
        ),
        (
            RunStatus::Aborted,
            "resume",
            "terminal Run cannot be controlled",
        ),
        (RunStatus::Paused, "pause", "that Run is not active"),
    ] {
        record.status = status;
        assert!(matches!(
            validate_inactive_control(&record, operation),
            Err(ApplicationError::Conflict(actual)) if actual == message
        ));
    }
    record.status = RunStatus::BudgetStopped;
    assert!(validate_inactive_control(&record, "resume").is_ok());
    assert!(matches!(
        validate_resume_budget(&record),
        Err(ApplicationError::Conflict(message))
            if message == "budget-stopped Run requires an explicit budget policy"
    ));
}

#[tokio::test]
async fn pilot_read_tools_keep_legacy_json_bytes_and_record_field_order() {
    use crate::application::RunApplication;
    use anchor_runtime::graph::RunStore;

    let root = tempfile::TempDir::new().unwrap();
    let application = RunApplication::new(root.path().into(), root.path().join("catalog"));
    let mut record = record();
    record.started = Some("2026-10-19T00:00:00Z".into());
    record.updated = record.started.clone();
    application.store().save(&record).unwrap();
    let projection = r#"{"phase":"ready","active":false,"control_requested":null,"terminal":false,"quiescent":false,"needs_attention":false,"actions":{"pause":false,"stop":false,"resume":false}}"#;
    let expected_list = format!(
        r#"{{"runs":[{{"run":"facade-test","status":"ready","projection":{projection}}}]}}"#
    );
    assert_eq!(
        serde_json::to_vec(&application.pilot_run_list().await.unwrap()).unwrap(),
        expected_list.as_bytes()
    );
    let legacy_record = serde_json::to_string(&record).unwrap();
    let expected_status =
        format!(r#"{{"run":"facade-test","record":{legacy_record},"projection":{projection}}}"#);
    assert_eq!(
        serde_json::to_vec(&application.pilot_run_status("facade-test").await.unwrap()).unwrap(),
        expected_status.as_bytes()
    );
}
