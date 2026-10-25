use super::*;

#[tokio::test]
async fn channel_plugin_status_readers_share_a_lease_without_allowing_graph_writes() {
    let _environment = PROCESS_ENV.lock().await;
    let (root, state) = fixture();
    set_host_env(root.path());
    let (_, session, _) = saved_assistant(&state, "local");
    let before = fixtures::record(&state.application, "persistent-fixture-run");
    let graph_path = state.bundle_root.join("graph.json");
    let graph_before = std::fs::read(&graph_path).unwrap();
    let reader =
        crate::assistant_plugins::graph_read_lease(&state.data_root, &state.bundle_root).unwrap();
    let app = router_with_web_root(state.clone(), root.path().join("web"));
    let endpoint = format!("/channel-sessions/{session}/assistant");
    let (first, second) = tokio::join!(
        call(app.clone(), "GET", &endpoint, None),
        call(app.clone(), "GET", &endpoint, None),
    );
    for (status, current) in [first, second] {
        assert_eq!(status, StatusCode::OK, "{current}");
        assert_eq!(current["plugin_update"]["state"], "current", "{current}");
    }
    let denied = state
        .application
        .update_graph("fixture", definition())
        .await
        .unwrap_err();
    assert!(matches!(
        denied,
        crate::application::graphs::GraphManagementError::Conflict(_)
    ));
    assert_eq!(std::fs::read(&graph_path).unwrap(), graph_before);
    assert_eq!(
        fixtures::record(&state.application, "persistent-fixture-run"),
        before
    );
    drop(reader);

    let writer =
        crate::assistant_plugins::graph_lease(&state.data_root, &state.bundle_root).unwrap();
    let (status, current) = call(app.clone(), "GET", &endpoint, None).await;
    assert_eq!(status, StatusCode::OK, "{current}");
    assert_eq!(current["plugin_update"]["state"], "unavailable");
    drop(writer);

    let (status, current) = call(app, "GET", &endpoint, None).await;
    assert_eq!(status, StatusCode::OK, "{current}");
    assert_eq!(current["plugin_update"]["state"], "current");
    assert_eq!(
        fixtures::record(&state.application, "persistent-fixture-run"),
        before
    );
}

#[tokio::test]
async fn operator_plugin_save_preserves_run_and_delivery_and_reports_pending() {
    let _environment = PROCESS_ENV.lock().await;
    let (root, state) = fixture();
    set_host_env(root.path());
    let (sessions, session, turn) = saved_assistant(&state, "local");
    let plugin = state
        .application
        .library_root()
        .to_path_buf()
        .join("plugins/docmost");
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
    let before = fixtures::record(&state.application, "persistent-fixture-run");
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
        fixtures::record(&state.application, "persistent-fixture-run"),
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
    // Re-selecting a previously published plugin set must approve its current
    // Host declarations, even when the authoring Graph digest is unchanged.
    let (_, bundle) = state.application.load_graph("fixture").unwrap();
    let digest = bundle.snapshot.digest().unwrap();
    let approved =
        crate::assistant_plugins::publication(&state.data_root, &state.bundle_root, &digest)
            .unwrap()
            .unwrap();
    std::fs::write(
        plugin.join("plugin.json"),
        r#"{"name":"Docmost","mutable":["cache/"]}"#,
    )
    .unwrap();
    let denied = state
        .application
        .update_graph_from_agent("fixture", next.clone())
        .await;
    assert!(
        denied.is_ok(),
        "an unchanged Agent draft may save without approving permissions"
    );
    assert_eq!(
        crate::assistant_plugins::publication(&state.data_root, &state.bundle_root, &digest)
            .unwrap()
            .unwrap(),
        approved
    );
    let (status, removed) = call(
        app.clone(),
        "PUT",
        "/graphs/fixture",
        Some(&json!({"definition":definition()}).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{removed}");
    let (status, saved_again) = call(
        app,
        "PUT",
        "/graphs/fixture",
        Some(&json!({"definition":next}).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved_again}");
    let refreshed =
        crate::assistant_plugins::publication(&state.data_root, &state.bundle_root, &digest)
            .unwrap()
            .unwrap();
    assert!(!approved.bindings["docmost"].same_authorization(&refreshed.bindings["docmost"]));
    assert_eq!(
        fixtures::record(&state.application, "persistent-fixture-run"),
        before
    );
    assert_eq!(
        sessions
            .list_unfinished_channel_deliveries("local", &session)
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn agent_grant_and_structural_save_require_explicit_version_boundary() {
    let _environment = PROCESS_ENV.lock().await;
    let (root, state) = fixture();
    set_host_env(root.path());
    let (sessions, session, _) = saved_assistant(&state, "local");
    let plugin = state
        .application
        .library_root()
        .to_path_buf()
        .join("plugins/docmost");
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
