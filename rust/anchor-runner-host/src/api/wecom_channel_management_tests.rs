use super::*;
use crate::application::channel_management::fake::FakeChannels;
use axum::{body::Body, http::Request};
use std::sync::Arc;
use tower::ServiceExt;

const CALLBACK: &str = "anchor-channel-api-fixture";
const OTHER_CALLBACK: &str = "anchor-channel-api-other";

fn fixture() -> (tempfile::TempDir, ApiState, Arc<FakeChannels>) {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("state/platform")).unwrap();
    anchor_platform_session::SessionStore::open(root.path().join("state/platform/sessions.sqlite"))
        .unwrap();
    let schedules =
        super::super::schedules::ScheduleStore::open(root.path().join("schedules.json")).unwrap();
    let mut bot = FakeChannels::bot("bot-graph", "local", "reply", &["alice"]);
    bot.account = Some("wecom:private-account-hash".into());
    let mut other = FakeChannels::bot("other-bot", "local", "reply", &["*"]);
    other.account = Some("wecom:other-account-hash".into());
    let port = Arc::new(
        FakeChannels::new()
            .with(bot)
            .with(other)
            .with_callback("bot-graph", CALLBACK)
            .with_callback("other-bot", OTHER_CALLBACK),
    );
    let application = RunApplication::new(root.path().join("state"), root.path().join("catalog"))
        .with_channel_management(port.clone());
    let (model_configuration, runtime_management) =
        crate::api::tests::management_fixture(root.path());
    let state = ApiState {
        model_configuration,
        runtime_management,
        bundle_root: root.path().join("bundle"),
        catalog_root: root.path().join("catalog"),
        data_root: root.path().join("state"),
        workspace_root: root.path().join("workspaces"),
        graph_name: String::new(),
        application,
        owner: "local".into(),
        schedules,
        pilots: Default::default(),
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
async fn callback_key_reaches_only_its_own_graphs_callback_routes_even_on_loopback() {
    let (_root, state, port) = fixture();
    for _ in 0..2 {
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
            request(&state, "GET", "/channels/bot-graph/events", CALLBACK).await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request(&state, "POST", "/channels/bot-graph/events", CALLBACK).await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            request(
                &state,
                "GET",
                "/channels/bot-graph/progress/unknown-event",
                CALLBACK
            )
            .await,
            StatusCode::NOT_FOUND
        );
        // One bot's key does not reach another bot's callbacks, nor a
        // prefix-sharing Graph name.
        for path in ["/channels/other-bot/events", "/channels/bot-graph-2/events"] {
            assert_eq!(
                request(&state, "POST", path, CALLBACK).await,
                StatusCode::UNAUTHORIZED,
                "{path}"
            );
        }
        // An unrouted subpath is not a callback route at all.
        assert!(
            !request(&state, "POST", "/channels/bot-graph/events/extra", CALLBACK)
                .await
                .is_success()
        );
        assert_eq!(
            request(&state, "POST", "/channels/other-bot/events", OTHER_CALLBACK).await,
            StatusCode::BAD_REQUEST
        );
        let headers = HeaderMap::from_iter([(
            header::AUTHORIZATION,
            axum::http::HeaderValue::from_str(&format!("Bearer {CALLBACK}")).unwrap(),
        )]);
        assert_eq!(private_owner(&state, &headers), "local");
        let settings = current_settings(&state, "local", "bot-graph").unwrap();
        assert_eq!(settings.graph, "bot-graph");
        assert!(settings.allows("alice"));
        assert!(!settings.allows("bob"));
        assert!(current_settings(&state, "another-owner", "bot-graph").is_err());
        port.update("bot-graph", |bot| bot.enabled = false);
        assert_eq!(
            request(&state, "POST", "/channels/bot-graph/events", CALLBACK).await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request(
                &state,
                "GET",
                "/channels/bot-graph/progress/unknown-event",
                CALLBACK
            )
            .await,
            StatusCode::UNAUTHORIZED
        );
        // The other bot keeps working while this one is stopped.
        assert_eq!(
            request(&state, "POST", "/channels/other-bot/events", OTHER_CALLBACK).await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            request(&state, "GET", "/graphs", "anchor-channel-api-stale").await,
            StatusCode::UNAUTHORIZED
        );
        port.update("bot-graph", |bot| bot.enabled = true);
    }
}

/// The Web/API needs no key; a callback key stays confined to its own routes.
#[tokio::test]
async fn the_web_api_needs_no_key() {
    let (_root, state, _port) = fixture();
    for path in ["/graphs", "/sessions"] {
        assert_eq!(
            request(&state, "GET", path, "").await,
            StatusCode::OK,
            "{path}"
        );
    }
}
