//! The Pilot tool surface's regression suite.
//!
//! It lives beside the tools it exercises; the table itself stays the single
//! source of authority, which one of these cases asserts directly.

use super::*;

use anchor_platform_session::CreateSession;

fn fixture() -> (tempfile::TempDir, PilotTools) {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("fixture")).unwrap();
    std::fs::write(root.path().join("fixture/graph.json"), r#"{"entry":"idle","ops":{"idle":{"run":"true"}},"nodes":[{"id":"idle","op":"idle"}],"edges":[]}"#).unwrap();
    std::fs::write(
        root.path().join("fixture/manifest.json"),
        r#"{"format":1,"graph":"graph.json","plugins":[]}"#,
    )
    .unwrap();
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
    let tools = PilotTools {
        application: RunApplication::new(root.path().join("state"), root.path().to_path_buf()),
        graph_name: "fixture".into(),
        catalog_root: root.path().to_path_buf(),
        library_root: root.path().to_path_buf(),
        data_root: root.path().join("state"),
        workspace_root: root.path().join("work"),
        sessions,
        owner: "local".into(),
        session: "pilot".into(),
        turn: String::new(),
    };
    (root, tools)
}

#[tokio::test]
async fn assistant_reviews_a_workspace_candidate_without_installing_it() {
    let (root, tools) = fixture();
    let candidate = root.path().join("work/candidate");
    std::fs::create_dir_all(candidate.join("skills/check")).unwrap();
    std::fs::write(
            candidate.join("plugin.json"),
            r#"{"name":"Candidate","description":"from the workspace","skills":"skills","mcpServers":{"probe":{"command":"bin/run","args":["serve"],"env_vars":["CANDIDATE_TOKEN"]}}}"#,
        )
        .unwrap();
    std::fs::write(candidate.join("skills/check/SKILL.md"), "# check\n").unwrap();

    let review = tools
        .execute("plugin_review", json!({"directory":"candidate"}))
        .await
        .unwrap();
    assert_eq!(review["id"], "candidate");
    assert_eq!(review["name"], "Candidate");
    assert_eq!(
        review["resources"],
        json!(["plugin.json", "skills/check/SKILL.md"])
    );
    assert_eq!(review["servers"][0]["name"], "probe");
    assert_eq!(
        review["servers"][0]["environment"],
        json!(["CANDIDATE_TOKEN"])
    );
    assert!(
        !root.path().join("plugins").exists(),
        "a review must not publish anything"
    );
}

