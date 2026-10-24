use super::*;
use crate::application::channel_management::{ChannelManagementPort, ChannelRuntimeState};
use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use futures_util::future::BoxFuture;
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

struct Port(Mutex<Option<ChannelRuntimeState>>);
impl ChannelManagementPort for Port {
    fn status<'a>(
        &'a self,
        owner: &'a str,
        _: &'a str,
    ) -> BoxFuture<'a, Result<Value, ApplicationError>> {
        Box::pin(async move {
            Ok(match self.0.lock().unwrap().as_ref() {
                Some(state) if state.owner == owner => {
                    json!({"configured":true,"binding":state.binding,"enabled":state.enabled})
                }
                Some(_) => json!({"configured":true,"owner_matches":false}),
                None => json!({"configured":false}),
            })
        })
    }
    fn configure<'a>(
        &'a self,
        owner: &'a str,
        _: &'a str,
        binding: ChannelBinding,
    ) -> BoxFuture<'a, Result<Value, ApplicationError>> {
        Box::pin(async move {
            let mut current = self.0.lock().unwrap();
            if current.as_ref().is_some_and(|state| state.owner != owner) {
                return Err(ApplicationError::Conflict("another owner".into()));
            }
            *current = Some(ChannelRuntimeState {
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
        owner: &'a str,
        _: &'a str,
        enabled: bool,
    ) -> BoxFuture<'a, Result<Value, ApplicationError>> {
        Box::pin(async move {
            let mut current = self.0.lock().unwrap();
            let state = current.as_mut().ok_or(ApplicationError::Missing)?;
            if state.owner != owner {
                return Err(ApplicationError::Conflict("another owner".into()));
            }
            state.enabled = enabled;
            Ok(json!({"enabled":enabled}))
        })
    }
    fn snapshot(&self, _: &str) -> Result<Option<ChannelRuntimeState>, ApplicationError> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn callback_owner(&self, _: &str, _: &str) -> Option<String> {
        None
    }
}

fn fixture() -> (tempfile::TempDir, ApiState, Arc<Port>) {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("task")).unwrap();
    std::fs::create_dir_all(root.path().join("plugins/gateway")).unwrap();
    std::fs::write(
        root.path().join("plugins/gateway/plugin.json"),
        r#"{"name":"Gateway"}"#,
    )
    .unwrap();
    std::fs::write(root.path().join("task/graph.json"), r#"{"entry":"wait","agents":{"reply":{"model":"fixture","instructions":"fixture"}},"ops":{"wait":{"host":{"operation":"session.wait_input"}},"deliver":{"host":{"operation":"session.reply"}}},"nodes":[{"id":"wait","op":"wait"},{"id":"reply","agent":"reply","plugins":["gateway"]},{"id":"deliver","op":"deliver"}],"edges":[{"from":"wait","to":"reply"},{"from":"reply","to":"deliver"},{"from":"deliver","to":"wait"}]}"#).unwrap();
    std::fs::write(
        root.path().join("task/manifest.json"),
        r#"{"format":1,"graph":"graph.json","plugins":[{"id":"gateway"}]}"#,
    )
    .unwrap();
    let port = Arc::new(Port(Mutex::new(None)));
    let (model_configuration, runtime_management) =
        crate::api::tests::management_fixture(root.path());
    let state = ApiState {
        model_configuration,
        runtime_management,
        bundle_root: root.path().join("task"),
        catalog_root: root.path().into(),
        data_root: root.path().join("state"),
        workspace_root: root.path().join("workspaces"),
        graph_name: String::new(),
        application: RunApplication::new(root.path().join("state"), root.path().into())
            .with_channel_management(port.clone()),
        loopback: false,
        api_keys: vec!["a".repeat(32), "b".repeat(32)],
        schedules: super::super::schedules::ScheduleStore::open(root.path().join("schedules.json"))
            .unwrap(),
        pilots: Default::default(),
        wecom: WecomSettings {
            graph: String::new(),
            reply_node: String::new(),
            account: None,
            users: HashSet::new(),
        },
        channel_descriptors: Default::default(),
        channel_event_locks: Default::default(),
        response_fixture: None,
    };
    (root, state, port)
}
async fn call(
    state: &ApiState,
    method: &str,
    path: &str,
    key: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(key) = key {
        request = request.header(header::AUTHORIZATION, format!("Bearer {key}"));
    }
    let response = router(state.clone())
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn channel_management_http_is_authenticated_owner_bound_and_checks_expected_binding() {
    let _guard = crate::PROCESS_ENV_LOCK.lock().await;
    let (_root, state, port) = fixture();
    let key = "a".repeat(32);
    let path = "/channels/wecom/configuration";
    assert_eq!(
        call(&state, "GET", path, None, json!({})).await.0,
        StatusCode::UNAUTHORIZED
    );
    let binding = json!({"graph":"task","reply_node":"reply","plugin":"gateway","users":["alice"],"send_users":["alice"]});
    let (status, result) = call(&state, "PUT", path, Some(&key), binding.clone()).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let actual_owner = format!("api:{:x}", Sha256::digest(key.as_bytes()));
    assert_eq!(port.0.lock().unwrap().as_ref().unwrap().owner, actual_owner);
    let (_, other) = call(&state, "GET", path, Some(&"b".repeat(32)), json!({})).await;
    assert_eq!(other["owner_matches"], false);
    assert!(other.get("binding").is_none());
    let mut stale = binding.clone();
    stale["users"] = json!(["bob"]);
    let (status, _) = call(
        &state,
        "POST",
        "/channels/wecom/enabled",
        Some(&key),
        json!({"enabled":true,"expected_binding":stale}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(!port.0.lock().unwrap().as_ref().unwrap().enabled);
    let (status, result) = call(
        &state,
        "POST",
        "/channels/wecom/enabled",
        Some(&key),
        json!({"enabled":true,"expected_binding":binding}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert!(port.0.lock().unwrap().as_ref().unwrap().enabled);
}

#[tokio::test]
async fn channel_management_http_rejects_secrets_owner_forgery_and_unbound_enable() {
    let _guard = crate::PROCESS_ENV_LOCK.lock().await;
    let (_root, state, port) = fixture();
    let key = "a".repeat(32);
    let binding =
        json!({"graph":"task","reply_node":"reply","plugin":"gateway","users":[],"send_users":[]});
    for (field, value) in [
        ("owner", "forged"),
        ("WECOM_BOT_SECRET", "private"),
        ("account", "bot"),
    ] {
        let mut request = binding.clone();
        request[field] = json!(value);
        assert_eq!(
            call(
                &state,
                "PUT",
                "/channels/wecom/configuration",
                Some(&key),
                request
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    assert!(port.0.lock().unwrap().is_none());
    assert_eq!(
        call(
            &state,
            "POST",
            "/channels/wecom/enabled",
            Some(&key),
            json!({"enabled":true})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            &state,
            "GET",
            "/channels/unsupported/configuration",
            Some(&key),
            json!({})
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[tokio::test]
async fn channel_binding_requires_a_persistent_assistant_graph() {
    let _guard = crate::PROCESS_ENV_LOCK.lock().await;
    let (root, state, port) = fixture();
    let key = "a".repeat(32);
    let binding = json!({"graph":"task","reply_node":"reply","plugin":"gateway","users":["alice"],"send_users":["alice"]});
    let single = r#"{"entry":"reply","agents":{"reply":{"model":"fixture","instructions":"fixture"}},"nodes":[{"id":"reply","agent":"reply","plugins":["gateway"]}],"edges":[]}"#;
    let persistent = std::fs::read(root.path().join("task/graph.json")).unwrap();
    std::fs::write(root.path().join("task/graph.json"), single).unwrap();
    let (status, refused) = call(
        &state,
        "PUT",
        "/channels/wecom/configuration",
        Some(&key),
        binding.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("persistent assistant"),
        "{refused}"
    );
    assert!(port.0.lock().unwrap().is_none());

    // A binding saved while the Graph was an assistant cannot be enabled once
    // the Graph has lost its input loop.
    std::fs::write(root.path().join("task/graph.json"), persistent).unwrap();
    let (status, result) = call(
        &state,
        "PUT",
        "/channels/wecom/configuration",
        Some(&key),
        binding.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    std::fs::write(root.path().join("task/graph.json"), single).unwrap();
    let (status, refused) = call(
        &state,
        "POST",
        "/channels/wecom/enabled",
        Some(&key),
        json!({"enabled":true,"expected_binding":binding}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert!(!port.0.lock().unwrap().as_ref().unwrap().enabled);
}
