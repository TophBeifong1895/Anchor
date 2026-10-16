use super::*;
use crate::application::{RunMetadata, metadata};

fn frozen_record(state: &ApiState, status: RunStatus) -> GraphRunRecord {
    let snapshot = GraphSnapshot::from_authoring(json!({
        "objective":"frozen graph", "entry":"module",
        "agents":{"worker":{"model":"fixture","instructions":"private instructions"}},
        "graphs":{"review":{"entry":"first","exit":"last","nodes":[
            {"id":"first","agent":"worker"},{"id":"last","agent":"worker"}
        ],"edges":[{"from":"first","to":"last"}]}},
        "nodes":[{"id":"module","graph":"review"}],"edges":[]
    }))
    .unwrap();
    let mut record = GraphRunRecord::create_with_id(snapshot, json!({}), "frozen-run").unwrap();
    record.status = status;
    FileRunStore::new(state.data_root.join("runs"))
        .save(&record)
        .unwrap();
    metadata::save(
        &state.data_root,
        &RunMetadata::new(
            record.run_id.clone(),
            "fixture".into(),
            record.graph_digest.clone(),
            &state.bundle_root,
        )
        .unwrap(),
    )
    .unwrap();
    record
}

#[tokio::test]
async fn frozen_topology_survives_live_graph_changes_and_missing_definition() {
    let (_root, state) = super::super::tests::fixture();
    let record = frozen_record(&state, RunStatus::Completed);
    let initial = get_run(State(state.clone()), AxumPath(record.run_id.clone()))
        .await
        .unwrap()
        .0;
    assert_eq!(initial["definition"]["entry"], "module/first");
    assert_eq!(initial["definition"]["nodes"].as_array().unwrap().len(), 2);
    assert_eq!(
        initial["definition"]["edges"],
        json!([{"from":"module/first","to":"module/last"}])
    );
    std::fs::write(
        state.bundle_root.join("graph.json"),
        "changed and invalid graph",
    )
    .unwrap();
    let changed = get_run(State(state.clone()), AxumPath(record.run_id.clone()))
        .await
        .unwrap()
        .0;
    assert_eq!(changed["definition"], initial["definition"]);
    std::fs::remove_file(state.bundle_root.join("graph.json")).unwrap();
    let removed = get_run(State(state), AxumPath(record.run_id.clone()))
        .await
        .unwrap()
        .0;
    assert_eq!(removed["definition"], initial["definition"]);
    assert_eq!(removed["nodes"], json!(["module/first", "module/last"]));
    assert!(!removed["definition"].to_string().contains("private"));
}

#[tokio::test]
async fn list_and_detail_share_status_control_and_attention_projection() {
    let (_root, state) = super::super::tests::fixture();
    let mut record = frozen_record(&state, RunStatus::Ready);
    for (status, phase, resume, stop, attention) in [
        (RunStatus::Ready, "ready", true, true, false),
        (RunStatus::WaitingCall, "waiting_call", true, true, false),
        (RunStatus::Running, "waiting_resume", true, true, true),
        (
            RunStatus::WaitingRecovery,
            "waiting_recovery",
            true,
            true,
            true,
        ),
        (
            RunStatus::BudgetStopped,
            "budget_stopped",
            false,
            true,
            true,
        ),
        (RunStatus::Completed, "completed", false, false, false),
    ] {
        record.status = status;
        record.cursor = None;
        record.recovery.clear();
        if matches!(
            status,
            RunStatus::WaitingRecovery | RunStatus::BudgetStopped
        ) {
            let key = json!({"run_id":record.run_id,"graph_digest":record.graph_digest,"node_id":"module/first","invocation":1});
            record.cursor = Some(serde_json::from_value(json!({"node_id":"module/first","key":key,"input_commits":[],"prepared_input":{}})).unwrap());
            record.invocations.insert("module/first".into(), 1);
            record.passes.insert("module/first".into(), 1);
            if status == RunStatus::WaitingRecovery {
                record.recovery.push(serde_json::from_value(json!({"key":key,"attempt":{"attempt_id":1,"step":1,"tool":"anchor_run","started_at":"2026-10-16T00:00:00Z"}})).unwrap());
            }
        }
        FileRunStore::new(state.data_root.join("runs"))
            .save(&record)
            .unwrap();
        let detail = get_run(State(state.clone()), AxumPath(record.run_id.clone()))
            .await
            .unwrap()
            .0;
        let runs = projected_runs(&state).await.unwrap();
        assert_eq!(detail["projection"], runs[0]["projection"]);
        assert_eq!(detail["projection"]["phase"], phase);
        assert_eq!(detail["projection"]["actions"]["resume"], resume);
        assert_eq!(detail["projection"]["actions"]["stop"], stop);
        assert_eq!(detail["projection"]["needs_attention"], attention);
    }
}

#[tokio::test]
async fn incomplete_run_identity_does_not_offer_inactive_controls() {
    let (_root, state) = super::super::tests::fixture();
    let record = frozen_record(&state, RunStatus::Ready);
    let mut metadata = state.application.metadata(&record.run_id).unwrap().unwrap();
    metadata.graph_digest = "different-frozen-graph".into();
    metadata::save(&state.data_root, &metadata).unwrap();
    let detail = get_run(State(state.clone()), AxumPath(record.run_id.clone()))
        .await
        .unwrap()
        .0;
    assert_eq!(
        detail["projection"]["actions"],
        json!({"pause":false,"stop":false,"resume":false})
    );
    std::fs::remove_file(
        state
            .data_root
            .join("run-metadata")
            .join(format!("{}.json", record.run_id)),
    )
    .unwrap();
    let runs = projected_runs(&state).await.unwrap();
    assert_eq!(
        runs[0]["projection"]["actions"],
        json!({"pause":false,"stop":false,"resume":false})
    );
}
