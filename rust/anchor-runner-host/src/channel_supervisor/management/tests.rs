use super::*;
use anchor_platform_session::{CreateSession, Session, SessionStore};
use std::{sync::Arc, time::Duration};

const FIELDS: &[&str] = &[
    "WECOM_BOT_ID",
    "WECOM_BOT_SECRET",
    "ANCHOR_WECOM_USERS",
    "ANCHOR_WECOM_SEND_USERS",
];

use crate::test_support::Environment;

/// Two bot Graphs, `alpha` and `beta`, mount the same channel Plugin.
struct Fixture {
    root: tempfile::TempDir,
    adapter: Arc<ChannelManagementAdapter>,
    sessions: SessionStore,
    session: Session,
    turn: String,
}

const ASSISTANT: &str = r#"{"entry":"wait","agents":{"reply":{"model":"fixture","instructions":"fixture"}},"ops":{"wait":{"host":{"operation":"session.wait_input"}},"deliver":{"host":{"operation":"session.reply"}}},"nodes":[{"id":"wait","op":"wait"},{"id":"reply","agent":"reply","plugins":["gateway"]},{"id":"deliver","op":"deliver"}],"edges":[{"from":"wait","to":"reply"},{"from":"reply","to":"deliver"},{"from":"deliver","to":"wait"}]}"#;

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let plugin = root.path().join("library/plugins/gateway");
        fs::create_dir_all(plugin.join("bin")).unwrap();
        fs::write(
            plugin.join("plugin.json"),
            json!({"name":"Fixture gateway"}).to_string(),
        )
        .unwrap();
        fs::write(plugin.join("channel.json"), json!({"platform":"wecom","transport":"websocket","entrypoint":"bin/gateway","required_environment":["WECOM_BOT_ID","WECOM_BOT_SECRET"]}).to_string()).unwrap();
        // Each Graph's gateway reports into its own state directory.
        fs::write(
            plugin.join("bin/gateway"),
            r#"#!/bin/sh
o="$WECOM_CHANNEL_STATE/observed"
mkdir -p "$o"
printf '%s' "$WECOM_BOT_ID" > "$o/account"
printf '%s' "$WECOM_BOT_SECRET" > "$o/secret"
printf '%s' "$ANCHOR_API_KEY" > "$o/callback"
printf '%s' "$ANCHOR_CHANNEL_WEBHOOK_URL" > "$o/webhook"
printf '%s' "$ANCHOR_WECOM_USERS" > "$o/users"
printf '%s' "$ANCHOR_WECOM_SEND_USERS" > "$o/send-users"
printf 'x' >> "$o/starts"
trap 'printf stopped > "$o/stopped"; exit 0' TERM
while :; do sleep 0.02; done
"#,
        )
        .unwrap();
        fs::set_permissions(
            plugin.join("bin/gateway"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        for graph in ["alpha", "beta"] {
            Self::graph(root.path(), graph, ASSISTANT);
        }
        // A one-shot Graph that mounts the Plugin is not a bot.
        Self::graph(
            root.path(),
            "task",
            r#"{"entry":"reply","agents":{"reply":{"model":"fixture","instructions":"fixture"}},"nodes":[{"id":"reply","agent":"reply","plugins":["gateway"]}],"edges":[]}"#,
        );
        fs::create_dir_all(root.path().join("state/platform")).unwrap();
        let sessions =
            SessionStore::open(root.path().join("state/platform/sessions.sqlite")).unwrap();
        let session = sessions
            .create(
                "local",
                CreateSession {
                    id: Some("pilot".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        let (turn, _) = sessions
            .create_turn("local", "pilot", "initialize-channel", None)
            .unwrap();
        let adapter =
            Arc::new(ChannelManagementAdapter::open(Self::configuration(root.path())).unwrap());
        Self {
            root,
            adapter,
            sessions,
            session,
            turn: turn.id,
        }
    }

    fn graph(root: &Path, name: &str, graph: &str) {
        let directory = root.join("catalog").join(name);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("graph.json"), graph).unwrap();
        fs::write(
            directory.join("manifest.json"),
            json!({"format":1,"graph":"graph.json","plugins":[{"id":"gateway"}]}).to_string(),
        )
        .unwrap();
    }

    fn configuration(root: &Path) -> ChannelHostConfiguration {
        ChannelHostConfiguration {
            data_root: root.join("state"),
            catalog_root: root.join("catalog"),
            configured_graph: None,
            library_root: root.join("library"),
            listen: "127.0.0.1:8077".into(),
        }
    }

    fn binding(user: &str) -> ChannelBinding {
        ChannelBinding {
            plugin: "gateway".into(),
            users: vec![user.into()],
            send_users: vec![user.into()],
            delegate_graphs: Vec::new(),
        }
    }

    async fn configure(&self, graph: &str, user: &str) {
        self.adapter
            .configure("local", graph, Self::binding(user))
            .await
            .unwrap();
    }

    fn credential(&self, graph: &str, field: &str, value: &str) {
        let request = crate::credential_store::request(
            &self.root.path().join("state"),
            &self.root.path().join("library"),
            &self.sessions,
            "local",
            &self.session,
            &self.turn,
            "gateway",
            &crate::credential_store::channel_target(PLATFORM, graph),
            field,
            "configure local test gateway",
        )
        .unwrap();
        crate::credential_store::submit(
            &self.root.path().join("state"),
            &self.root.path().join("library"),
            &self.sessions,
            "local",
            &self.session,
            &request.id,
            value,
        )
        .unwrap();
    }

    /// Configure `graph` for `user` with bot `bot`.
    async fn prepare(&self, graph: &str, user: &str, bot: &str) {
        self.configure(graph, user).await;
        self.credential(graph, "WECOM_BOT_ID", bot);
        self.credential(graph, "WECOM_BOT_SECRET", &format!("{bot}-secret"));
    }

    fn observed(&self, graph: &str, name: &str) -> PathBuf {
        super::super::state_directory(&self.root.path().join("state"), graph)
            .join("observed")
            .join(name)
    }

    fn read(&self, graph: &str, name: &str) -> String {
        fs::read_to_string(self.observed(graph, name)).unwrap()
    }

    async fn wait_for(&self, graph: &str, name: &str) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while !self.observed(graph, name).is_file() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
}

async fn status(fixture: &Fixture, graph: &str) -> Value {
    fixture.adapter.status("local", graph).await.unwrap()
}

#[tokio::test]
async fn two_bot_graphs_run_side_by_side_with_their_own_credentials_members_and_routes() {
    let _guard = crate::PROCESS_ENV_LOCK.lock().await;
    let _environment = Environment::cleared(FIELDS);
    let fixture = Fixture::new();
    let before = fixture.sessions.get("local", "pilot").unwrap();
    fixture.prepare("alpha", "alice", "private-alpha-bot").await;
    fixture.prepare("beta", "bob", "private-beta-bot").await;
    let alpha = status(&fixture, "alpha").await;
    assert_eq!(alpha["enabled"], false);
    assert_eq!(alpha["reply_node"], "reply");
    assert!(
        alpha["credentials"]
            .as_array()
            .unwrap()
            .iter()
            .all(|field| field["configured"] == true)
    );
    assert!(!alpha.to_string().contains("private-"));
    for graph in ["alpha", "beta"] {
        fixture
            .adapter
            .set_enabled("local", graph, true)
            .await
            .unwrap();
        fixture.wait_for(graph, "starts").await;
    }
    for (graph, user, bot) in [
        ("alpha", "alice", "private-alpha-bot"),
        ("beta", "bob", "private-beta-bot"),
    ] {
        assert_eq!(fixture.read(graph, "account"), bot);
        assert_eq!(fixture.read(graph, "secret"), format!("{bot}-secret"));
        assert_eq!(fixture.read(graph, "users"), user);
        assert_eq!(fixture.read(graph, "send-users"), user);
        assert!(
            fixture
                .read(graph, "webhook")
                .ends_with(&format!("/channels/{graph}/events"))
        );
    }
    let alpha_key = fixture.read("alpha", "callback");
    let beta_key = fixture.read("beta", "callback");
    assert_ne!(alpha_key, beta_key);
    for (key, graph) in [(&alpha_key, "alpha"), (&beta_key, "beta")] {
        let callback = fixture.adapter.callback(key).unwrap();
        assert_eq!(
            (callback.graph.as_str(), callback.owner.as_str()),
            (graph, "local")
        );
    }
    assert!(fixture.adapter.callback("wrong").is_none());
    let snapshots = fixture.adapter.snapshots().unwrap();
    assert_eq!(snapshots.len(), 2);
    assert!(snapshots.iter().all(|state| state.enabled));
    assert_ne!(snapshots[0].account, snapshots[1].account);
    let listed = fixture.adapter.list("local").await.unwrap();
    assert_eq!(listed["channels"].as_array().unwrap().len(), 2);
    let after = fixture.sessions.get("local", "pilot").unwrap();
    assert_eq!(before.network_allowed, after.network_allowed);
    assert_eq!(before.plugin_mounts, after.plugin_mounts);

    // Stopping one bot leaves the other running and its key valid.
    fixture
        .adapter
        .set_enabled("local", "alpha", false)
        .await
        .unwrap();
    fixture.wait_for("alpha", "stopped").await;
    assert!(fixture.adapter.callback(&alpha_key).is_none());
    assert!(fixture.adapter.callback(&beta_key).is_some());
    assert!(!fixture.observed("beta", "stopped").exists());
    assert!(
        fixture
            .adapter
            .snapshot("alpha")
            .unwrap()
            .unwrap()
            .descriptor
            .is_none()
    );
    fixture.adapter.shutdown().await;
}

#[tokio::test]
async fn one_bot_account_cannot_serve_two_enabled_graphs() {
    let _guard = crate::PROCESS_ENV_LOCK.lock().await;
    let _environment = Environment::cleared(FIELDS);
    let fixture = Fixture::new();
    fixture.prepare("alpha", "alice", "shared-bot").await;
    fixture.prepare("beta", "bob", " shared-bot ").await;
    fixture
        .adapter
        .set_enabled("local", "alpha", true)
        .await
        .unwrap();
    let refused = fixture.adapter.set_enabled("local", "beta", true).await;
    assert!(
        matches!(&refused, Err(ApplicationError::Conflict(reason)) if reason.contains("alpha")),
        "{:?}",
        refused.err()
    );
    assert_eq!(status(&fixture, "beta").await["enabled"], false);
    fixture
        .adapter
        .set_enabled("local", "alpha", false)
        .await
        .unwrap();
    fixture
        .adapter
        .set_enabled("local", "beta", true)
        .await
        .unwrap();
    fixture.adapter.shutdown().await;
}

#[tokio::test]
async fn an_account_with_sessions_of_another_graph_cannot_move() {
    let _guard = crate::PROCESS_ENV_LOCK.lock().await;
    let _environment = Environment::cleared(FIELDS);
    let fixture = Fixture::new();
    fixture.prepare("alpha", "alice", "moving-bot").await;
    fixture.prepare("beta", "alice", "moving-bot").await;
    let account = wecom_account_identity("moving-bot");
    fixture
        .sessions
        .create(
            "local",
            CreateSession {
                id: Some("alpha-chat".into()),
                title: "alpha chat".into(),
                graph: "alpha".into(),
                channel: [
                    ("source".to_owned(), PLATFORM.to_owned()),
                    ("account".to_owned(), account.clone()),
                ]
                .into(),
                ..Default::default()
            },
        )
        .unwrap();
    let refused = fixture.adapter.set_enabled("local", "beta", true).await;
    assert!(
        matches!(&refused, Err(ApplicationError::Conflict(reason)) if reason.contains("alpha chat")),
        "{:?}",
        refused.err()
    );
    // Its own Graph's Sessions do not block it.
    fixture
        .adapter
        .set_enabled("local", "alpha", true)
        .await
        .unwrap();
    fixture.adapter.shutdown().await;
}

#[tokio::test]
async fn only_persistent_assistants_with_complete_credentials_become_bots() {
    let _guard = crate::PROCESS_ENV_LOCK.lock().await;
    let _environment = Environment::cleared(FIELDS);
    let fixture = Fixture::new();
    assert!(matches!(
        fixture
            .adapter
            .configure("local", "task", Fixture::binding("alice"))
            .await,
        Err(ApplicationError::Invalid(reason)) if reason == crate::application::channel_management::NOT_A_BOT
    ));
    assert!(
        !fixture
            .root
            .path()
            .join("state/platform/channels/graphs/task.json")
            .exists()
    );
    assert!(matches!(
        fixture
            .adapter
            .configure("local", "../alpha", Fixture::binding("alice"))
            .await,
        Err(ApplicationError::Invalid(_))
    ));
    fixture.configure("alpha", "alice").await;
    assert!(matches!(
        fixture.adapter.set_enabled("local", "alpha", true).await,
        Err(ApplicationError::Invalid(_))
    ));
    // Another Graph's credentials are not this Graph's.
    fixture.credential("beta", "WECOM_BOT_ID", "beta-bot");
    fixture.credential("beta", "WECOM_BOT_SECRET", "beta-secret");
    fixture.credential("alpha", "WECOM_BOT_ID", "alpha-bot");
    assert!(matches!(
        fixture.adapter.set_enabled("local", "alpha", true).await,
        Err(ApplicationError::Invalid(_))
    ));
    assert!(!fixture.adapter.snapshot("alpha").unwrap().unwrap().enabled);
    assert_eq!(status(&fixture, "unbound").await["configured"], false);
}
#[tokio::test]
async fn bindings_reload_per_graph_and_reject_another_owner_or_live_rebinding() {
    let _guard = crate::PROCESS_ENV_LOCK.lock().await;
    let _environment = Environment::cleared(FIELDS);
    let fixture = Fixture::new();
    fixture.prepare("alpha", "alice", "alpha-bot").await;
    fixture.prepare("beta", "bob", "beta-bot").await;
    fixture
        .adapter
        .set_enabled("local", "alpha", true)
        .await
        .unwrap();
    fixture.wait_for("alpha", "starts").await;
    let callback = fixture.read("alpha", "callback");
    assert!(matches!(
        fixture
            .adapter
            .configure("local", "alpha", Fixture::binding("carol"))
            .await,
        Err(ApplicationError::Conflict(_))
    ));
    assert!(matches!(
        fixture.adapter.set_enabled("other", "alpha", false).await,
        Err(ApplicationError::Conflict(_))
    ));
    let hidden = fixture.adapter.status("other", "alpha").await.unwrap();
    assert_eq!(hidden["owner_matches"], false);
    assert!(hidden.get("binding").is_none());
    assert!(hidden.get("credentials").is_none());
    assert!(matches!(
        fixture.adapter.remove("alpha").await,
        Err(ApplicationError::Conflict(_))
    ));
    fixture.adapter.shutdown().await;
    let Fixture { root, adapter, .. } = fixture;
    // One Host owns the configuration at a time.
    assert!(ChannelManagementAdapter::open(Fixture::configuration(root.path())).is_err());
    drop(adapter);
    let reopened = ChannelManagementAdapter::open(Fixture::configuration(root.path())).unwrap();
    assert!(reopened.snapshot("alpha").unwrap().unwrap().enabled);
    assert!(!reopened.snapshot("beta").unwrap().unwrap().enabled);
    assert_eq!(reopened.callback(&callback).unwrap().graph, "alpha");
    reopened.resume().await.unwrap();
    let starts =
        super::super::state_directory(&root.path().join("state"), "alpha").join("observed/starts");
    tokio::time::timeout(Duration::from_secs(3), async {
        while fs::read_to_string(&starts).unwrap_or_default().len() < 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    reopened.set_enabled("local", "alpha", false).await.unwrap();
    let path = root
        .path()
        .join("state/platform/channels/graphs/alpha.json");
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );

    // Removing the Graph's binding deletes its credentials and gateway state
    // and leaves the other Graph's intact.
    reopened.remove("alpha").await.unwrap();
    assert!(!path.exists());
    assert!(reopened.snapshot("alpha").unwrap().is_none());
    assert!(!super::super::state_directory(&root.path().join("state"), "alpha").exists());
    let credentials = root
        .path()
        .join("state/platform/credentials")
        .join(crate::credential_store::hash(
            crate::credential_store::binding_owner("local").as_bytes(),
        ))
        .join("gateway/channel_wecom");
    assert!(!credentials.join("alpha").exists());
    assert!(credentials.join("beta/WECOM_BOT_ID.json").is_file());
    reopened.remove("alpha").await.unwrap();
}

#[tokio::test]
async fn concurrent_enables_start_one_gateway_and_disabling_waits_for_shutdown() {
    let _guard = crate::PROCESS_ENV_LOCK.lock().await;
    let _environment = Environment::cleared(FIELDS);
    let fixture = Fixture::new();
    fixture.prepare("alpha", "alice", "alpha-bot").await;
    let (first, second) = tokio::join!(
        fixture.adapter.set_enabled("local", "alpha", true),
        fixture.adapter.set_enabled("local", "alpha", true)
    );
    first.unwrap();
    second.unwrap();
    fixture.wait_for("alpha", "starts").await;
    let previous_key = fixture.read("alpha", "callback");
    assert_eq!(fixture.read("alpha", "starts"), "x");
    let (disabled, enabled) = tokio::join!(
        fixture.adapter.set_enabled("local", "alpha", false),
        fixture.adapter.set_enabled("local", "alpha", true)
    );
    disabled.unwrap();
    enabled.unwrap();
    assert!(fixture.adapter.callback(&previous_key).is_none());
    fixture
        .adapter
        .set_enabled("local", "alpha", false)
        .await
        .unwrap();
    assert_eq!(status(&fixture, "alpha").await["listener"], "stopped");
}

#[tokio::test]
async fn a_graph_that_stops_being_a_bot_stops_its_gateway_and_keeps_its_intent() {
    let _guard = crate::PROCESS_ENV_LOCK.lock().await;
    let _environment = Environment::cleared(FIELDS);
    for remove_graph in [true, false] {
        let fixture = Fixture::new();
        fixture.prepare("alpha", "alice", "alpha-bot").await;
        fixture
            .adapter
            .set_enabled("local", "alpha", true)
            .await
            .unwrap();
        fixture.wait_for("alpha", "starts").await;
        if remove_graph {
            fs::remove_dir_all(fixture.root.path().join("catalog/alpha")).unwrap();
        } else {
            fs::remove_file(
                fixture
                    .root
                    .path()
                    .join("library/plugins/gateway/plugin.json"),
            )
            .unwrap();
        }
        fixture.wait_for("alpha", "stopped").await;
        let status = status(&fixture, "alpha").await;
        assert_eq!(status["enabled"], true);
        assert_eq!(status["available"], false);
        assert_eq!(status["listener"], "unavailable");
        assert!(!fixture.adapter.snapshot("alpha").unwrap().unwrap().enabled);
        fixture
            .adapter
            .set_enabled("local", "alpha", false)
            .await
            .unwrap();
        fixture.adapter.shutdown().await;
    }
}

#[tokio::test]
async fn changed_channel_declarations_disable_the_binding_until_reconfigured() {
    let _guard = crate::PROCESS_ENV_LOCK.lock().await;
    let _environment = Environment::cleared(FIELDS);
    let fixture = Fixture::new();
    fixture.prepare("alpha", "alice", "alpha-bot").await;
    fixture
        .adapter
        .set_enabled("local", "alpha", true)
        .await
        .unwrap();
    assert!(fixture.adapter.snapshot("alpha").unwrap().unwrap().enabled);
    let plugin = fixture.root.path().join("library/plugins/gateway");
    let declaration = fs::read(plugin.join("channel.json")).unwrap();
    let mut changed: Value = serde_json::from_slice(&declaration).unwrap();
    changed["description"] = json!("changed authorization");
    fs::write(plugin.join("channel.json"), changed.to_string()).unwrap();
    assert!(!fixture.adapter.snapshot("alpha").unwrap().unwrap().enabled);
    assert_eq!(status(&fixture, "alpha").await["available"], false);
    fs::write(plugin.join("channel.json"), declaration).unwrap();
    assert!(fixture.adapter.snapshot("alpha").unwrap().unwrap().enabled);
    fixture.adapter.shutdown().await;
}

#[tokio::test]
async fn damaged_linked_or_legacy_channel_configuration_is_not_loaded_or_overwritten() {
    let _guard = crate::PROCESS_ENV_LOCK.lock().await;
    let _environment = Environment::cleared(FIELDS);
    let fixture = Fixture::new();
    fixture.configure("alpha", "alice").await;
    fixture.configure("beta", "bob").await;
    let Fixture { root, adapter, .. } = fixture;
    drop(adapter);
    let open = || ChannelManagementAdapter::open(Fixture::configuration(root.path()));
    let graphs = root.path().join("state/platform/channels/graphs");
    let path = graphs.join("alpha.json");
    let saved = fs::read(&path).unwrap();
    // One unusable binding keeps its own Graph out of service, untouched,
    // and lets the Host and the other bots start.
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    let opened = open().unwrap();
    assert_eq!(
        opened.status("local", "alpha").await.unwrap()["damaged"],
        true
    );
    assert!(opened.snapshot("alpha").is_err());
    assert!(matches!(
        opened
            .configure("local", "alpha", Fixture::binding("carol"))
            .await,
        Err(ApplicationError::Conflict(_))
    ));
    assert!(opened.remove("alpha").await.is_err());
    assert_eq!(
        opened
            .snapshots()
            .unwrap()
            .into_iter()
            .map(|state| state.graph)
            .collect::<Vec<_>>(),
        ["beta"]
    );
    assert!(opened.snapshot("beta").unwrap().is_some());
    assert_eq!(fs::read(&path).unwrap(), saved);
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o644
    );
    drop(opened);
    fs::remove_file(&path).unwrap();
    let outside = root.path().join("outside");
    fs::write(&outside, "sentinel").unwrap();
    std::os::unix::fs::symlink(&outside, &path).unwrap();
    let opened = open().unwrap();
    assert!(opened.snapshot("alpha").is_err());
    assert!(
        opened
            .configure("local", "alpha", Fixture::binding("carol"))
            .await
            .is_err()
    );
    assert_eq!(fs::read_to_string(&outside).unwrap(), "sentinel");
    drop(opened);
    fs::remove_file(&path).unwrap();
    // Files that are not `<graph>.json` are not bindings.
    fs::write(graphs.join(".DS_Store"), "index").unwrap();
    fs::write(graphs.join("beta.json~"), "backup").unwrap();
    let opened = open().unwrap();
    assert!(opened.snapshot("alpha").unwrap().is_none());
    assert!(opened.snapshot("beta").unwrap().is_some());
    // A Graph may be named like a staging file.
    opened
        .configure("local", ".tmpgraph", Fixture::binding("dave"))
        .await
        .unwrap_err();
    drop(opened);
    let legacy = root.path().join("state/platform/channels/wecom.json");
    fs::write(&legacy, "{}").unwrap();
    assert_eq!(open().err().as_deref(), Some(LEGACY_BINDING));
}

#[tokio::test]
async fn a_restart_keeps_a_bot_stopped_whose_changed_account_is_taken() {
    let _guard = crate::PROCESS_ENV_LOCK.lock().await;
    let _environment = Environment::cleared(FIELDS);
    let fixture = Fixture::new();
    fixture.prepare("alpha", "alice", "alpha-bot").await;
    fixture.prepare("beta", "bob", "beta-bot").await;
    for graph in ["alpha", "beta"] {
        fixture
            .adapter
            .set_enabled("local", graph, true)
            .await
            .unwrap();
        fixture.wait_for(graph, "starts").await;
    }
    // alpha's Bot ID is changed outside the API to the account beta serves.
    let state_root = fixture.root.path().join("state");
    let library = fixture.root.path().join("library");
    fixture
        .sessions
        .finish_turn(
            "local",
            "pilot",
            &fixture.turn,
            anchor_platform_session::TurnStatus::Completed,
            None,
        )
        .unwrap();
    let (turn, _) = fixture
        .sessions
        .create_turn("local", "pilot", "change-bot", None)
        .unwrap();
    let request = crate::credential_store::request(
        &state_root,
        &library,
        &fixture.sessions,
        "local",
        &fixture.session,
        &turn.id,
        "gateway",
        &crate::credential_store::channel_target(PLATFORM, "alpha"),
        "WECOM_BOT_ID",
        "change the bot outside the management API",
    )
    .unwrap();
    crate::credential_store::submit(
        &state_root,
        &library,
        &fixture.sessions,
        "local",
        &fixture.session,
        &request.id,
        "beta-bot",
    )
    .unwrap();
    let state = fixture.adapter.snapshot("alpha").unwrap().unwrap();
    assert!(!state.enabled, "a changed account is not served");
    assert!(state.saved_enabled, "but the bot is still saved as enabled");
    fixture.adapter.shutdown().await;
    let Fixture { root, adapter, .. } = fixture;
    drop(adapter);
    let reopened = ChannelManagementAdapter::open(Fixture::configuration(root.path())).unwrap();
    reopened.resume().await.unwrap();
    let starts = |graph| {
        fs::read_to_string(
            super::super::state_directory(&root.path().join("state"), graph)
                .join("observed/starts"),
        )
        .unwrap_or_default()
        .len()
    };
    tokio::time::timeout(Duration::from_secs(3), async {
        while starts("beta") < 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(starts("alpha"), 1, "alpha must not start on beta's account");
    let refused = reopened.set_enabled("local", "alpha", true).await;
    assert!(
        matches!(&refused, Err(ApplicationError::Conflict(reason)) if reason.contains("beta")),
        "{:?}",
        refused.err()
    );
    assert_eq!(starts("alpha"), 1);
    reopened.shutdown().await;
}

#[tokio::test]
async fn removing_a_graph_deletes_bot_credentials_saved_without_a_binding() {
    let _guard = crate::PROCESS_ENV_LOCK.lock().await;
    let _environment = Environment::cleared(FIELDS);
    let fixture = Fixture::new();
    fixture.credential("alpha", "WECOM_BOT_ID", "alpha-bot");
    fixture.prepare("beta", "bob", "beta-bot").await;
    let credentials = fixture
        .root
        .path()
        .join("state/platform/credentials")
        .join(crate::credential_store::hash(
            crate::credential_store::binding_owner("local").as_bytes(),
        ))
        .join("gateway/channel_wecom");
    assert!(credentials.join("alpha/WECOM_BOT_ID.json").is_file());
    fixture.adapter.remove("alpha").await.unwrap();
    assert!(!credentials.join("alpha").exists());
    assert!(credentials.join("beta/WECOM_BOT_ID.json").is_file());
}

/// Design card 005: the delegation list is an operator authorization, and a
/// bot cannot delegate to itself.
#[tokio::test]
async fn delegated_graphs_are_operator_authorized_and_bind_the_whole_graph() {
    let _guard = crate::PROCESS_ENV_LOCK.lock().await;
    let _environment = Environment::cleared(FIELDS);
    let fixture = Fixture::new();
    fixture.prepare("alpha", "alice", "alpha-bot").await;

    let mut delegated = Fixture::binding("alice");
    delegated.delegate_graphs = vec!["report".into()];
    fixture
        .adapter
        .configure("local", "alpha", delegated)
        .await
        .unwrap();
    assert_eq!(
        fixture
            .adapter
            .snapshot("alpha")
            .unwrap()
            .unwrap()
            .binding
            .delegate_graphs,
        vec!["report".to_owned()]
    );

    // A bot Graph never needs to delegate to its own binding: its assistant Run
    // already carries the send tool, and allowing it would blur the two paths.
    let mut itself = Fixture::binding("alice");
    itself.delegate_graphs = vec!["alpha".into()];
    let refused = fixture.adapter.configure("local", "alpha", itself).await;
    assert!(
        matches!(&refused, Err(ApplicationError::Invalid(reason)) if reason.contains("itself")),
        "{:?}",
        refused.err()
    );
}
/// A binding written before design card 005 has no `delegate_graphs`; it must
/// keep loading, and it delegates to nobody.
#[tokio::test]
async fn a_binding_without_the_delegation_field_reads_as_empty() {
    let _guard = crate::PROCESS_ENV_LOCK.lock().await;
    let _environment = Environment::cleared(FIELDS);
    let fixture = Fixture::new();
    let directory = fixture.root.path().join("state/platform/channels/graphs");
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("alpha.json");
    let digest = "0".repeat(64);
    let callback = format!("anchor-channel-api-{}", "0".repeat(64));
    fs::write(
        &path,
        json!({"format":2,"owner":"local","plugin":"gateway","users":["alice"],
               "send_users":["alice"],"enabled":false,
               "authorization_digest":digest,"callback_key":callback})
        .to_string(),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    // The adapter holds an exclusive lease on the configuration; reopen only
    // after releasing it, as the other reopen tests do.
    let Fixture { root, adapter, .. } = fixture;
    drop(adapter);
    let reopened = ChannelManagementAdapter::open(Fixture::configuration(root.path())).unwrap();
    let binding = reopened.snapshot("alpha").unwrap().unwrap().binding;
    assert_eq!(binding.delegate_graphs, Vec::<String>::new());
    drop(reopened);

    // And it round-trips once written with a list.
    let reopened = ChannelManagementAdapter::open(Fixture::configuration(root.path())).unwrap();
    let mut delegated = Fixture::binding("alice");
    delegated.delegate_graphs = vec!["report".into()];
    reopened
        .configure("local", "alpha", delegated)
        .await
        .unwrap();
    let saved = fs::read_to_string(&path).unwrap();
    assert!(saved.contains("\"report\""), "{saved}");
    assert!(saved.contains("\"delegate_graphs\""), "{saved}");
}

/// Only an enabled bot that names this Graph is delegated to it.
#[tokio::test]
async fn delegation_requires_an_enabled_bot_that_names_the_graph() {
    let _guard = crate::PROCESS_ENV_LOCK.lock().await;
    let _environment = Environment::cleared(FIELDS);
    let fixture = Fixture::new();
    fixture.prepare("alpha", "alice", "alpha-bot").await;
    assert!(
        super::delegated_bots(&fixture.root.path().join("state"), "report")
            .unwrap()
            .is_empty()
    );
    let mut delegated = Fixture::binding("alice");
    delegated.delegate_graphs = vec!["report".into()];
    fixture
        .adapter
        .configure("local", "alpha", delegated)
        .await
        .unwrap();

    // Saved but not enabled: the gateway is not running, so there is no
    // control surface to grant.
    assert!(
        super::delegated_bots(&fixture.root.path().join("state"), "report")
            .unwrap()
            .is_empty()
    );
    // Some other Graph is not named by anyone.
    assert!(
        super::delegated_bots(&fixture.root.path().join("state"), "other")
            .unwrap()
            .is_empty()
    );
}

/// T29: a name that does not resolve to a Graph is refused at enable time, so
/// a typo surfaces while the operator is still editing the binding rather than
/// from a node that fails at send time.
#[tokio::test]
async fn enabling_refuses_a_delegated_graph_that_does_not_exist() {
    let _guard = crate::PROCESS_ENV_LOCK.lock().await;
    let _environment = Environment::cleared(FIELDS);
    let fixture = Fixture::new();
    fixture.prepare("alpha", "alice", "alpha-bot").await;
    let mut delegated = Fixture::binding("alice");
    delegated.delegate_graphs = vec!["ghost".into()];
    fixture
        .adapter
        .configure("local", "alpha", delegated)
        .await
        .unwrap();

    let refused = fixture.adapter.set_enabled("local", "alpha", true).await;
    assert!(
        matches!(&refused, Err(ApplicationError::Invalid(reason)) if reason.contains("ghost")),
        "{:?}",
        refused.err()
    );
    // The refused enable left the binding stopped and undelegated.
    let binding = fixture.adapter.snapshot("alpha").unwrap().unwrap();
    assert!(!binding.enabled);
    assert!(
        super::delegated_bots(&fixture.root.path().join("state"), "ghost")
            .unwrap()
            .is_empty()
    );
}
