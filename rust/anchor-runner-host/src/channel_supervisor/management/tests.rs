use super::*;
use anchor_platform_session::{CreateSession, Session, SessionStore};
use std::{ffi::OsString, sync::Arc, time::Duration};

const FIELDS: &[&str] = &[
    "WECOM_BOT_ID",
    "WECOM_BOT_SECRET",
    "ANCHOR_WECOM_GRAPH",
    "ANCHOR_WECOM_REPLY_NODE",
    "ANCHOR_WECOM_USERS",
    "ANCHOR_WECOM_SEND_USERS",
];

struct Environment(Vec<(&'static str, Option<OsString>)>);
impl Environment {
    fn clear() -> Self {
        let fields = FIELDS
            .iter()
            .map(|name| (*name, env::var_os(name)))
            .collect();
        for field in FIELDS {
            unsafe { env::remove_var(field) };
        }
        Self(fields)
    }
}
impl Drop for Environment {
    fn drop(&mut self) {
        for (field, value) in &self.0 {
            unsafe {
                match value {
                    Some(value) => env::set_var(field, value),
                    None => env::remove_var(field),
                };
            }
        }
    }
}

struct Fixture {
    root: tempfile::TempDir,
    adapter: Arc<ChannelManagementAdapter>,
    sessions: SessionStore,
    session: Session,
    turn: String,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let plugin = root.path().join("library/plugins/gateway");
        fs::create_dir_all(plugin.join("bin")).unwrap();
        fs::write(
            plugin.join("plugin.json"),
            json!({"name":"Fixture gateway","mutable":["observed/"]}).to_string(),
        )
        .unwrap();
        fs::write(plugin.join("channel.json"), json!({"platform":"wecom","transport":"websocket","entrypoint":"bin/gateway","required_environment":["WECOM_BOT_ID","WECOM_BOT_SECRET"]}).to_string()).unwrap();
        fs::write(
            plugin.join("bin/gateway"),
            r#"#!/bin/sh
mkdir -p observed
printf '%s' "$WECOM_BOT_ID" > observed/account
printf '%s' "$WECOM_BOT_SECRET" > observed/secret
printf '%s' "$ANCHOR_API_KEY" > observed/callback
printf '%s' "$ANCHOR_WECOM_USERS" > observed/users
printf '%s' "$ANCHOR_WECOM_SEND_USERS" > observed/send-users
printf 'x' >> observed/starts
trap 'printf stopped > observed/stopped; exit 0' TERM
while :; do sleep 0.02; done
"#,
        )
        .unwrap();
        fs::set_permissions(
            plugin.join("bin/gateway"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        let graph = root.path().join("catalog/task");
        fs::create_dir_all(&graph).unwrap();
        fs::write(graph.join("graph.json"),json!({"entry":"reply","agents":{"reply":{"model":"fixture","instructions":"fixture"}},"nodes":[{"id":"reply","agent":"reply","plugins":["gateway"]}],"edges":[]}).to_string()).unwrap();
        fs::write(
            graph.join("manifest.json"),
            json!({"format":1,"graph":"graph.json","plugins":[{"id":"gateway"}]}).to_string(),
        )
        .unwrap();
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
        let adapter = Arc::new(
            ChannelManagementAdapter::open(Self::configuration(root.path()), None).unwrap(),
        );
        Self {
            root,
            adapter,
            sessions,
            session,
            turn: turn.id,
        }
    }

    fn configuration(root: &Path) -> ChannelHostConfiguration {
        ChannelHostConfiguration::from_env(
            root.join("state"),
            root.join("catalog"),
            root.join("library"),
            "127.0.0.1:8077".into(),
            "local".into(),
        )
    }

    fn binding() -> ChannelBinding {
        ChannelBinding {
            graph: "task".into(),
            reply_node: "reply".into(),
            plugin: "gateway".into(),
            users: vec!["alice".into()],
            send_users: vec!["alice".into()],
        }
    }

    async fn prepare(&self) {
        self.adapter
            .configure("local", "wecom", Self::binding())
            .await
            .unwrap();
        for (field, value) in [
            ("WECOM_BOT_ID", "private-fixture-bot"),
            ("WECOM_BOT_SECRET", "private-fixture-secret"),
        ] {
            let request = crate::credential_store::request(
                &self.root.path().join("state"),
                &self.root.path().join("library"),
                &self.sessions,
                "local",
                &self.session,
                &self.turn,
                "gateway",
                "channel_wecom",
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
    }

    fn observed(&self, name: &str) -> PathBuf {
        self.root
            .path()
            .join("library/plugins/gateway/observed")
            .join(name)
    }

    async fn wait_for(&self, name: &str) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while !self.observed(name).is_file() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn managed_channel_uses_private_input_and_shared_binding_without_network_or_mount_grants() {
    let _guard = crate::PROCESS_ENV_LOCK.lock().await;
    let _environment = Environment::clear();
    let fixture = Fixture::new();
    let before = fixture.sessions.get("local", "pilot").unwrap();
    fixture.prepare().await;
    let status = fixture.adapter.status("local", "wecom").await.unwrap();
    assert_eq!(status["enabled"], false);
    assert_eq!(status["credentials"].as_array().unwrap().len(), 2);
    assert!(
        status["credentials"]
            .as_array()
            .unwrap()
            .iter()
            .all(|field| field["configured"] == true)
    );
    assert!(!status.to_string().contains("private-fixture"));
    let enabled = fixture
        .adapter
        .set_enabled("local", "wecom", true)
        .await
        .unwrap();
    assert_eq!(enabled["enabled"], true);
    fixture.wait_for("starts").await;
    assert_eq!(
        fs::read_to_string(fixture.observed("account")).unwrap(),
        "private-fixture-bot"
    );
    assert_eq!(
        fs::read_to_string(fixture.observed("secret")).unwrap(),
        "private-fixture-secret"
    );
    assert_eq!(
        fs::read_to_string(fixture.observed("users")).unwrap(),
        "alice"
    );
    assert_eq!(
        fs::read_to_string(fixture.observed("send-users")).unwrap(),
        "alice"
    );
    let callback = fs::read_to_string(fixture.observed("callback")).unwrap();
    assert_eq!(
        fixture
            .adapter
            .callback_owner("wecom", &callback)
            .as_deref(),
        Some("local")
    );
    assert!(fixture.adapter.callback_owner("wecom", "wrong").is_none());
    assert!(
        fixture
            .adapter
            .callback_owner("different-platform", &callback)
            .is_none()
    );
    let snapshot = fixture.adapter.snapshot("wecom").unwrap().unwrap();
    assert_eq!(snapshot.binding, Fixture::binding());
    assert!(snapshot.account.unwrap().starts_with("wecom:"));
    let after = fixture.sessions.get("local", "pilot").unwrap();
    assert_eq!(before.network_allowed, after.network_allowed);
    assert_eq!(before.plugin_mounts, after.plugin_mounts);
    assert!(after.run_ids.is_empty());
    fixture
        .adapter
        .set_enabled("local", "wecom", false)
        .await
        .unwrap();
    fixture.wait_for("stopped").await;
    assert!(fixture.adapter.callback_owner("wecom", &callback).is_none());
    assert!(
        fixture
            .adapter
            .snapshot("wecom")
            .unwrap()
            .unwrap()
            .descriptor
            .is_none()
    );
}

#[tokio::test]
async fn channel_reloads_one_durable_binding_and_rejects_another_owner_or_live_rebinding() {
    let _guard = crate::PROCESS_ENV_LOCK.lock().await;
    let _environment = Environment::clear();
    let fixture = Fixture::new();
    fixture.prepare().await;
    fixture
        .adapter
        .set_enabled("local", "wecom", true)
        .await
        .unwrap();
    fixture.wait_for("starts").await;
    let callback = fs::read_to_string(fixture.observed("callback")).unwrap();
    assert!(matches!(
        fixture
            .adapter
            .configure("local", "wecom", Fixture::binding())
            .await,
        Err(ApplicationError::Conflict(_))
    ));
    assert!(matches!(
        fixture.adapter.set_enabled("other", "wecom", false).await,
        Err(ApplicationError::Conflict(_))
    ));
    assert_eq!(
        fixture
            .adapter
            .status("other", "wecom")
            .await
            .unwrap()
            .get("binding"),
        None
    );
    assert_eq!(
        fixture
            .adapter
            .status("other", "wecom")
            .await
            .unwrap()
            .get("credentials"),
        None
    );
    fixture.adapter.shutdown().await;
    let Fixture {
        root,
        adapter,
        sessions: _,
        session: _,
        turn: _,
    } = fixture;
    drop(adapter);
    let reopened =
        ChannelManagementAdapter::open(Fixture::configuration(root.path()), None).unwrap();
    assert!(reopened.snapshot("wecom").unwrap().unwrap().enabled);
    assert_eq!(
        reopened.callback_owner("wecom", &callback).as_deref(),
        Some("local")
    );
    reopened.resume().await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while fs::read_to_string(root.path().join("library/plugins/gateway/observed/starts"))
            .unwrap_or_default()
            .len()
            < 2
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    reopened.set_enabled("local", "wecom", false).await.unwrap();
    assert_eq!(
        fs::metadata(root.path().join("state/platform/channels/wecom.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[tokio::test]
async fn concurrent_enables_start_one_gateway_and_disabling_waits_for_shutdown() {
    let _guard = crate::PROCESS_ENV_LOCK.lock().await;
    let _environment = Environment::clear();
    let fixture = Fixture::new();
    fixture.prepare().await;
    let (first, second) = tokio::join!(
        fixture.adapter.set_enabled("local", "wecom", true),
        fixture.adapter.set_enabled("local", "wecom", true)
    );
    first.unwrap();
    second.unwrap();
    fixture.wait_for("starts").await;
    let previous_key = fs::read_to_string(fixture.observed("callback")).unwrap();
    assert_eq!(fs::read_to_string(fixture.observed("starts")).unwrap(), "x");
    let (disabled, enabled) = tokio::join!(
        fixture.adapter.set_enabled("local", "wecom", false),
        fixture.adapter.set_enabled("local", "wecom", true)
    );
    disabled.unwrap();
    enabled.unwrap();
    assert!(
        fixture
            .adapter
            .callback_owner("wecom", &previous_key)
            .is_none()
    );
    fixture
        .adapter
        .set_enabled("local", "wecom", false)
        .await
        .unwrap();
    assert_eq!(
        fixture.adapter.status("local", "wecom").await.unwrap()["listener"],
        "stopped"
    );
}

#[tokio::test]
async fn removing_graph_or_plugin_stops_the_gateway_and_preserves_inspectable_intent() {
    let _guard = crate::PROCESS_ENV_LOCK.lock().await;
    let _environment = Environment::clear();
    for remove_graph in [true, false] {
        let fixture = Fixture::new();
        fixture.prepare().await;
        fixture
            .adapter
            .set_enabled("local", "wecom", true)
            .await
            .unwrap();
        fixture.wait_for("starts").await;
        if remove_graph {
            fs::remove_dir_all(fixture.root.path().join("catalog/task")).unwrap();
        } else {
            fs::remove_file(
                fixture
                    .root
                    .path()
                    .join("library/plugins/gateway/plugin.json"),
            )
            .unwrap();
        }
        fixture.wait_for("stopped").await;
        let status = fixture.adapter.status("local", "wecom").await.unwrap();
        assert_eq!(status["enabled"], true);
        assert_eq!(status["available"], false);
        assert_eq!(status["listener"], "unavailable");
        assert!(
            fixture
                .adapter
                .set_enabled("local", "wecom", true)
                .await
                .is_err()
        );
        fixture
            .adapter
            .set_enabled("local", "wecom", false)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn deployment_binding_is_read_only_and_missing_credentials_do_not_enable() {
    let _guard = crate::PROCESS_ENV_LOCK.lock().await;
    let _environment = Environment::clear();
    let fixture = Fixture::new();
    fixture
        .adapter
        .configure("local", "wecom", Fixture::binding())
        .await
        .unwrap();
    assert!(matches!(
        fixture.adapter.set_enabled("local", "wecom", true).await,
        Err(ApplicationError::Invalid(_))
    ));
    assert!(!fixture.adapter.snapshot("wecom").unwrap().unwrap().enabled);
    let mut configuration = Fixture::configuration(fixture.root.path());
    configuration.deployment_binding = Some(Fixture::binding());
    let Fixture {
        root,
        adapter,
        sessions: _,
        session: _,
        turn: _,
    } = fixture;
    drop(adapter);
    let external = ChannelManagementAdapter::open(configuration, None).unwrap();
    assert_eq!(
        external.status("local", "wecom").await.unwrap()["configuration_owner"],
        "deployment"
    );
    assert!(matches!(
        external
            .configure("local", "wecom", Fixture::binding())
            .await,
        Err(ApplicationError::Conflict(_))
    ));
    assert!(matches!(
        external.set_enabled("local", "wecom", true).await,
        Err(ApplicationError::Conflict(_))
    ));
    assert_eq!(
        external.snapshot("wecom").unwrap().unwrap().binding.graph,
        "task"
    );
    assert!(
        root.path()
            .join("state/platform/channels/wecom.json")
            .is_file()
    );
}

#[tokio::test]
async fn damaged_or_linked_channel_configuration_is_not_loaded_or_overwritten() {
    let _guard = crate::PROCESS_ENV_LOCK.lock().await;
    let _environment = Environment::clear();
    let fixture = Fixture::new();
    fixture
        .adapter
        .configure("local", "wecom", Fixture::binding())
        .await
        .unwrap();
    let Fixture {
        root,
        adapter,
        sessions: _,
        session: _,
        turn: _,
    } = fixture;
    assert!(ChannelManagementAdapter::open(Fixture::configuration(root.path()), None).is_err());
    drop(adapter);
    let path = root.path().join("state/platform/channels/wecom.json");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(ChannelManagementAdapter::open(Fixture::configuration(root.path()), None).is_err());
    fs::remove_file(&path).unwrap();
    let outside = root.path().join("outside");
    fs::write(&outside, "sentinel").unwrap();
    std::os::unix::fs::symlink(&outside, &path).unwrap();
    assert!(ChannelManagementAdapter::open(Fixture::configuration(root.path()), None).is_err());
    assert_eq!(fs::read_to_string(outside).unwrap(), "sentinel");
}
