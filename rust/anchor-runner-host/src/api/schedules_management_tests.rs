use super::*;
use crate::{application::scheduling::ScheduleManagementPort, pilot_tools::PilotTools};
use anchor_platform_session::{CreateSession, SessionStore};
use anchor_runtime::ToolPort;
use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use tower::ServiceExt;

fn fixture() -> (tempfile::TempDir, ApiState, PilotTools) {
    let root = tempfile::tempdir().unwrap();
    let graph = root.path().join("catalog/task");
    std::fs::create_dir_all(&graph).unwrap();
    std::fs::write(graph.join("graph.json"), r#"{"entry":"work","ops":{"work":{"run":"true"}},"nodes":[{"id":"work","op":"work"}],"edges":[]}"#).unwrap();
    std::fs::write(
        graph.join("manifest.json"),
        r#"{"format":1,"graph":"graph.json","plugins":[]}"#,
    )
    .unwrap();
    let schedules = ScheduleStore::open(root.path().join("schedules.json")).unwrap();
    let application = RunApplication::new(root.path().join("state"), root.path().join("catalog"))
        .with_schedule_management(Arc::new(ScheduleManagementAdapter::new(schedules.clone())));
    let sessions = SessionStore::open(root.path().join("sessions.sqlite")).unwrap();
    sessions
        .create(
            "local",
            CreateSession {
                id: Some("pilot".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let (turn, _) = sessions
        .create_turn("local", "pilot", "create-plan", None)
        .unwrap();
    let tools = PilotTools {
        application: application.clone(),
        graph_name: String::new(),
        library_root: root.path().join("library"),
        data_root: root.path().join("state"),
        workspace_root: root.path().join("workspaces"),
        sessions,
        owner: "local".into(),
        session: "pilot".into(),
        turn: turn.id,
        context: None,
    };
    let (model_configuration, runtime_management) =
        crate::api::tests::management_fixture(root.path());
    let state = ApiState {
        model_configuration,
        runtime_management,
        bundle_root: graph,
        catalog_root: root.path().join("catalog"),
        data_root: root.path().join("state"),
        workspace_root: root.path().join("workspaces"),
        graph_name: String::new(),
        application,
        loopback: true,
        api_keys: Vec::new(),
        schedules,
        pilots: Default::default(),
        wecom: Default::default(),
        channel_descriptors: Default::default(),
        channel_event_locks: Default::default(),
        response_fixture: None,
    };
    (root, state, tools)
}

async fn pilot(tools: &PilotTools, name: &str, arguments: Value) -> Value {
    let content = tools.call(name, arguments).await.unwrap();
    serde_json::from_str(content[0].as_text().unwrap()).unwrap()
}

async fn http(
    state: &ApiState,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(
            body.map(|value| Body::from(value.to_string()))
                .unwrap_or_else(Body::empty),
        )
        .unwrap();
    let response = super::super::router(state.clone())
        .oneshot(request)
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn pilot_future_plan_is_the_same_http_fact_and_survives_store_restart() {
    let (root, state, tools) = fixture();
    let at = (Local::now().naive_local() + Duration::days(1))
        .with_nanosecond(0)
        .unwrap();
    let result=pilot(&tools,"schedule_create",json!({"graph":"task","rule":{"type":"once","at":format_local_datetime(at)},"input":{"task":"later"}})).await;
    let schedule = result["schedule"].clone();
    assert_eq!(schedule["next_at"], format_local_datetime(at));
    assert_eq!(schedule["enabled"], true);
    assert!(
        parse_local_datetime(schedule["next_at"].as_str().unwrap()).unwrap()
            > Local::now().naive_local()
    );
    assert!(tools.application.records().unwrap().is_empty());
    let (status, listed) = http(&state, "GET", "/schedules", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["schedules"][0], schedule);
    let (status, created) = http(
        &state,
        "POST",
        "/schedules",
        Some(json!({"graph":"task","rule":{"type":"weekly","time":"09:00","weekdays":[2,0,2]}})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(created["schedule"]["rule"]["weekdays"], json!([0, 2]));
    assert_eq!(
        pilot(&tools, "schedule_list", json!({})).await["schedules"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    drop(tools);
    drop(state);
    let reopened = ScheduleStore::open(root.path().join("schedules.json")).unwrap();
    assert_eq!(
        reopened.lock().unwrap().items[0].id,
        schedule["id"].as_str().unwrap()
    );
    assert_eq!(
        reopened.lock().unwrap().items[0].input,
        json!({"task":"later"})
    );
}

#[tokio::test]
async fn http_and_pilot_delete_one_shared_plan_and_unknown_or_invalid_requests_do_not_change_it() {
    let (_root, state, tools) = fixture();
    let created = pilot(
        &tools,
        "schedule_create",
        json!({"graph":"task","rule":{"type":"daily","time":"09:00"}}),
    )
    .await;
    let id = created["schedule"]["id"].as_str().unwrap();
    let before = state.application.list_schedules().unwrap();
    for body in [
        json!({"graph":"task","rule":{"type":"weekly","time":"09:00","weekdays":[]}}),
        json!({"graph":"missing","rule":{"type":"daily","time":"09:00"}}),
        json!({"graph":"task","rule":{"type":"daily","time":"09:00"},"network":true}),
    ] {
        let (status, _) = http(&state, "POST", "/schedules", Some(body.clone())).await;
        assert!(!status.is_success());
        assert!(tools.call("schedule_create", body).await.is_err());
        assert_eq!(state.application.list_schedules().unwrap(), before);
    }
    let deleted = pilot(&tools, "schedule_delete", json!({"schedule":id})).await;
    assert_eq!(deleted, json!({"schedule":id,"deleted":true}));
    let (status, _) = http(&state, "DELETE", &format!("/schedules/{id}"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        state.application.list_schedules().unwrap(),
        json!({"schedules":[]})
    );
}

#[test]
fn schedule_store_persistence_failure_rolls_back_create_and_delete() {
    let root = tempfile::tempdir().unwrap();
    let store = ScheduleStore::open(root.path().join("schedules.json")).unwrap();
    let adapter = ScheduleManagementAdapter::new(store.clone());
    let created = adapter
        .create("task", &json!({"type":"daily","time":"09:00"}), &json!({}))
        .unwrap();
    let before = adapter.list().unwrap();
    let path = root.path().join("not-a-directory");
    std::fs::write(&path, "ordinary-file").unwrap();
    store.lock().unwrap().path = path.join("schedules.json");
    assert!(matches!(
        adapter.create("task", &json!({"type":"daily","time":"12:00"}), &json!({})),
        Err(ApplicationError::Storage(_))
    ));
    assert_eq!(adapter.list().unwrap(), before);
    assert!(matches!(
        adapter.delete(created["schedule"]["id"].as_str().unwrap()),
        Err(ApplicationError::Storage(_))
    ));
    assert_eq!(adapter.list().unwrap(), before);
    assert!(matches!(
        adapter.update(
            created["schedule"]["id"].as_str().unwrap(),
            "task",
            &json!({"type":"daily","time":"12:00"}),
            &json!({"edited":true}),
            false
        ),
        Err(ApplicationError::Storage(_))
    ));
    assert_eq!(adapter.list().unwrap(), before);
}

#[tokio::test]
async fn editing_a_plan_preserves_identity_and_only_new_occurrences_use_edited_inputs() {
    let (root, state, tools) = fixture();
    let created = pilot(
        &tools,
        "schedule_create",
        json!({"graph":"task","rule":{"type":"interval","seconds":3600},"input":{"version":1}}),
    )
    .await;
    let original = &created["schedule"];
    let id = original["id"].as_str().unwrap();
    let (status, edited) = http(
        &state,
        "PUT",
        &format!("/schedules/{id}"),
        Some(json!({"graph":"task","rule":original["rule"],"input":{"version":2},"enabled":true})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    assert_eq!(edited["schedule"]["id"], original["id"]);
    assert_eq!(edited["schedule"]["created_at"], original["created_at"]);
    assert_eq!(edited["schedule"]["next_at"], original["next_at"]);
    assert_eq!(edited["schedule"]["input"], json!({"version":2}));
    assert!(state.application.records().unwrap().is_empty());
    let paused = pilot(&tools, "schedule_update", json!({"schedule":id,"graph":"task","rule":original["rule"],"input":{"version":3},"enabled":false})).await;
    assert_eq!(paused["schedule"]["enabled"], false);
    assert_eq!(paused["schedule"]["next_at"], original["next_at"]);
    let (_, listed) = http(&state, "GET", "/schedules", None).await;
    assert_eq!(listed["schedules"][0], paused["schedule"]);
    drop(tools);
    drop(state);
    let reopened = ScheduleStore::open(root.path().join("schedules.json")).unwrap();
    assert_eq!(
        reopened.lock().unwrap().items[0].input,
        json!({"version":3})
    );
    assert!(!reopened.lock().unwrap().items[0].enabled);
}

#[tokio::test]
async fn reenabling_skips_missed_intervals_and_rejects_a_past_once_without_changing_the_plan() {
    let (_root, state, _tools) = fixture();
    let now = Local::now().naive_local();
    let adapter = ScheduleManagementAdapter::new(state.schedules.clone());
    let created = adapter
        .create(
            "task",
            &json!({"type":"interval","seconds":3600}),
            &json!({}),
        )
        .unwrap();
    let id = created["schedule"]["id"].as_str().unwrap();
    {
        let mut store = state.schedules.lock().unwrap();
        store.items[0].enabled = false;
        store.items[0].next_at = format_local_datetime(now - Duration::days(3));
    }
    let enabled = adapter
        .update(
            id,
            "task",
            &json!({"type":"interval","seconds":3600}),
            &json!({}),
            true,
        )
        .unwrap();
    assert!(parse_local_datetime(enabled["schedule"]["next_at"].as_str().unwrap()).unwrap() > now);
    let before = adapter.list().unwrap();
    let (status, _) = http(&state, "PUT", &format!("/schedules/{id}"), Some(json!({"graph":"task","rule":{"type":"once","at":format_local_datetime(now - Duration::days(1))},"enabled":true}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(adapter.list().unwrap(), before);
    assert!(state.application.records().unwrap().is_empty());
    let (status, _) = http(
        &state,
        "PUT",
        &format!("/schedules/{id}"),
        Some(json!({"graph":"missing","rule":{"type":"daily","time":"10:00"},"enabled":true})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(adapter.list().unwrap(), before);
}
