use super::*;
use crate::run_metadata::{ConversationSource, GraphCallSource, SessionCall, SessionContext};
use anchor_runtime::graph::{GraphCallOutcome, RecoveryAttempt};
use std::time::Duration;

async fn resolve_session_call(Json(request): Json<Value>) -> Json<Value> {
    assert_eq!(request["session"], "alice");
    assert_eq!(request["graph"], "child");
    Json(json!({
        "session":"alice",
        "reply_node":"work",
        "conversation_id":"alice-conversation",
        "channel":{"source":"wecom","sender_id":"alice-user"}
    }))
}

fn seed_session_call(state: &ApiState, run: &str, mode: &str, bound: bool) {
    let bundle = FileGraphBundleLoader::new(&state.bundle_root)
        .load()
        .unwrap();
    let mut record = GraphRunRecord::create_with_id(
        bundle.snapshot,
        json!({"session":"alice", "channel":{"source":"wecom", "sender_id":"alice-user"}}),
        run,
    )
    .unwrap();
    record.plugin_bindings_initialized = true;
    let mut child = RunMetadata::graph_call_child(
        run.into(),
        "fixture".into(),
        record.graph_digest.clone(),
        &state.bundle_root,
        GraphCallSource {
            parent_run: "absent-parent".into(),
            parent_graph: "source".into(),
            parent_graph_digest: "source-digest".into(),
            node: "invoke".into(),
            invocation: 1,
            mode: mode.into(),
            root_run: "absent-parent".into(),
        },
    )
    .unwrap();
    child.session_call = Some(SessionCall {
        context: SessionContext {
            session: "alice".into(),
            reply_node: "work".into(),
            conversation_id: "alice-conversation".into(),
            channel: record.input["channel"].clone(),
        },
        status: "pending".into(),
        error: String::new(),
    });
    if bound {
        child.conversation = Some(ConversationSource {
            session: "alice".into(),
            reply_node: "work".into(),
            previous_run: None,
        });
    }
    metadata::save(&state.data_root, &child).unwrap();
    fixtures::create(&state.application, &record);
}

/// A foreground turn of `session` on the fixture Graph, parked the way a
/// stopped assistant instance is persisted: its conversation identity names
/// `previous` as the Run it was handed over from.
fn foreground(state: &ApiState, run: &str, session: &str, previous: Option<&str>) -> String {
    let bundle = FileGraphBundleLoader::new(&state.bundle_root)
        .load()
        .unwrap();
    let mut record =
        GraphRunRecord::create_with_id(bundle.snapshot, json!({"session":session}), run).unwrap();
    record.plugin_bindings_initialized = true;
    fixtures::create(&state.application, &record);
    fixtures::control(&state.application, run, "stop", "stopped");
    let mut metadata = RunMetadata::new(
        run.into(),
        "fixture".into(),
        record.graph_digest,
        &state.bundle_root,
    )
    .unwrap();
    metadata.conversation = Some(ConversationSource {
        session: session.into(),
        reply_node: "work".into(),
        previous_run: previous.map(str::to_owned),
    });
    metadata::save(&state.data_root, &metadata).unwrap();
    run.into()
}

async fn resume(app: &Router, run: &str) {
    let (status, resumed) = call(app.clone(), "POST", &format!("/runs/{run}/resume"), None).await;
    assert!(status.is_success(), "{status}: {resumed}");
}

async fn execute(
    app: &Router,
    run: &str,
    session: &str,
    previous: Option<&str>,
) -> (StatusCode, Value) {
    call(
        app.clone(),
        "POST",
        &format!("/runs/{run}/session-execution"),
        Some(&json!({"session":session, "previous_run":previous}).to_string()),
    )
    .await
}

fn record_bytes(state: &ApiState, run: &str) -> Vec<u8> {
    std::fs::read(kernel_record_path(state, run)).unwrap()
}

