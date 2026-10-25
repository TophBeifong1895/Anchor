use super::*;
use anchor_platform_session::CreateSession;

async fn private_call(
    app: Router,
    method: &str,
    uri: &str,
    key: &str,
    body: &str,
) -> (StatusCode, String) {
    let response = app
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {key}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(response.headers()["referrer-policy"], "no-referrer");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(!text.contains("private-credential-value-for-http-fixture"));
    (status, text)
}

#[tokio::test]
async fn pilot_credential_http_is_private_and_accepts_only_value() {
    let _env = PROCESS_ENV.lock().await;
    let (_root, state) = fixture();
    let keys = ["a".repeat(32)];
    let headers = HeaderMap::from_iter([(
        header::AUTHORIZATION,
        format!("Bearer {}", keys[0]).parse().unwrap(),
    )]);
    let owner = private_owner(&state, &headers);
    let sessions = store(&state).unwrap();
    let session = sessions
        .create(
            &owner,
            CreateSession {
                id: Some("responses-private-credentials".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let (turn, _) = sessions
        .create_turn(&owner, &session.id, "credential-request", Some("configure"))
        .unwrap();
    let library = state.application.library_root().to_path_buf();
    let plugin = library.join("plugins/private-http-fixture");
    std::fs::create_dir_all(&plugin).unwrap();
    std::fs::write(
        plugin.join("plugin.json"),
        json!({"name":"HTTP credential fixture","mcpServers":{"api":{"url":"https://example.com/mcp","bearer_token_env_var":"ANCHOR_TEST_PRIVATE_HTTP_CREDENTIAL"}}}).to_string(),
    ).unwrap();
    let request = crate::credential_store::request(
        &state.data_root,
        &library,
        &sessions,
        &owner,
        &session,
        &turn.id,
        "private-http-fixture",
        "api",
        "ANCHOR_TEST_PRIVATE_HTTP_CREDENTIAL",
        "connect declared API",
    )
    .unwrap();
    let list_uri = format!("/sessions/{}/credential-requests", session.id);
    let value_uri = format!("{list_uri}/{}/value", request.id);
    let app = router(state.clone());
    let (status, body) = private_call(app.clone(), "GET", &list_uri, &keys[0], "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["requests"][0]["status"],
        "pending"
    );
    let value = "private-credential-value-for-http-fixture";
    let (status, _) = private_call(
        app.clone(),
        "POST",
        &value_uri,
        &keys[0],
        &json!({"value":value,"approved":true}).to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) =
        private_call(app.clone(), "POST", &value_uri, &keys[0], "malformed JSON").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        crate::credential_store::list(&state.data_root, &sessions, &owner, &session).unwrap()[0]
            .status,
        "pending"
    );
    let (status, body) = private_call(
        app.clone(),
        "POST",
        &value_uri,
        &keys[0],
        &json!({"value":value}).to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["request"]["status"],
        "configured"
    );
    let (status, body) = private_call(app, "GET", &list_uri, &keys[0], "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["requests"][0]["status"],
        "configured"
    );
    assert!(
        !serde_json::to_string(&sessions.events(&owner, &session.id, 0).unwrap())
            .unwrap()
            .contains(value)
    );
    let definition = FilePluginCatalog::new(&library)
        .definition("private-http-fixture")
        .unwrap();
    assert_eq!(
        crate::credential_store::environment_values(
            &state.data_root,
            &crate::credential_store::binding_owner(&owner),
            &definition,
            "api"
        )
        .unwrap()["ANCHOR_TEST_PRIVATE_HTTP_CREDENTIAL"],
        value
    );
    assert!(
        crate::credential_store::environment_values(
            &state.data_root,
            &crate::credential_store::binding_owner("other-owner"),
            &definition,
            "api"
        )
        .unwrap()
        .is_empty()
    );
}
