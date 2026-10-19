use super::*;
use anchor_runtime::graph::{
    GraphRunRecord, InvocationKey, PendingRecovery, RecoveryAttempt, RunCursor, RunStatus, RunStore,
};
use std::time::{Duration, UNIX_EPOCH};

#[tokio::test]
async fn run_status_http_json_bytes_match_the_master_contract() {
    for (status, wire_status, phase, resume, stop, terminal, quiescent, attention) in [
        (
            RunStatus::Ready,
            "ready",
            "ready",
            true,
            true,
            false,
            false,
            false,
        ),
        (
            RunStatus::Running,
            "running",
            "waiting_resume",
            true,
            true,
            false,
            false,
            true,
        ),
        (
            RunStatus::Paused,
            "paused",
            "paused",
            true,
            true,
            false,
            false,
            false,
        ),
        (
            RunStatus::BudgetStopped,
            "budget_stopped",
            "budget_stopped",
            false,
            true,
            false,
            false,
            true,
        ),
        (
            RunStatus::WaitingCall,
            "waiting_call",
            "waiting_call",
            true,
            true,
            false,
            false,
            false,
        ),
        (
            RunStatus::WaitingRecovery,
            "waiting_recovery",
            "waiting_recovery",
            true,
            true,
            false,
            false,
            true,
        ),
        (
            RunStatus::Completed,
            "completed",
            "completed",
            false,
            false,
            true,
            true,
            false,
        ),
        (
            RunStatus::Failed,
            "failed",
            "failed",
            false,
            false,
            true,
            true,
            true,
        ),
        (
            RunStatus::Aborted,
            "aborted",
            "aborted",
            false,
            false,
            true,
            true,
            true,
        ),
        (
            RunStatus::Stopped,
            "stopped",
            "stopped",
            true,
            true,
            false,
            true,
            false,
        ),
    ] {
        let (_root, state) = fixture();
        let bundle = FileGraphBundleLoader::new(&state.bundle_root)
            .load()
            .unwrap();
        let mut record =
            GraphRunRecord::create_with_id(bundle.snapshot, json!({}), "wire-run").unwrap();
        record.status = status;
        if matches!(
            status,
            RunStatus::BudgetStopped | RunStatus::WaitingRecovery
        ) {
            let key = InvocationKey {
                run_id: record.run_id.clone(),
                graph_digest: record.graph_digest.clone(),
                node_id: "work".into(),
                invocation: 1,
            };
            record.invocations.insert("work".into(), 1);
            record.passes.insert("work".into(), 1);
            record.cursor = Some(RunCursor {
                node_id: "work".into(),
                key: key.clone(),
                input_commits: Vec::new(),
                prepared_input: Value::Null,
            });
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
        FileRunStore::new(state.data_root.join("runs"))
            .save(&record)
            .unwrap();
        std::fs::File::open(state.data_root.join("runs/wire-run.json"))
            .unwrap()
            .set_modified(UNIX_EPOCH + Duration::from_secs(1))
            .unwrap();
        let mut metadata = RunMetadata::new(
            record.run_id.clone(),
            state.graph_name.clone(),
            record.graph_digest.clone(),
            &state.bundle_root,
        )
        .unwrap();
        metadata.created = "2026-10-19T00:00:00+00:00".into();
        metadata::save(&state.data_root, &metadata).unwrap();
        let request = Request::builder().uri("/runs").body(Body::empty()).unwrap();
        let response = router(state).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        let expected = format!(
            r#"{{"runs":[{{"session_call":null,"run":"wire-run","graph":"fixture","status":"{wire_status}","running":false,"projection":{{"phase":"{phase}","active":false,"control_requested":null,"terminal":{terminal},"quiescent":{quiescent},"needs_attention":{attention},"actions":{{"pause":false,"stop":{stop},"resume":{resume}}}}},"started":"2026-10-19T00:00:00+00:00","updated":"1970-01-01T00:00:01+00:00","executed":[],"objective":"fixture","trigger":{{"source":"manual","schedule":null,"scheduled_at":null}}}}]}}"#,
            resume = resume,
            stop = stop,
            attention = attention,
            phase = phase,
            quiescent = quiescent,
            terminal = terminal,
            wire_status = wire_status,
        );
        assert_eq!(bytes.as_ref(), expected.as_bytes(), "{status:?}");
    }
}