fn metadata_bytes(state: &ApiState, run: &str) -> Vec<u8> {
    std::fs::read(
        state
            .data_root
            .join("run-metadata")
            .join(format!("{run}.json")),
    )
    .unwrap()
}

async fn completed_session_call(state: &ApiState, app: &Router, run: &str) {
    seed_session_call(state, run, "wait", false);
    let (status, accepted) = execute(app, run, "alice", None).await;
    assert_eq!(status, StatusCode::OK, "{accepted}");
    wait_idle(state).await;
    assert!(
        fixtures::Reader::new(&state.application)
            .load(run)
            .unwrap()
            .unwrap()
            .has_succeeded()
    );
}

#[tokio::test]
async fn session_execution_rejects_other_session_and_wrong_previous_identity() {
    let (root, state) = fixture();
    let _env = PROCESS_ENV.lock().await;
    set_host_env(root.path());
    let app = router(state.clone());
    let alice = foreground(&state, "alice-foreground", "alice", None);
    let bob = foreground(&state, "bob-foreground", "bob", None);
    let run = "session-identity";
    seed_session_call(&state, run, "wait", false);
    let original_record = record_bytes(&state, run);
    let original_metadata = metadata_bytes(&state, run);
    for (session, previous) in [
        ("bob", Some(alice.as_str())),
        ("alice", None),
        ("alice", Some(bob.as_str())),
        ("alice", Some("missing-run")),
    ] {
        let (status, rejected) = execute(&app, run, session, previous).await;
        assert_eq!(status, StatusCode::CONFLICT, "{rejected}");
        assert_eq!(record_bytes(&state, run), original_record);
        assert_eq!(metadata_bytes(&state, run), original_metadata);
        assert!(state.application.active_runs(None).await.is_empty());
    }
    let (status, accepted) = execute(&app, run, "alice", Some(&alice)).await;
    assert_eq!(status, StatusCode::OK, "{accepted}");
    wait_idle(&state).await;
    let original_record = record_bytes(&state, run);
    let original_metadata = metadata_bytes(&state, run);
    let (status, rejected) = execute(&app, run, "alice", Some(&bob)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{rejected}");
    assert_eq!(record_bytes(&state, run), original_record);
    assert_eq!(metadata_bytes(&state, run), original_metadata);
}

#[tokio::test]
async fn queued_session_call_allows_foreground_and_is_not_dispatched_at_startup() {
    let (root, state) = fixture();
    let _env = PROCESS_ENV.lock().await;
    set_host_env(root.path());
    seed_session_call(&state, "queued-session-call", "detach", false);
    state
        .application
        .recover_detached_at_startup()
        .await
        .unwrap();
    assert!(state.application.active_runs(None).await.is_empty());
    let app = router(state.clone());
    let foreground = foreground(&state, "queued-foreground", "alice", None);
    resume(&app, &foreground).await;
    wait_idle(&state).await;
    let store = fixtures::Reader::new(&state.application);
    assert!(is_executing(
        &store.load("queued-session-call").unwrap().unwrap()
    ));
    assert!(store.load(&foreground).unwrap().unwrap().has_succeeded());
    let context = state
        .application
        .metadata("queued-session-call")
        .unwrap()
        .unwrap();
    assert_eq!(context.session_call.unwrap().status, "pending");
    assert!(context.conversation.is_none());
}

#[tokio::test]
async fn bound_session_call_yields_and_continues_same_run_after_foreground_and_restart() {
    let (root, mut state) = fixture();
    let _env = PROCESS_ENV.lock().await;
    set_host_env(root.path());
    let run = "bound-session-call";
    // This is the durable boundary after binding and before dispatch on a prior host.
    seed_session_call(&state, run, "wait", true);
    let frozen = fixtures::Reader::new(&state.application)
        .load(run)
        .unwrap()
        .unwrap();
    let app = router(state.clone());
    let (status, yielded) = call(
        app.clone(),
        "POST",
        &format!("/runs/{run}/session-yield"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{yielded}");
    let stopped = fixtures::Reader::new(&state.application)
        .load(run)
        .unwrap()
        .unwrap();
    assert!(stopped.waits_reason("stopped"));
    assert_eq!(stopped.input, frozen.input);
    let next = foreground(&state, "bound-foreground", "alice", Some(run));
    resume(&app, &next).await;
    wait_idle(&state).await;
    drop(app);

    state.application = RunApplication::new(state.data_root.clone(), state.catalog_root.clone())
        .with_configured_graph("fixture".into(), state.bundle_root.clone());
    let restarted = state.clone();
    let app = router(restarted.clone());
    let (status, rejected) = execute(&app, run, "alice", Some(&next)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{rejected}");
    let (status, accepted) = execute(&app, run, "alice", None).await;
    assert_eq!(status, StatusCode::OK, "{accepted}");
    wait_idle(&restarted).await;
    let resumed = fixtures::Reader::new(&state.application)
        .load(run)
        .unwrap()
        .unwrap();
    assert!(resumed.has_succeeded(), "{resumed:?}");
    assert_eq!(resumed.run_id, run);
    assert_eq!(resumed.snapshot, frozen.snapshot);
    assert_eq!(resumed.input, frozen.input);
    assert_eq!(resumed.passes["work"], 1);
    assert_eq!(state.application.records().unwrap().len(), 2);
    let binding = state.application.metadata(run).unwrap().unwrap();
    assert_eq!(binding.session_call.unwrap().status, "pending");
    assert_eq!(binding.conversation.unwrap().previous_run, None);
}

#[tokio::test]
async fn active_session_call_yield_releases_execution_without_settling_delivery() {
    let (root, state) = fixture();
    let _env = PROCESS_ENV.lock().await;
    set_host_env(root.path());
    unsafe {
        env::set_var("ANCHOR_RUNNER_ALLOWED_COMMANDS", "true,sh,cat,sleep,printf");
    }
    write_graph_bundle(&state.bundle_root, &json!({
        "objective":"session gate", "entry":"work", "agents":{},
        "ops":{"work":{"run":"printf ready > gate.ready; while [ ! -f gate.release ]; do sleep 0.02; done", "wall_time_limit_seconds":10}},
        "nodes":[{"id":"work","op":"work"}], "edges":[],
    })).unwrap();
    let run = "active-session-call";
    seed_session_call(&state, run, "wait", false);
    let app = router(state.clone());
    let (status, accepted) = execute(&app, run, "alice", None).await;
    assert_eq!(status, StatusCode::OK, "{accepted}");
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let record = fixtures::Reader::new(&state.application)
                .load(run)
                .unwrap()
                .unwrap();
            assert!(
                !(is_failed(&record) || record.has_succeeded()),
                "{record:?}"
            );
            if let Some(cursor) = record.view.dispatches().first() {
                let workspace = HostArtifacts::new(
                    state.data_root.join("artifacts"),
                    state.workspace_root.clone(),
                )
                .workspace_path(&cursor.key)
                .unwrap();
                if workspace.join("gate.ready").is_file() {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the Op did not reach its local fixture gate");
    let (status, yielded) = call(
        app.clone(),
        "POST",
        &format!("/runs/{run}/session-yield"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{yielded}");
    wait_idle(&state).await;
    let record = fixtures::Reader::new(&state.application)
        .load(run)
        .unwrap()
        .unwrap();
    assert!(record.waits_reason("stopped"));
    assert!(!record.view.dispatches().is_empty());
    assert_eq!(
        state
            .application
            .metadata(run)
            .unwrap()
            .unwrap()
            .session_call
            .unwrap()
            .status,
        "pending"
    );
    let next = foreground(&state, "active-foreground", "alice", Some(run));
    resume(&app, &next).await;
    // Release only the new foreground Op; the interrupted child remains its original Run.
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let record = fixtures::Reader::new(&state.application)
                .load(&next)
                .unwrap()
                .unwrap();
            if let Some(cursor) = record.view.dispatches().first() {
                let workspace = HostArtifacts::new(
                    state.data_root.join("artifacts"),
                    state.workspace_root.clone(),
                )
                .workspace_path(&cursor.key)
                .unwrap();
                if workspace.join("gate.ready").is_file() {
                    std::fs::write(workspace.join("gate.release"), "release").unwrap();
                    break;
                }
            }
            assert!(
                !(is_failed(&record) || record.has_succeeded()),
                "{record:?}"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the foreground Op did not reach its local fixture gate");
    wait_idle(&state).await;
    assert_eq!(
        fixtures::Reader::new(&state.application)
            .load(run)
            .unwrap()
            .unwrap(),
        record
    );
}

#[tokio::test]
async fn completed_session_execution_stays_waiting_until_delivery_settlement() {
    let (root, state) = fixture();
    let _env = PROCESS_ENV.lock().await;
    set_host_env(root.path());
    let app = router(state.clone());
    let run = "session-delivery";
    completed_session_call(&state, &app, run).await;
    let original_record = record_bytes(&state, run);
    assert_eq!(
        state.application.session_call_outcome(run, "wait").unwrap(),
        Some(GraphCallOutcome::Waiting {
            child_run_id: run.into()
        }),
    );
    let (_, detail) = call(app.clone(), "GET", &format!("/runs/{run}"), None).await;
    assert_eq!(detail["projection"]["phase"], "settled");
    assert_eq!(detail["session_call"]["status"], "pending");
    for _ in 0..2 {
        let (status, settled) = call(
            app.clone(),
            "POST",
            &format!("/runs/{run}/session-settlement"),
            Some(r#"{"status":"delivered"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{settled}");
    }
    assert_eq!(
        state.application.session_call_outcome(run, "wait").unwrap(),
        None
    );
    assert_eq!(record_bytes(&state, run), original_record);
    let (status, rejected) = call(
        app,
        "POST",
        &format!("/runs/{run}/session-settlement"),
        Some(r#"{"status":"failed","error":"late failure"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{rejected}");
    assert_eq!(
        state
            .application
            .metadata(run)
            .unwrap()
            .unwrap()
            .session_call
            .unwrap()
            .status,
        "delivered"
    );
}

#[tokio::test]
async fn completed_pending_wait_child_recovers_delivery_and_parent_after_restart() {
    let (root, mut state) = fixture();
    let _env = PROCESS_ENV.lock().await;
    set_host_env(root.path());
    let session_host = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let session_host_url = format!("http://{}", session_host.local_addr().unwrap());
    let session_host_task = tokio::spawn(async move {
        axum::serve(
            session_host,
            Router::new().route(
                "/v1/runtime/session-calls/resolve",
                axum::routing::post(resolve_session_call),
            ),
        )
        .await
        .unwrap();
    });
    unsafe {
        env::set_var("ANCHOR_SESSION_HOST_URL", session_host_url);
        env::set_var("ANCHOR_SESSION_HOST_TOKEN", "fixture-token");
    }
    write_graph_bundle(
        &state.bundle_root,
        &json!({
            "objective":"wait for delivered Session child",
            "entry":"invoke",
            "agents":{},
            "ops":{
                "invoke":{"call":{"graph":"child","mode":"wait","session":"alice"}},
                "after":{"run":"true"}
            },
            "nodes":[{"id":"invoke","op":"invoke"},{"id":"after","op":"after"}],
            "edges":[{"from":"invoke","to":"after"}]
        }),
    )
    .unwrap();
    write_graph_bundle(
        &state.catalog_root.join("child"),
        &json!({
            "objective":"deterministic Session child",
            "entry":"work",
            "agents":{},
            "ops":{"work":{"run":"true"}},
            "nodes":[{"id":"work","op":"work"}],
            "edges":[]
        }),
    )
    .unwrap();

    let app = router(state.clone());
    let (status, accepted) = call(
        app.clone(),
        "POST",
        "/trigger",
        Some(r#"{"graph":"fixture","input":{}}"#),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{accepted}");
    let parent = accepted["run"].as_str().unwrap().to_owned();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let record = fixtures::Reader::new(&state.application)
                .load(&parent)
                .unwrap()
                .unwrap();
            if record.waits_for_children() {
                break;
            }
            assert!(!is_failed(&record), "{record:?}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("parent did not wait for Session delivery");
    let child = state.application.child_metadata(&parent).unwrap()[0]
        .run_id
        .clone();

    let (status, admitted) = execute(&app, &child, "alice", None).await;
    assert_eq!(status, StatusCode::OK, "{admitted}");
    wait_idle(&state).await;
    assert!(
        fixtures::Reader::new(&state.application)
            .load(&parent)
            .unwrap()
            .unwrap()
            .waits_for_children()
    );
    let child_record = record_bytes(&state, &child);
    let child_metadata = metadata_bytes(&state, &child);
    assert_eq!(
        state
            .application
            .metadata(&child)
            .unwrap()
            .unwrap()
            .session_call
            .unwrap()
            .status,
        "pending"
    );

    drop(app);
    state.application = RunApplication::new(state.data_root.clone(), state.catalog_root.clone())
        .with_configured_graph("fixture".into(), state.bundle_root.clone());
    let restarted_app = router(state.clone());
    for _ in 0..2 {
        state
            .application
            .recover_detached_at_startup()
            .await
            .unwrap();
    }
    let (status, runs) = call(restarted_app.clone(), "GET", "/runs", None).await;
    assert_eq!(status, StatusCode::OK, "{runs}");
    let child_projection = runs["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|run| run["run"] == child)
        .unwrap();
    assert_eq!(child_projection["projection"]["outcome"], "done");
    assert_eq!(child_projection["session_call"]["status"], "pending");
    assert_eq!(record_bytes(&state, &child), child_record);
    assert_eq!(metadata_bytes(&state, &child), child_metadata);
    assert!(
        fixtures::Reader::new(&state.application)
            .load(&parent)
            .unwrap()
            .unwrap()
            .waits_for_children()
    );

    let (status, readmitted) = execute(&restarted_app, &child, "alice", None).await;
    assert_eq!(status, StatusCode::OK, "{readmitted}");
    assert!(state.application.active_runs(None).await.is_empty());
    assert_eq!(record_bytes(&state, &child), child_record);
    assert_eq!(metadata_bytes(&state, &child), child_metadata);

    for _ in 0..2 {
        let (status, settled) = call(
            restarted_app.clone(),
            "POST",
            &format!("/runs/{child}/session-settlement"),
            Some(r#"{"status":"delivered"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{settled}");
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let record = fixtures::Reader::new(&state.application)
                .load(&parent)
                .unwrap()
                .unwrap();
            if record.has_succeeded() {
                break record;
            }
            assert!(!is_failed(&record), "{record:?}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("settled Session child did not resume its waiting parent");
    wait_idle(&state).await;
    let child_record_after = fixtures::Reader::new(&state.application)
        .load(&child)
        .unwrap()
        .unwrap();
    assert!(child_record_after.has_succeeded());
    assert_eq!(child_record_after.passes["work"], 1);
    assert_eq!(child_record_after.results["work"].len(), 1);
    assert_eq!(
        state
            .application
            .metadata(&child)
            .unwrap()
            .unwrap()
            .session_call
            .unwrap()
            .status,
        "delivered"
    );
    let parent_record = fixtures::Reader::new(&state.application)
        .load(&parent)
        .unwrap()
        .unwrap();
    assert_eq!(parent_record.graph_calls.len(), 1);
    assert!(parent_record.graph_calls.values().all(|call| {
        call.status == anchor_runtime::graph::GraphCallStatus::Completed
            && call.child_run_id.as_deref() == Some(child.as_str())
    }));
    assert!(parent_record.results.contains_key("invoke"));
    assert!(parent_record.results.contains_key("after"));
    session_host_task.abort();
}

#[tokio::test]
async fn stopping_completed_pending_session_call_preserves_completed_run() {
    let (root, state) = fixture();
    let _env = PROCESS_ENV.lock().await;
    set_host_env(root.path());
    let app = router(state.clone());
    let run = "session-stop-delivery";
    completed_session_call(&state, &app, run).await;
    let original_record = record_bytes(&state, run);
    let (status, stopped) = call(app.clone(), "POST", &format!("/runs/{run}/stop"), None).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{stopped}");
    assert_eq!(record_bytes(&state, run), original_record);
    let call_state = state
        .application
        .metadata(run)
        .unwrap()
        .unwrap()
        .session_call
        .unwrap();
    assert_eq!(call_state.status, "failed");
    assert_eq!(
        state.application.session_call_outcome(run, "wait").unwrap(),
        Some(GraphCallOutcome::Failed {
            child_run_id: Some(run.into()),
            reason: call_state.error
        }),
    );
    let (status, rejected) = execute(&app, run, "alice", None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{rejected}");
}

#[tokio::test]
async fn generic_session_resume_pause_and_recovery_cannot_bypass_host_arbitration() {
    let (_root, state) = fixture();
    let run = "session-controls";
    write_graph_bundle(
        &state.bundle_root,
        &json!({
            "entry":"work","agents":{"worker":{"model":"fixture"}},
            "nodes":[{"id":"work","agent":"worker"}],"edges":[]
        }),
    )
    .unwrap();
    seed_session_call(&state, run, "wait", true);
    fixtures::recovery(
        &state.application,
        run,
        "work",
        vec![RecoveryAttempt {
            attempt_id: 17,
            step: 0,
            tool: "anchor_run".into(),
            started_at: "unknown".into(),
        }],
    );
    let original_record = record_bytes(&state, run);
    let original_metadata = metadata_bytes(&state, run);
    let app = router(state.clone());
    for (operation, body) in [
        ("resume", None),
        ("pause", None),
        (
            "recovery",
            Some(r#"{"node_id":"work","invocation":1,"attempt_id":17,"decision":"retry"}"#),
        ),
        (
            "recovery",
            Some(
                r#"{"node_id":"work","invocation":1,"attempt_id":17,"decision":"completed","observation":"Inspect the existing effect before continuing"}"#,
            ),
        ),
    ] {
        let (status, rejected) = call(
            app.clone(),
            "POST",
            &format!("/runs/{run}/{operation}"),
            body,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{rejected}");
        assert_eq!(record_bytes(&state, run), original_record);
        assert_eq!(metadata_bytes(&state, run), original_metadata);
        assert!(state.application.active_runs(None).await.is_empty());
    }
}

#[tokio::test]
async fn session_recovery_native_persistence_failure_keeps_attempt_pending_without_dispatch() {
    let (root, state) = fixture();
    let _env = PROCESS_ENV.lock().await;
    struct Restore(Vec<(&'static str, Option<std::ffi::OsString>)>);
    impl Drop for Restore {
        fn drop(&mut self) {
            for (name, value) in &self.0 {
                unsafe {
                    match value {
                        Some(value) => env::set_var(name, value),
                        None => env::remove_var(name),
                    }
                }
            }
        }
    }
    let names = [
        "ANCHOR_RUNNER_WORKSPACE_ROOT",
        "ANCHOR_RUNNER_ALLOWED_COMMANDS",
        "ANCHOR_RUNNER_AGENT_RUNTIME",
        "ANCHOR_GOOSE_BINARY",
        "ANCHOR_GOOSE_BINARY_SHA256",
        "ANCHOR_GOOSE_LOCAL_NETWORK",
        "ANCHOR_GOOSE_ALLOW_SHARED_NETWORK",
        "ANCHOR_MODEL_API_KEY",
        "ANCHOR_MODEL_URL",
        "ANCHOR_MODEL_NAME",
        "ANCHOR_MODEL_DIRECT",
    ];
    let _restore = Restore(
        names
            .iter()
            .map(|name| (*name, env::var_os(name)))
            .collect(),
    );
    set_host_env(root.path());
    let binary = root.path().join("unstarted-goose");
    let bytes = b"This fixture must stop before dispatch";
    std::fs::write(&binary, bytes).unwrap();
    let binary_sha256 = format!("{:x}", Sha256::digest(bytes));
    unsafe {
        env::set_var("ANCHOR_RUNNER_AGENT_RUNTIME", "goose");
        env::set_var("ANCHOR_GOOSE_BINARY", &binary);
        env::set_var("ANCHOR_GOOSE_BINARY_SHA256", &binary_sha256);
        env::set_var("ANCHOR_GOOSE_LOCAL_NETWORK", "0");
        env::set_var("ANCHOR_GOOSE_ALLOW_SHARED_NETWORK", "1");
        env::set_var("ANCHOR_MODEL_DIRECT", "0");
    }
    crate::model_configuration::ModelConfiguration::from_env(&state.data_root)
        .unwrap()
        .save(
            serde_json::from_value(json!({
                "endpoint":"http://127.0.0.1:1/v1","wire_api":"chat","model":"fixture",
                "api_key":{"action":"set","value":"unstarted-fixture-only"}
            }))
            .unwrap(),
        )
        .unwrap();
    write_graph_bundle(
        &state.bundle_root,
        &json!({
            "entry":"work","agents":{"worker":{"model":"fixture"}},
            "nodes":[{"id":"work","agent":"worker"}],"edges":[]
        }),
    )
    .unwrap();
    let run = "session-recovery-write-failure";
    seed_session_call(&state, run, "wait", true);
    let key = state
        .application
        .read_run(run)
        .unwrap()
        .unwrap()
        .dispatches()[0]
        .key
        .clone();
    let observation = json!({
        "tool":"anchor_run","arguments":{"command":"inspect existing effect"},
        "native_tool_call":{"session":"native-session","tool_call":"pending-tool"},"result":null
    });
    let identity = serde_json::to_vec(&(
        &key,
        "anchor_run",
        &observation["arguments"],
        &observation["native_tool_call"],
    ))
    .unwrap();
    let digest = Sha256::digest(identity);
    let attempt_id =
        (u64::from_be_bytes(digest[..8].try_into().unwrap()) & i64::MAX as u64).max(1) as i64;
    fixtures::recovery(
        &state.application,
        run,
        "work",
        vec![RecoveryAttempt {
            attempt_id,
            step: 0,
            tool: "anchor_run".into(),
            started_at: "unknown".into(),
        }],
    );
    let facts = state.data_root.join("goose-acp");
    std::fs::create_dir_all(&facts).unwrap();
    let fact_path = facts.join(format!(
        "{:x}.json",
        Sha256::digest(key.durable_key().as_bytes())
    ));
    let fact = serde_json::to_vec_pretty(&json!({
        "version":2,"key":key,"binary_sha256":binary_sha256,"session_id":"native-session",
        "completion":null,"reason":null,"model_binding":null,"tool_observation":observation
    }))
    .unwrap();
    std::fs::write(&fact_path, &fact).unwrap();
    // Reading and validating the real native fact succeeds; only its durable
    // decision write fails when store_fact opens the temporary path.
    std::fs::create_dir(fact_path.with_extension("tmp")).unwrap();
    let original_record = record_bytes(&state, run);
    let original_metadata = metadata_bytes(&state, run);
    let app = router(state.clone());
    let (status, failed) = execute(&app, run, "alice", None).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{failed}");
    assert!(
        failed["error"].as_str().unwrap().contains("Is a directory"),
        "{failed}"
    );
    assert_eq!(record_bytes(&state, run), original_record);
    assert_eq!(metadata_bytes(&state, run), original_metadata);
    assert_eq!(std::fs::read(&fact_path).unwrap(), fact);
    assert!(
        state
            .application
            .read_run(run)
            .unwrap()
            .unwrap()
            .has_pending_recovery()
    );
    assert!(state.application.active_runs(None).await.is_empty());

    // The ordinary human API shares the same persistence ordering. Preparing
    // its native port must query Run facts before recover_attempt takes the
    // worker lock, so this failure returns rather than blocking on that lock.
    let mut ordinary = state.application.metadata(run).unwrap().unwrap();
    ordinary.session_call = None;
    ordinary.conversation = None;
    metadata::save(&state.data_root, &ordinary).unwrap();
    let ordinary_metadata = metadata_bytes(&state, run);
    let (status, failed) = call(
        app.clone(),
        "POST",
        &format!("/runs/{run}/recovery"),
        Some(
            &json!({"node_id":"work","invocation":1,"attempt_id":attempt_id,
            "decision":"completed","observation":"Inspect the existing effect"})
            .to_string(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{failed}");
    assert!(
        failed["error"].as_str().unwrap().contains("Is a directory"),
        "{failed}"
    );
    assert_eq!(record_bytes(&state, run), original_record);
    assert_eq!(metadata_bytes(&state, run), ordinary_metadata);
    assert_eq!(std::fs::read(&fact_path).unwrap(), fact);
    assert!(
        state
            .application
            .read_run(run)
            .unwrap()
            .unwrap()
            .has_pending_recovery()
    );
    assert!(state.application.active_runs(None).await.is_empty());
}

#[tokio::test]
async fn pending_session_delivery_blocks_graph_mutation_and_deletion() {
    let (root, state) = fixture();
    let _env = PROCESS_ENV.lock().await;
    set_host_env(root.path());
    let app = router(state.clone());
    let run = "session-retained-delivery";
    completed_session_call(&state, &app, run).await;
    let graph = std::fs::read(state.bundle_root.join("graph.json")).unwrap();
    let original_record = record_bytes(&state, run);
    let original_metadata = metadata_bytes(&state, run);
    let update = json!({"definition":json!({"objective":"updated", "entry":"work", "agents":{}, "ops":{"work":{"run":"true"}}, "nodes":[{"id":"work","op":"work"}], "edges":[]})}).to_string();
    for (method, uri, body) in [
        ("PUT", "/graphs/fixture".to_owned(), Some(update.as_str())),
        ("DELETE", "/graphs/fixture".to_owned(), None),
        ("DELETE", format!("/runs/{run}"), None),
    ] {
        let (status, rejected) = call(app.clone(), method, &uri, body).await;
        assert_eq!(status, StatusCode::CONFLICT, "{rejected}");
        assert_eq!(
            std::fs::read(state.bundle_root.join("graph.json")).unwrap(),
            graph
        );
        assert_eq!(record_bytes(&state, run), original_record);
        assert_eq!(metadata_bytes(&state, run), original_metadata);
    }
    assert_eq!(
        call(
            app.clone(),
            "POST",
            &format!("/runs/{run}/session-settlement"),
            Some(r#"{"status":"delivered"}"#)
        )
        .await
        .0,
        StatusCode::OK
    );
    let (status, updated) = call(app.clone(), "PUT", "/graphs/fixture", Some(&update)).await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    let (status, deleted) = call(app, "DELETE", "/graphs/fixture", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{deleted}");
    assert!(state.application.records().unwrap().is_empty());
}