#[tokio::test]
async fn assistant_review_refuses_paths_outside_its_workspace() {
    let (root, tools) = fixture();
    let workspace = root.path().join("work");
    std::fs::create_dir_all(workspace.join("candidate")).unwrap();
    std::fs::write(
        workspace.join("candidate/plugin.json"),
        r#"{"name":"Candidate"}"#,
    )
    .unwrap();
    std::fs::write(workspace.join("plain.txt"), "not a directory").unwrap();
    std::os::unix::fs::symlink(root.path().join("fixture"), workspace.join("escape")).unwrap();

    for directory in [
        "/etc",
        "../fixture",
        "candidate/../candidate",
        "escape",
        "plain.txt",
        "missing",
        "",
    ] {
        let error = tools
            .execute("plugin_review", json!({"directory":directory}))
            .await
            .unwrap_err();
        assert!(
            matches!(error, ToolError::Failed(_)),
            "{directory:?} was accepted"
        );
    }

    // The same candidate is accepted once addressed inside the workspace.
    assert!(
        tools
            .execute("plugin_review", json!({"directory":"candidate"}))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn assistant_refuses_to_rewrite_or_delete_its_own_graph() {
    let (_root, tools) = fixture();
    let definition = json!({"entry":"idle","ops":{"idle":{"run":"true"}},"nodes":[{"id":"idle","op":"idle"}],"edges":[]});

    for error in [
        tools
            .execute(
                "graph_update",
                json!({"graph":"fixture","definition":definition}),
            )
            .await
            .unwrap_err(),
        tools
            .execute(
                "graph_create",
                json!({"name":"fixture","definition":definition}),
            )
            .await
            .unwrap_err(),
    ] {
        match error {
            ToolError::Failed(reason) => {
                assert!(reason.contains("defines this assistant"), "{reason}");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    // Reading the assistant's own Graph stays available, and another Graph
    // is not refused by this guard.
    assert!(
        tools
            .execute("graph_read", json!({"graph":"fixture"}))
            .await
            .is_ok()
    );
    let other = tools
        .execute(
            "graph_update",
            json!({"graph":"other","definition":definition}),
        )
        .await
        .unwrap_err();
    match other {
        ToolError::Failed(reason) => assert!(!reason.contains("defines this assistant")),
        other => panic!("expected a Turn-state failure, got {other:?}"),
    }
}

#[tokio::test]
async fn read_tools_do_not_create_runs_or_modify_definitions() {
    let (root, tools) = fixture();
    let original = std::fs::read(root.path().join("fixture/graph.json")).unwrap();
    assert_eq!(tools.definitions().len(), 16);
    assert_eq!(
        tools
            .definitions()
            .iter()
            .filter(|tool| tools.is_read_only(&tool.name))
            .count(),
        10,
        "the retired environment reads took three read-only tools with them"
    );
    assert!(!tools.is_read_only("graph_run"));
    assert_eq!(
        tools
            .execute("graph_read", json!({"graph":"fixture"}))
            .await
            .unwrap()["definition"]["entry"],
        "idle"
    );
    assert_eq!(
        tools
            .execute(
                "graph_validate",
                json!({"definition":serde_json::from_slice::<Value>(&original).unwrap()})
            )
            .await
            .unwrap()["valid"],
        true
    );
    assert_eq!(
        tools.execute("plugin_list", json!({})).await.unwrap(),
        json!({"plugins":[]})
    );
    assert_eq!(
        tools.execute("run_list", json!({})).await.unwrap(),
        json!({"runs":[]})
    );
    assert_eq!(
        tools.execute("session_wait", json!({})).await.unwrap()["session"]["id"],
        "pilot"
    );
    assert!(
        tools
            .execute("graph_run", json!({"graph":"fixture"}))
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read(root.path().join("fixture/graph.json")).unwrap(),
        original
    );
    assert!(!root.path().join("state").exists());
}

#[tokio::test]
async fn graph_mutations_require_an_owned_running_turn_and_do_not_start_runs() {
    let (root, tools) = fixture();
    let definition = json!({"entry":"idle","ops":{"idle":{"run":"true"}},
            "nodes":[{"id":"idle","op":"idle"}],"edges":[]});
    assert!(
        tools
            .execute(
                "graph_create",
                json!({"name":"new","definition":definition})
            )
            .await
            .is_err()
    );
    let (turn, _) = tools
        .sessions
        .create_turn("local", "pilot", "write-fixture", Some("create a Graph"))
        .unwrap();
    let mut tools = tools.for_turn(&turn.id);
    tools.owner = "responses-other".into();
    assert!(
        tools
            .execute(
                "graph_create",
                json!({"name":"new","definition":definition})
            )
            .await
            .is_err()
    );
    assert!(!root.path().join("new").exists());
    tools.owner = "local".into();
    let created = tools
        .execute(
            "graph_create",
            json!({"name":"new","definition":definition}),
        )
        .await
        .unwrap();
    assert_eq!(created["graph"], "new");
    assert!(tools.application.records().unwrap().is_empty());
    let mut updated = definition;
    updated["objective"] = json!("updated only, not executed");
    tools
        .execute("graph_update", json!({"graph":"new","definition":updated}))
        .await
        .unwrap();
    assert_eq!(
        tools
            .execute("graph_read", json!({"graph":"new"}))
            .await
            .unwrap()["definition"],
        updated
    );
    assert!(tools.application.records().unwrap().is_empty());
    assert!(
        tools
            .execute("graph_delete", json!({"graph":"new"}))
            .await
            .is_err()
    );
    tools
        .sessions
        .finish_turn(
            "local",
            "pilot",
            &turn.id,
            anchor_platform_session::TurnStatus::Completed,
            None,
        )
        .unwrap();
    assert!(
        tools
            .execute("graph_update", json!({"graph":"new","definition":updated}))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn graph_paths_symlinks_and_plugin_credentials_fail_closed() {
    let (root, tools) = fixture();
    assert!(
        tools
            .execute("graph_read", json!({"graph":"../secret"}))
            .await
            .is_err()
    );
    std::fs::write(root.path().join("secret.json"), "host-secret").unwrap();
    std::fs::remove_file(root.path().join("fixture/graph.json")).unwrap();
    std::os::unix::fs::symlink(
        root.path().join("secret.json"),
        root.path().join("fixture/graph.json"),
    )
    .unwrap();
    assert!(
        tools
            .execute("graph_read", json!({"graph":"fixture"}))
            .await
            .is_err()
    );
    let plugin = root.path().join("plugins/demo");
    std::fs::create_dir_all(&plugin).unwrap();
    std::fs::write(plugin.join("plugin.json"), r#"{"name":"Demo","mcpServers":{"remote":{"url":"https://private.invalid/?key=do-not-expose","headers":{"Authorization":"private-key"}}}}"#).unwrap();
    let public = tools
        .execute("plugin_list", json!({}))
        .await
        .unwrap()
        .to_string();
    assert!(
        !public.contains("private-key")
            && !public.contains("private.invalid")
            && !public.contains("do-not-expose"),
        "{public}"
    );
}

/// The retired environment tools are gone, not merely disabled: an unknown name
/// is refused by the dispatcher and no definition advertises them.
#[tokio::test]
async fn the_retired_environment_tools_no_longer_exist() {
    let (_root, tools) = fixture();
    for name in [
        "environment_list",
        "environment_request",
        "environment_references",
        "environment_show",
        "environment_publish",
        "dependency_provision",
    ] {
        let error = tools.execute(name, json!({})).await.unwrap_err();
        assert!(
            error.to_string().contains("is not registered")
                || error.to_string().contains("unknown tool"),
            "`{name}` must not be a tool any more, got {error}"
        );
    }
}
