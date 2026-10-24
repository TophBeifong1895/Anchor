use super::*;
use crate::application::{
    ApplicationError,
    channel_management::{ChannelManagementPort, ChannelRuntimeState},
};
use anchor_platform_session::{CreateSession, TurnStatus};
use futures_util::future::BoxFuture;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Port(Mutex<Option<ChannelRuntimeState>>);
impl ChannelManagementPort for Port {
    fn status<'a>(
        &'a self,
        _owner: &'a str,
        _platform: &'a str,
    ) -> BoxFuture<'a, Result<Value, ApplicationError>> {
        Box::pin(async { Ok(json!({"configured":self.0.lock().unwrap().is_some()})) })
    }
    fn configure<'a>(
        &'a self,
        owner: &'a str,
        _platform: &'a str,
        binding: ChannelBinding,
    ) -> BoxFuture<'a, Result<Value, ApplicationError>> {
        Box::pin(async move {
            *self.0.lock().unwrap() = Some(ChannelRuntimeState {
                owner: owner.into(),
                binding,
                enabled: false,
                account: None,
                descriptor: None,
            });
            Ok(json!({"configured":true}))
        })
    }
    fn set_enabled<'a>(
        &'a self,
        _owner: &'a str,
        _platform: &'a str,
        enabled: bool,
    ) -> BoxFuture<'a, Result<Value, ApplicationError>> {
        Box::pin(async move {
            self.0
                .lock()
                .unwrap()
                .as_mut()
                .ok_or(ApplicationError::Missing)?
                .enabled = enabled;
            Ok(json!({"enabled":enabled}))
        })
    }
    fn snapshot(&self, _platform: &str) -> Result<Option<ChannelRuntimeState>, ApplicationError> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn callback_owner(&self, _platform: &str, _key: &str) -> Option<String> {
        None
    }
}

fn fixture() -> (tempfile::TempDir, PilotTools, Arc<Port>) {
    let root = tempfile::tempdir().unwrap();
    let graph = root.path().join("catalog/task");
    std::fs::create_dir_all(&graph).unwrap();
    let plugin = root.path().join("catalog/plugins/gateway");
    std::fs::create_dir_all(&plugin).unwrap();
    std::fs::write(
        plugin.join("plugin.json"),
        json!({"name":"Gateway fixture"}).to_string(),
    )
    .unwrap();
    std::fs::write(graph.join("graph.json"),json!({"entry":"wait","agents":{"reply":{"model":"fixture","instructions":"fixture"}},"ops":{"wait":{"host":{"operation":"session.wait_input"}},"deliver":{"host":{"operation":"session.reply"}}},"nodes":[{"id":"wait","op":"wait"},{"id":"reply","agent":"reply","plugins":["gateway"]},{"id":"deliver","op":"deliver"}],"edges":[{"from":"wait","to":"reply"},{"from":"reply","to":"deliver"},{"from":"deliver","to":"wait"}]}).to_string()).unwrap();
    std::fs::write(
        graph.join("manifest.json"),
        json!({"format":1,"graph":"graph.json","plugins":[{"id":"gateway"}]}).to_string(),
    )
    .unwrap();
    std::fs::create_dir_all(root.path().join("state/platform")).unwrap();
    let sessions = anchor_platform_session::SessionStore::open(
        root.path().join("state/platform/sessions.sqlite"),
    )
    .unwrap();
    sessions
        .create(
            "local",
            CreateSession {
                id: Some("pilot".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let port = Arc::new(Port::default());
    let tools = PilotTools {
        application: crate::application::RunApplication::new(
            root.path().join("state"),
            root.path().join("catalog"),
        )
        .with_channel_management(port.clone()),
        graph_name: String::new(),
        library_root: root.path().join("catalog"),
        data_root: root.path().join("state"),
        workspace_root: root.path().join("workspaces"),
        sessions,
        owner: "local".into(),
        session: "pilot".into(),
        turn: String::new(),
        context: None,
    };
    (root, tools, port)
}

fn arguments() -> Value {
    json!({"platform":"wecom","graph":"task","reply_node":"reply","plugin":"gateway","users":["alice"],"send_users":["alice"]})
}

#[tokio::test]
async fn channel_mutations_require_an_owned_active_turn_and_leave_session_execution_grants_unchanged()
 {
    let (_root, mut tools, port) = fixture();
    let (platform, binding) = configuration_arguments(&arguments()).unwrap();
    assert!(
        tools
            .configure_channel_approved(&platform, binding.clone())
            .await
            .is_err()
    );
    let (turn, _) = tools
        .sessions
        .create_turn("local", "pilot", "initialize", None)
        .unwrap();
    tools = tools.for_turn(&turn.id);
    tools.owner = "other".into();
    assert!(
        tools
            .configure_channel_approved(&platform, binding.clone())
            .await
            .is_err()
    );
    tools.owner = "local".into();
    let before = tools.sessions.get("local", "pilot").unwrap();
    tools
        .configure_channel_approved(&platform, binding.clone())
        .await
        .unwrap();
    tools
        .enable_channel_approved(&platform, true, &binding)
        .await
        .unwrap();
    assert!(port.0.lock().unwrap().as_ref().unwrap().enabled);
    let after = tools.sessions.get("local", "pilot").unwrap();
    assert_eq!(before.network_allowed, after.network_allowed);
    assert_eq!(before.plugin_mounts, after.plugin_mounts);
    assert!(tools.application.records().unwrap().is_empty());
    tools
        .sessions
        .finish_turn("local", "pilot", &turn.id, TurnStatus::Completed, None)
        .unwrap();
    assert!(
        tools
            .enable_channel_approved(&platform, false, &binding)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn malformed_bindings_unknown_graphs_and_model_credentials_or_approval_are_rejected() {
    let (_root, tools, port) = fixture();
    for field in [
        "secret",
        "WECOM_BOT_SECRET",
        "credential",
        "approval",
        "confirm",
        "network",
        "owner",
    ] {
        let mut request = arguments();
        request[field] = json!("not-a-model-parameter");
        assert!(configuration_arguments(&request).is_err());
        assert!(enable_arguments(&json!({"platform":"wecom","enabled":true,field:true})).is_err());
    }
    let mut request = arguments();
    request["send_users"] = json!([]);
    assert!(configuration_arguments(&request).is_err());
    let (platform, mut binding) = configuration_arguments(&arguments()).unwrap();
    for (graph, node, plugin) in [
        ("missing", "reply", "gateway"),
        ("task", "missing", "gateway"),
        ("task", "reply", "missing"),
    ] {
        binding.graph = graph.into();
        binding.reply_node = node.into();
        binding.plugin = plugin.into();
        assert!(
            tools
                .application
                .configure_channel("local", &platform, binding.clone())
                .await
                .is_err()
        );
    }
    assert!(port.0.lock().unwrap().is_none());
    assert!(
        tools
            .application
            .set_channel_enabled("local", "wecom", true)
            .await
            .is_err()
    );
}
