use super::*;
use crate::application::oauth_management::PilotOAuthOrigin;
use anchor_platform_session::{CreateSession, TurnStatus};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[tokio::test]
async fn pilot_oauth_uses_host_callback_and_cancelled_session_never_becomes_http_authorization() {
    let _env = PROCESS_ENV.lock().await;
    let names = ["ANCHOR_RUNNER_LIBRARY_ROOT", "ANCHOR_OAUTH_REDIRECT_URI"];
    let previous = names
        .iter()
        .map(|name| (*name, std::env::var_os(name)))
        .collect::<Vec<_>>();
    struct Restore(Vec<(&'static str, Option<std::ffi::OsString>)>);
    impl Drop for Restore {
        fn drop(&mut self) {
            for (name, value) in self.0.drain(..) {
                unsafe {
                    match value {
                        Some(value) => std::env::set_var(name, value),
                        None => std::env::remove_var(name),
                    }
                }
            }
        }
    }
    let _restore = Restore(previous);
    for name in names {
        unsafe { std::env::remove_var(name) }
    }
    let (_root, mut state) = fixture();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let exchanges = Arc::new(AtomicUsize::new(0));
    let exchange_count = exchanges.clone();
    let provider=Router::new().route("/.well-known/oauth-protected-resource/mcp",get({let base=base.clone();move||{let base=base.clone();async move {Json(json!({"resource":format!("{base}/mcp"),"authorization_servers":[base]}))}}}))
        .route("/.well-known/oauth-authorization-server",get({let base=base.clone();move||{let base=base.clone();async move{Json(json!({"issuer":base,"authorization_endpoint":format!("{base}/authorize"),"token_endpoint":format!("{base}/token"),"registration_endpoint":format!("{base}/register")}))}}}))
        .route("/register",post(||async{Json(json!({"client_id":"pilot-client"}))}))
        .route("/token",post(move||{let count=exchange_count.clone();async move {count.fetch_add(1,Ordering::AcqRel);Json(json!({"access_token":"pilot-oauth-private-access","token_type":"Bearer","expires_in":3600}))}}));
    let provider_task = tokio::spawn(async move { axum::serve(listener, provider).await.unwrap() });
    let plugin = state.catalog_root.join("plugins/browser");
    std::fs::create_dir_all(&plugin).unwrap();
    std::fs::write(plugin.join("plugin.json"),json!({"name":"Browser OAuth fixture","mcpServers":{"remote":{"url":format!("{base}/mcp"),"auth":"oauth","oauth_resource":format!("{base}/mcp")}}}).to_string()).unwrap();
    state.application = state.application.clone().with_oauth_management(Arc::new(
        crate::api::oauth::OAuthManagementAdapter::new(
            state.data_root.clone(),
            state.catalog_root.clone(),
            "127.0.0.1:43127",
        ),
    ));
    let sessions = store(&state).unwrap();
    let create = |id: &str| {
        let session = sessions
            .create(
                "local",
                CreateSession {
                    id: Some(id.into()),
                    ..Default::default()
                },
            )
            .unwrap();
        let (turn, _) = sessions
            .create_turn("local", id, "oauth", Some("configure browser"))
            .unwrap();
        PilotOAuthOrigin {
            sessions: sessions.clone(),
            owner: "local".into(),
            session,
            turn: turn.id,
        }
    };
    let begin = |origin: PilotOAuthOrigin| {
        let application = state.application.clone();
        async move {
            application
                .authorize_pilot_oauth(
                    &origin,
                    "browser",
                    "remote",
                    &crate::dependency_provision::DependencyAccess::default(),
                )
                .await
                .unwrap()
        }
    };
    let callback_url = |result: &Value| {
        let url = url::Url::parse(result["authorization_url"].as_str().unwrap()).unwrap();
        assert_eq!(
            url.query_pairs()
                .find(|(name, _)| name == "redirect_uri")
                .unwrap()
                .1,
            "http://127.0.0.1:43127/oauth/callback"
        );
        let transaction = url
            .query_pairs()
            .find(|(name, _)| name == "state")
            .unwrap()
            .1
            .into_owned();
        format!("/oauth/callback?state={transaction}&code=pilot-code")
    };
    let stopped = create("pilot-oauth-stopped");
    let result = begin(stopped.clone()).await;
    sessions
        .finish_turn(
            "local",
            &stopped.session.id,
            &stopped.turn,
            TurnStatus::Stopped,
            None,
        )
        .unwrap();
    let response = router(state.clone())
        .oneshot(
            Request::builder()
                .uri(callback_url(&result))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(exchanges.load(Ordering::Acquire), 0);
    let completed = create("pilot-oauth-completed-stop");
    let result = begin(completed.clone()).await;
    sessions
        .finish_turn(
            "local",
            &completed.session.id,
            &completed.turn,
            TurnStatus::Completed,
            None,
        )
        .unwrap();
    assert!(
        sessions
            .events("local", &completed.session.id, 0)
            .unwrap()
            .iter()
            .all(|event| event.kind != "pilot.continuation.requested")
    );
    for stopping in [true, false] {
        let (status, stopped) = call(
            router(state.clone()),
            "POST",
            &format!("/sessions/{}/stop", completed.session.id),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{stopped}");
        assert_eq!(stopped["stopping"], stopping);
    }
    let mut restarted = state.clone();
    restarted.pilots = crate::pilot_host::PilotService::default();
    let response = router(restarted)
        .oneshot(
            Request::builder()
                .uri(callback_url(&result))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        exchanges.load(Ordering::Acquire),
        0,
        "a callback after stop must not exchange a token"
    );
    assert_eq!(
        sessions
            .list_turns("local", &completed.session.id)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        sessions
            .get_turn("local", &completed.session.id, &completed.turn)
            .unwrap()
            .status,
        TurnStatus::Completed
    );
    assert_eq!(
        sessions
            .events("local", &completed.session.id, 0)
            .unwrap()
            .iter()
            .filter(|event| event.kind == "pilot.continuation.cancelled")
            .count(),
        1
    );
    let deleted = create("pilot-oauth-deleted");
    let result = begin(deleted.clone()).await;
    sessions
        .finish_turn(
            "local",
            &deleted.session.id,
            &deleted.turn,
            TurnStatus::Completed,
            None,
        )
        .unwrap();
    state
        .pilots
        .delete_session(
            sessions.clone(),
            state.data_root.clone(),
            state.workspace_root.clone(),
            state.catalog_root.clone(),
            "local".into(),
            deleted.session.clone(),
        )
        .await
        .unwrap();
    let response = router(state.clone())
        .oneshot(
            Request::builder()
                .uri(callback_url(&result))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(exchanges.load(Ordering::Acquire), 0);
    assert_eq!(
        state
            .application
            .oauth_status("local", "browser", "remote")
            .unwrap()["authorized"],
        false
    );
    let successful = create("pilot-oauth-successful");
    let result = begin(successful.clone()).await;
    let response = router(state.clone())
        .oneshot(
            Request::builder()
                .uri(callback_url(&result))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(exchanges.load(Ordering::Acquire), 1);
    assert_eq!(
        crate::pilot_host::continuation_status(
            &sessions,
            "local",
            &successful.session.id,
            &successful.turn
        )
        .unwrap()["status"],
        "scheduled"
    );
    let status = state
        .application
        .oauth_status("local", "browser", "remote")
        .unwrap();
    assert_eq!(status["authorized"], true);
    assert!(!status.to_string().contains("pilot-oauth-private-access"));
    assert!(
        !serde_json::to_string(&sessions.events("local", &successful.session.id, 0).unwrap())
            .unwrap()
            .contains("pilot-oauth-private-access")
    );
    assert_eq!(
        state
            .application
            .oauth_status("other-owner", "browser", "remote")
            .unwrap()["authorized"],
        false
    );
    provider_task.abort();
}
