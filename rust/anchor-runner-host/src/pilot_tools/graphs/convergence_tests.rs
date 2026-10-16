use super::*;

fn fixture() -> (tempfile::TempDir, PilotTools) {
    let root = tempfile::tempdir().unwrap();
    let sessions = SessionStore::open(root.path().join("sessions.sqlite")).unwrap();
    let tools = PilotTools {
        application: RunApplication::new(root.path().join("state"), root.path().to_path_buf()),
        graph_name: String::new(),
        library_root: root.path().to_path_buf(),
        data_root: root.path().join("state"),
        workspace_root: root.path().join("work"),
        sessions,
        owner: "local".into(),
        session: "pilot".into(),
        turn: String::new(),
        context: None,
    };
    (root, tools)
}

#[tokio::test]
async fn discovery_and_read_use_application_bundle_validity_and_configured_alias() {
    let _guard = crate::PROCESS_ENV_LOCK.lock().await;
    let (root, mut tools) = fixture();
    tools
        .application
        .create_graph("usable", None)
        .await
        .unwrap();
    let broken = root.path().join("broken");
    std::fs::create_dir(&broken).unwrap();
    std::fs::write(broken.join("graph.json"), r#"{"nodes":[]}"#).unwrap();
    tools.application = tools
        .application
        .with_configured_graph("alias".into(), root.path().join("usable"));
    tools.graph_name = "alias".into();
    let listing = tools.graph_list(&json!({})).await.unwrap();
    assert_eq!(
        listing,
        json!({"graphs":[{"graph":"alias","active_runs":[]}]})
    );
    let application = tools.application.list_graphs().await.unwrap();
    assert_eq!(
        listing["graphs"][0]["graph"],
        application["graphs"][0]["graph"]
    );
    assert!(tools.application.read_graph("broken").is_err());
    assert!(tools.graph_read(&json!({"graph":"broken"})).await.is_err());
    let read = tools.graph_read(&json!({"graph":"alias"})).await.unwrap();
    assert_eq!(
        read["definition"],
        tools.application.read_graph("alias").unwrap()["definition"]
    );
    assert_eq!(read.as_object().unwrap().len(), 2);
}

#[tokio::test]
async fn validation_shares_acceptance_with_application_and_redacts_private_failures() {
    let _guard = crate::PROCESS_ENV_LOCK.lock().await;
    let (_root, tools) = fixture();
    let valid = json!({"entry":"worker","agents":{"worker":{"model":"explicit-legacy-alias"}},"nodes":[{"id":"worker","agent":"worker"}],"edges":[]});
    let mut missing_plugin = valid.clone();
    missing_plugin["nodes"][0]["plugins"] = json!(["not-installed"]);
    for definition in [valid, missing_plugin, json!({"entry":"missing"})] {
        let application = tools.application.validate_graph(&definition);
        let pilot = tools
            .graph_validate(&json!({"definition":definition}))
            .await
            .unwrap();
        assert_eq!(pilot["valid"], application.is_ok());
        if definition["nodes"][0]["plugins"].is_array() {
            assert_eq!(pilot["error"], "invalid Graph or unavailable Plugin");
        }
    }
}
