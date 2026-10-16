use super::*;

#[tokio::test]
async fn operator_plugin_save_preserves_run_and_delivery_and_reports_pending() {
    let _environment = PROCESS_ENV.lock().await;
    let (root, state) = fixture();
    set_host_env(root.path(), &state.data_root);
    let (sessions, session, turn) = saved_assistant(&state, "local");
    let plugin = crate::library_root(&state.catalog_root).join("plugins/docmost");
    std::fs::create_dir_all(&plugin).unwrap();
    std::fs::write(plugin.join("plugin.json"), r#"{"name":"Docmost"}"#).unwrap();
    sessions
        .admit_channel_delivery(
            "local",
            &session,
            &turn,
            ChannelDeliveryRequest {
                key: "pending-reply".into(),
                kind: "text".into(),
                content_sha256: "a".repeat(64),
            },
        )
        .unwrap();
    let before = std::fs::read(state.data_root.join("runs/persistent-fixture-run.json")).unwrap();
    let mut next = definition();
    next["nodes"][1]["plugins"] = json!(["docmost"]);
    let app = router_with_web_root(state.clone(), root.path().join("web"));
    let (status, saved) = call(
        app.clone(),
        "PUT",
        "/graphs/fixture",
        Some(&json!({"definition":next}).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["plugin_activation"]["mode"], "next_turn");
    assert_eq!(
        std::fs::read(state.data_root.join("runs/persistent-fixture-run.json")).unwrap(),
        before
    );
    assert_eq!(
        sessions
            .list_unfinished_channel_deliveries("local", &session)
            .unwrap()
            .len(),
        1
    );
    let (status, current) = call(
        app.clone(),
        "GET",
        &format!("/channel-sessions/{session}/assistant"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{current}");
    assert_eq!(current["plugin_update"]["state"], "pending");
    let (status, saved_again) = call(
        app,
        "PUT",
        "/graphs/fixture",
        Some(&json!({"definition":next}).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved_again}");
}

#[tokio::test]
async fn agent_grant_and_structural_save_require_explicit_version_boundary() {
    let _environment = PROCESS_ENV.lock().await;
    let (root, state) = fixture();
    set_host_env(root.path(), &state.data_root);
    let (sessions, session, _) = saved_assistant(&state, "local");
    let plugin = crate::library_root(&state.catalog_root).join("plugins/docmost");
    std::fs::create_dir_all(&plugin).unwrap();
    std::fs::write(plugin.join("plugin.json"), r#"{"name":"Docmost"}"#).unwrap();
    let original = std::fs::read(state.bundle_root.join("graph.json")).unwrap();
    let mut next = definition();
    next["nodes"][1]["plugins"] = json!(["docmost"]);
    let denied = state
        .application
        .update_graph_from_agent("fixture", next.clone())
        .await
        .unwrap_err();
    assert!(matches!(
        denied,
        crate::application::graphs::GraphManagementError::Conflict(_)
    ));
    assert_eq!(
        std::fs::read(state.bundle_root.join("graph.json")).unwrap(),
        original
    );
    next["objective"] = json!("changed workflow behavior");
    let app = router_with_web_root(state.clone(), root.path().join("web"));
    let (status, denied) = call(
        app.clone(),
        "PUT",
        "/graphs/fixture",
        Some(&json!({"definition":next}).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{denied}");
    assert_eq!(
        std::fs::read(state.bundle_root.join("graph.json")).unwrap(),
        original
    );
    sessions
        .retire_channel_assistant("local", &session, "persistent-fixture-run")
        .unwrap();
    let (status, accepted) = call(
        app,
        "PUT",
        "/graphs/fixture",
        Some(&json!({"definition":next}).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{accepted}");
}
