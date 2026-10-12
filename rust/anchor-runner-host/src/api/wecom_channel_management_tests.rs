use super::*;
use crate::application::channel_management::{
    ChannelBinding, ChannelManagementPort, ChannelRuntimeState,
};
use axum::{body::Body, http::Request};
use futures_util::future::BoxFuture;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

const CALLBACK: &str = "anchor-channel-api-fixture";
const ADMIN: &str = "administrative-api-key-long-enough-for-fixture";

struct Port(Mutex<ChannelRuntimeState>);
impl ChannelManagementPort for Port {
    fn status<'a>(
        &'a self,
        _owner: &'a str,
        _platform: &'a str,
    ) -> BoxFuture<'a, Result<Value, ApplicationError>> {
        Box::pin(async { Ok(json!({})) })
    }
    fn configure<'a>(
        &'a self,
        _owner: &'a str,
        _platform: &'a str,
        _binding: ChannelBinding,
    ) -> BoxFuture<'a, Result<Value, ApplicationError>> {
        Box::pin(async { Err(ApplicationError::Missing) })
    }
    fn set_enabled<'a>(
        &'a self,
        _owner: &'a str,
        _platform: &'a str,
        _enabled: bool,
    ) -> BoxFuture<'a, Result<Value, ApplicationError>> {
        Box::pin(async { Err(ApplicationError::Missing) })
    }
    fn snapshot(&self, _platform: &str) -> Result<Option<ChannelRuntimeState>, ApplicationError> {
        Ok(Some(self.0.lock().unwrap().clone()))
    }
    fn callback_owner(&self, platform: &str, key: &str) -> Option<String> {
        let current = self.0.lock().unwrap();
        (current.enabled && platform == "wecom" && key == CALLBACK).then(|| current.owner.clone())
    }
}

fn fixture() -> (tempfile::TempDir, ApiState, Arc<Port>) {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("state/platform")).unwrap();
    anchor_platform_session::SessionStore::open(root.path().join("state/platform/sessions.sqlite"))
        .unwrap();
    let schedules =
        super::super::schedules::ScheduleStore::open(root.path().join("schedules.json")).unwrap();
    let port = Arc::new(Port(Mutex::new(ChannelRuntimeState {
        owner: "local".into(),
        binding: ChannelBinding {
            graph: "requested-channel-graph".into(),
            reply_node: "reply".into(),
            plugin: "gateway".into(),
            users: vec!["alice".into()],
            send_users: vec!["alice".into()],
        },
        enabled: true,
        account: Some("wecom:private-account-hash".into()),
        descriptor: None,
    })));
    let application = RunApplication::new(root.path().join("state"), root.path().join("catalog"))
        .with_channel_management(port.clone());
    let state = ApiState {
        bundle_root: root.path().join("bundle"),
        catalog_root: root.path().join("catalog"),
        data_root: root.path().join("state"),
        workspace_root: root.path().join("workspaces"),
        graph_name: String::new(),
        application,
        loopback: true,
        api_keys: vec![ADMIN.into()],
        schedules,
        pilots: Default::default(),
        wecom: WecomSettings {
            graph: "stale-deployment-graph".into(),
            reply_node: "stale".into(),
            account: None,
            users: HashSet::new(),
        },
        channel_descriptors: Default::default(),
        channel_event_locks: Default::default(),
        response_fixture: None,
    };
    (root, state, port)
}

async fn request(state: &ApiState, method: &str, path: &str, key: &str) -> StatusCode {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {key}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{}"))
        .unwrap();
    super::super::router(state.clone())
        .oneshot(request)
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn callback_key_is_owner_bound_and_cannot_authorize_management_even_on_loopback() {
    let (_root, mut state, port) = fixture();
    for unrestricted_loopback in [false, true] {
        if unrestricted_loopback {
            state.api_keys.clear();
        }
        for path in ["/graphs", "/sessions", "/schedules", "/plugins"] {
            assert_eq!(
                request(&state, "GET", path, CALLBACK).await,
                StatusCode::UNAUTHORIZED
            );
        }
        assert_eq!(
            request(&state, "POST", "/schedules", CALLBACK).await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request(&state, "GET", "/channels/wecom/events", CALLBACK).await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request(&state, "POST", "/channels/wecom/events", CALLBACK).await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            request(
                &state,
                "GET",
                "/channels/wecom/progress/unknown-event",
                CALLBACK
            )
            .await,
            StatusCode::NOT_FOUND
        );
        let headers = HeaderMap::from_iter([(
            header::AUTHORIZATION,
            axum::http::HeaderValue::from_str(&format!("Bearer {CALLBACK}")).unwrap(),
        )]);
        assert_eq!(private_owner(&state, &headers), "local");
        let settings = current_settings(&state, "local").unwrap();
        assert_eq!(settings.graph, "requested-channel-graph");
        assert!(settings.allows("alice"));
        assert!(!settings.allows("bob"));
        assert!(current_settings(&state, "another-owner").is_err());
        port.0.lock().unwrap().enabled = false;
        assert_eq!(
            request(&state, "POST", "/channels/wecom/events", CALLBACK).await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request(
                &state,
                "GET",
                "/channels/wecom/progress/unknown-event",
                CALLBACK
            )
            .await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request(&state, "GET", "/graphs", "anchor-channel-api-stale").await,
            StatusCode::UNAUTHORIZED
        );
        port.0.lock().unwrap().enabled = true;
    }
}

#[tokio::test]
async fn explicitly_configured_api_keys_keep_existing_admin_authority() {
    let (_root, state, _port) = fixture();
    assert_eq!(
        request(&state, "GET", "/graphs", ADMIN).await,
        StatusCode::OK
    );
    assert_eq!(
        request(&state, "GET", "/sessions", ADMIN).await,
        StatusCode::OK
    );
    assert_eq!(
        request(&state, "GET", "/graphs", "wrong-api-key").await,
        StatusCode::UNAUTHORIZED
    );
}
