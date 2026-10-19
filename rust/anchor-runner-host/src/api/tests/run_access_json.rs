use super::*;
use anchor_runtime::graph::{
    GraphRunRecord, InvocationKey, PendingRecovery, RecoveryAttempt, RunCursor, RunStatus, RunStore,
};
use std::time::{Duration, UNIX_EPOCH};

#[tokio::test]
async fn run_status_http_json_bytes_match_the_phase_contract() {
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
            "null",
            false,
            false,
            true,
        ),
        (
            RunStatus::WaitingRecovery,
            r#""phase":"waiting","waiting_for":"effect""#,
            "null",
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
            RunStatus::Failed,
            r#""phase":"settled","outcome":"failed""#,
            r#""""#,
            false,
            false,
            false,
        ),
        (
            RunStatus::Aborted,
            r#""phase":"settled","outcome":"aborted""#,
            r#""""#,
            false,
            false,
            false,
        ),
        (
            RunStatus::Stopped,
            r#""phase":"waiting","waiting_for":"human""#,
            r#""stopped""#,
            true,
            false,
            true,
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
            r#"{{"runs":[{{"session_call":null,"run":"wire-run","graph":"fixture","running":false,"projection":{{{phase_fields},"reason":{reason},"active":false,"actions":{{"pause":false,"stop":{stop},"resume":{resume},"abandon":{abandon}}},"updated":"1970-01-01T00:00:01+00:00"}},"started":"2026-10-19T00:00:00+00:00","updated":"1970-01-01T00:00:01+00:00","executed":[],"objective":"fixture","trigger":{{"source":"manual","schedule":null,"scheduled_at":null}}}}]}}"#,
        );
        assert_eq!(bytes.as_ref(), expected.as_bytes(), "{status:?}");
    }
}
