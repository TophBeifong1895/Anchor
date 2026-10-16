use super::*;
use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use tower::ServiceExt;

async fn call(
    state: &ApiState,
    method: &str,
    key: &str,
    suffix: &str,
    body: Value,
) -> (StatusCode, Value) {
    let response = crate::api::router(state.clone())
        .oneshot(
            Request::builder()
                .method(method)
                .uri(format!("/plugins/config-demo/configuration{suffix}"))
                .header(header::AUTHORIZATION, format!("Bearer {key}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(response.headers()["referrer-policy"], "no-referrer");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(!text.contains("api-private-secret"));
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

#[tokio::test]
async fn plugin_configuration_api_is_private_owner_bound_and_rejects_stale_updates_and_tests() {
    let _env = crate::PROCESS_ENV_LOCK.lock().await;
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
    let _restore = Restore(
        [
            "ANCHOR_RUNNER_LIBRARY_ROOT",
            "ANCHOR_PLUGIN_EXTERNAL_CONFIGURATION",
            "ANCHOR_CONFIG_TEST_KEY",
            "ANCHOR_CONFIG_TEST_ENDPOINT",
        ]
        .into_iter()
        .map(|name| (name, std::env::var_os(name)))
        .collect(),
    );
    unsafe {
        std::env::remove_var("ANCHOR_RUNNER_LIBRARY_ROOT");
    }
    let (_root, mut state) = crate::api::tests::fixture();
    state.api_keys = vec!["a".repeat(32), "b".repeat(32)];
    let key = state.api_keys[0].clone();
    let other = state.api_keys[1].clone();
    let plugin = crate::library_root(&state.catalog_root).join("plugins/config-demo");
    std::fs::create_dir_all(&plugin).unwrap();
    std::fs::write(plugin.join("plugin.json"), json!({"name":"Config demo","configuration":{
        "fields":{"endpoint":{"type":"url","default":"http://127.0.0.1:1"},"key":{"type":"secret"}},
        "environment":{"api":{"ANCHOR_CONFIG_TEST_ENDPOINT":"endpoint","ANCHOR_CONFIG_TEST_KEY":"key"}}},
        "mcpServers":{"api":{"url":"${ANCHOR_CONFIG_TEST_ENDPOINT}/mcp","bearer_token_env_var":"ANCHOR_CONFIG_TEST_KEY"}}}).to_string()).unwrap();
    let (status, _) = call(&state, "GET", "invalid", "", json!({})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, initial) = call(&state, "GET", &key, "", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{initial}");
    assert_eq!(initial["revision"], 0);
    let digest = initial["authorization_digest"].as_str().unwrap();
    let update = json!({"authorization_digest":digest,"revision":0,"fields":{"key":{"action":"set","value":"api-private-secret"}}});
    let (status, saved) = call(&state, "PUT", &key, "", update.clone()).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["revision"], 1);
    assert_eq!(saved["fields"][1]["state"], "configured");
    assert!(saved["fields"][1].get("value").is_none());
    let (_, other_view) = call(&state, "GET", &other, "", json!({})).await;
    assert_eq!(other_view["revision"], 0);
    assert_eq!(other_view["fields"][1]["state"], "missing");
    assert_eq!(
        call(&state, "PUT", &key, "", update).await.0,
        StatusCode::CONFLICT
    );
    let (status, failed) = call(
        &state,
        "POST",
        &key,
        "/test",
        json!({"authorization_digest":digest,"revision":0}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{failed}");
    assert!(failed.to_string().contains("configuration changed"));
    // The actual preparation path acquires the Library publication lease once.
    // A closed local endpoint yields a bounded failure without any provider or
    // external service, and catches recursive management/preparation locking.
    let (status, _) = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        call(
            &state,
            "POST",
            &key,
            "/test",
            json!({"authorization_digest":digest,"revision":1}),
        ),
    )
    .await
    .expect("connection test must not recursively acquire the Library lease");
    assert!(matches!(status, StatusCode::OK | StatusCode::CONFLICT));
    let (status, _) = call(
        &state,
        "PUT",
        &key,
        "",
        json!({"authorization_digest":digest,"revision":1,
        "fields":{"endpoint":{"action":"clear"},"key":{"action":"clear"}}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, cleared) = call(&state, "GET", &key, "", json!({})).await;
    assert_eq!(cleared["revision"], 2);
    assert_eq!(cleared["fields"][0]["state"], "missing");
    assert!(cleared["fields"][0].get("value").is_none());
    unsafe {
        std::env::set_var("ANCHOR_CONFIG_TEST_KEY", "api-private-secret");
        std::env::set_var("ANCHOR_CONFIG_TEST_ENDPOINT", "https://example.com/");
        std::env::set_var(
            "ANCHOR_PLUGIN_EXTERNAL_CONFIGURATION",
            json!({"config-demo":{
            "key":"ANCHOR_CONFIG_TEST_KEY", "endpoint":"ANCHOR_CONFIG_TEST_ENDPOINT"}})
            .to_string(),
        );
    }
    let (status, external) = call(&state, "GET", &key, "", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(external["fields"][0]["value"], "https://example.com");
    assert_eq!(external["fields"][0]["source"], "external");
    assert_eq!(external["fields"][1]["source"], "external");
    assert_eq!(external["fields"][1]["state"], "deployment");
    assert_eq!(external["fields"][1]["editable"], false);
    assert!(external["fields"][1].get("value").is_none());
    assert_eq!(
        call(
            &state,
            "PUT",
            &key,
            "",
            json!({"authorization_digest":digest,"revision":2,
        "fields":{"key":{"action":"clear"}}})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
}
