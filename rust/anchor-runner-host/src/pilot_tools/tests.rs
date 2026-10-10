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
    assert_eq!(tools.definitions().len(), 20);
    assert_eq!(
        tools
            .definitions()
            .iter()
            .filter(|tool| tools.is_read_only(&tool.name))
            .count(),
        13
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
#[tokio::test]
async fn a_requirement_for_a_name_the_assistants_called_subgraph_uses_is_refused() {
    let (root, mut tools) = fixture();
    // The own-dependency guard loads bundles, which needs the run store.
    std::fs::create_dir_all(root.path().join("state/runs")).unwrap();
    // Recording a need is a host fact a turn owns, so the turn must run.
    let (turn, _) = tools
        .sessions
        .create_turn("local", "pilot", "fixture-request", Some("fixture"))
        .unwrap();
    tools.turn = turn.id;
    // The assistant's own Graph calls `sub`, and `sub` declares the name.
    std::fs::create_dir_all(root.path().join("sub")).unwrap();
    std::fs::write(
        root.path().join("sub/graph.json"),
        json!({
            "objective": "sub",
            "entry": "inside",
            "agents": {"worker": {"model": "test"}},
            "ops": {},
            "nodes": [{"id": "inside", "agent": "worker", "environment": "research"}],
            "edges": []
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        root.path().join("sub/manifest.json"),
        json!({"format": 1, "graph": "graph.json", "plugins": []}).to_string(),
    )
    .unwrap();
    std::fs::write(
        root.path().join("fixture/graph.json"),
        json!({
            "objective": "assistant",
            "entry": "work",
            "agents": {"worker": {"model": "test"}},
            "ops": {"work": {"call": {"graph": "sub", "mode": "wait"}}},
            "nodes": [{"id": "work", "op": "work"}],
            "edges": []
        })
        .to_string(),
    )
    .unwrap();

    // Asking for the name its called Graph uses would let the assistant choose
    // its own dependency content, so it is refused.
    let refused = tools
        .execute(
            "environment_request",
            json!({"name":"research","purpose":"would change itself"}),
        )
        .await;
    assert!(refused.is_err(), "{refused:?}");
    // A name nobody in its execution closure uses is still allowed.
    let allowed = tools
        .execute(
            "environment_request",
            json!({"name":"unrelated","purpose":"a real need"}),
        )
        .await;
    assert!(allowed.is_ok(), "{allowed:?}");
}

#[tokio::test]
async fn a_requirement_refuses_an_interpreter_specifier_it_could_never_satisfy() {
    let (root, mut tools) = fixture();
    std::fs::create_dir_all(root.path().join("state/runs")).unwrap();
    let (turn, _) = tools
        .sessions
        .create_turn("local", "pilot", "fixture-specifier", Some("fixture"))
        .unwrap();
    tools.turn = turn.id;
    let failure = tools
        .execute(
            "environment_request",
            json!({
                "name": "research",
                "purpose": "analysis",
                "interpreter": "python>=3.11",
            }),
        )
        .await
        .expect_err("a specifier must be refused when it is recorded");
    assert!(
        failure.to_string().contains("version prefix"),
        "the refusal explains the accepted shape: {failure}"
    );
}

#[tokio::test]
async fn the_assistant_records_an_environment_requirement_without_publishing() {
    let (root, mut tools) = fixture();
    // The own-dependency guard loads bundles, which needs the run store.
    std::fs::create_dir_all(root.path().join("state/runs")).unwrap();
    // Recording a need is a host fact a turn owns, so the turn must run.
    let (turn, _) = tools
        .sessions
        .create_turn("local", "pilot", "fixture-request", Some("fixture"))
        .unwrap();
    tools.turn = turn.id;
    let requested = tools
        .execute(
            "environment_request",
            json!({
                "name":"research",
                "purpose":"parse the weekly reports",
                "interpreter":"3.11",
                "packages":["pandas"]
            }),
        )
        .await
        .unwrap();
    assert_eq!(requested["requested"], true);
    assert_eq!(requested["name"], "research");
    let recorded =
        crate::application::environments::list_environment_requirements(&root.path().join("state"))
            .unwrap();
    assert_eq!(recorded.unreadable, 0);
    assert_eq!(recorded.requirements.len(), 1);
    assert_eq!(recorded.requirements[0].name, "research");
    assert_eq!(recorded.requirements[0].requests.len(), 1);
    assert_eq!(
        recorded.requirements[0].requests[0].packages,
        vec!["pandas"]
    );
    assert_eq!(recorded.requirements[0].format, 1);
    assert!(recorded.requirements[0].observed.is_none());

    // A model-supplied approval field, or a malformed ask, is refused.
    for refused in [
        json!({"name":"research","purpose":"x","confirm":true}),
        json!({"name":"research"}),
        json!({"name":"re search","purpose":"x"}),
        json!({"name":"research","purpose":""}),
        json!({"name":"research","purpose":"x","packages":"pandas"}),
        json!({"name":"research","purpose":"x","packages":["bad package!"]}),
        json!({"name":"research","purpose":"x","interpreter":""}),
    ] {
        assert!(
            tools
                .execute("environment_request", refused.clone())
                .await
                .is_err(),
            "{refused} must be refused"
        );
    }
    // Nothing was published: the requirement is a request, not an asset.
    let store = anchor_library::EnvironmentStore::new(root.path());
    assert!(store.names().unwrap().is_empty());
    assert!(
        store.list().unwrap().is_empty(),
        "recording a need must not publish any content"
    );
}

#[tokio::test]
async fn the_assistant_sees_environments_without_changing_them() {
    let (root, tools) = fixture();
    let prepared = root.path().join("prepared");
    std::fs::create_dir_all(prepared.join("bin")).unwrap();
    std::fs::write(prepared.join("marker.txt"), "one").unwrap();
    let store = anchor_library::EnvironmentStore::new(root.path());
    let published = store.publish(&prepared).unwrap();
    store.set_name("research", &published.id, None).unwrap();

    let listed = tools.execute("environment_list", json!({})).await.unwrap();
    assert_eq!(listed["names"][0]["name"], "research");
    assert_eq!(listed["names"][0]["id"], published.id);
    assert_eq!(listed["names"][0]["version"], 1);
    assert_eq!(listed["environments"][0]["id"], published.id);

    // By name and by id describe the same published content, and the digest is
    // re-verified rather than assumed.
    let by_name = tools
        .execute("environment_show", json!({"name":"research"}))
        .await
        .unwrap();
    assert_eq!(by_name["id"], published.id);
    assert_eq!(by_name["verified"], true);
    let by_id = tools
        .execute("environment_show", json!({"id":published.id}))
        .await
        .unwrap();
    assert_eq!(by_id["id"], published.id);

    // Unknown or ambiguous requests are refused, and neither tool writes.
    let missing = tools
        .execute("environment_show", json!({"name":"missing"}))
        .await
        .unwrap_err();
    assert!(
        missing.to_string().contains("no dependency environment"),
        "{missing}"
    );
    let both = tools
        .execute(
            "environment_show",
            json!({"id":published.id,"name":"research"}),
        )
        .await
        .unwrap_err();
    assert!(both.to_string().contains("exactly one"), "{both}");
    assert!(!tools.is_read_only("plugin_install"));
    assert!(tools.is_read_only("environment_list"));
    assert!(tools.is_read_only("environment_show"));
}

#[test]
fn the_tool_table_is_the_only_source_of_authority() {
    let names = PILOT_TOOLS.iter().map(|tool| tool.name).collect::<Vec<_>>();
    let unique = names.iter().collect::<std::collections::BTreeSet<_>>();
    assert_eq!(names.len(), unique.len(), "tool names must be unique");

    let inspect = PILOT_TOOLS
        .iter()
        .filter(|tool| tool.authority == ToolAuthority::Inspect)
        .count();
    let mutate = PILOT_TOOLS
        .iter()
        .filter(|tool| tool.authority == ToolAuthority::Mutate)
        .count();
    let confirm = PILOT_TOOLS
        .iter()
        .filter(|tool| tool.authority == ToolAuthority::Confirm)
        .map(|tool| tool.name.to_owned())
        .collect::<std::collections::BTreeSet<_>>();
    // The confirmed tools are exactly the ones the interaction layer owns, so
    // a new confirmed tool cannot be added to one list and forgotten in the
    // other — which is what made "approval is never a tool argument" a
    // property of a list rather than of the design.
    let interactive = crate::goose_acp::pilot_interaction::definitions();
    let interactive = interactive
        .into_iter()
        .map(|definition| definition.name.to_string())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(confirm, interactive);

    // The advertised schema of a confirmed tool must be the one the
    // interaction layer shows, or the model is told something else than the
    // human is asked.
    for tool in PILOT_TOOLS
        .iter()
        .filter(|tool| tool.authority == ToolAuthority::Confirm)
    {
        let advertised = crate::goose_acp::pilot_interaction::definitions()
            .into_iter()
            .find(|definition| definition.name == tool.name)
            .unwrap_or_else(|| panic!("{} is not advertised", tool.name));
        assert_eq!(
            advertised.parameters,
            (tool.parameters)(),
            "{} schema drifted",
            tool.name
        );
    }

    let (root, tools) = fixture();
    let _ = root;
    assert_eq!(tools.definitions().len(), inspect + mutate);
    assert_eq!(
        tools
            .definitions()
            .iter()
            .filter(|tool| tools.is_read_only(&tool.name))
            .count(),
        inspect
    );
    assert_eq!((inspect, mutate, confirm.len()), (13, 7, 5));
    // The two axes must agree: everything readable is Inspect, and every
    // Inspect tool is read-only. "Only assets need a human click" is then a
    // property of the table, not a sentence in a document.
    for tool in PILOT_TOOLS {
        assert_eq!(
            tool.authority == ToolAuthority::Inspect,
            tool.effect == ToolEffect::Read,
            "{} authority and effect disagree",
            tool.name
        );
    }
    let host_facts = PILOT_TOOLS
        .iter()
        .filter(|tool| tool.effect == ToolEffect::HostFact)
        .map(|tool| tool.name)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        host_facts,
        ["ask_user", "environment_request"]
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>(),
        "a host fact changes no asset and needs no approval"
    );
}
