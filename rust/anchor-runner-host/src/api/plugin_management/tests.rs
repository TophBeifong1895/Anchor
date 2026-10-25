use super::*;
use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use tower::ServiceExt;

async fn call(
    state: &ApiState,
    method: &str,
    uri: &str,
    key: &str,
    body: Value,
) -> (StatusCode, Value) {
    let result = super::super::router(state.clone())
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {key}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    if super::is_private_path(uri) || super::super::credential_requests::is_private_path(uri) {
        assert_eq!(result.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(result.headers()["referrer-policy"], "no-referrer");
    }
    let status = result.status();
    let text = String::from_utf8(
        to_bytes(result.into_body(), 64 * 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(!text.contains("management-private-secret"));
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}
async fn settle(state: &ApiState, key: &str, mut value: Value) -> Value {
    for _ in 0..200 {
        if !matches!(value["state"].as_str(), Some("preparing" | "running")) {
            return value;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let (status, next) = call(
            state,
            "GET",
            &format!("/plugins/preflights/{}", value["token"].as_str().unwrap()),
            key,
            json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{next}");
        value = next;
    }
    panic!("Plugin operation did not settle: {value}");
}
fn candidate(name: &str) -> Value {
    json!({"id":"managed-demo","files":[{"path":"plugin.json","content_base64":STANDARD.encode(json!({"name":name,
        "mcpServers":{"api":{"url":"https://example.com/mcp","bearer_token_env_var":"ANCHOR_TEST_MANAGEMENT_CREDENTIAL"}}}).to_string())}]})
}

#[tokio::test]
async fn standalone_install_credentials_rotation_update_and_uninstall_keep_secrets_private() {
    let _env = crate::PROCESS_ENV_LOCK.lock().await;
    let (_root, state) = crate::api::tests::fixture();
    let key = "a".repeat(32);
    let (status, review) = call(
        &state,
        "POST",
        "/plugins/install-preflight",
        &key,
        candidate("Managed demo"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{review}");
    let review = settle(&state, &key, review).await;
    // A review stages under the data root, where the next start-up can see
    // what a lost process left behind; /tmp collected one per review.
    let staged = std::fs::read_dir(lifecycle::staging_root(&state.data_root))
        .unwrap()
        .count();
    assert_eq!(staged, 1);
    assert!(
        !state
            .application
            .library_root()
            .to_path_buf()
            .join("plugins/managed-demo")
            .exists()
    );
    let uri = format!(
        "/plugins/preflights/{}/confirm",
        review["token"].as_str().unwrap()
    );
    let guard = state.application.graph_catalog_mutation_guard().await;
    let (status, result) = call(&state, "POST", &uri, &key, json!({"confirm":true})).await;
    assert_eq!(result["state"], "running");
    assert_eq!(
        call(&state, "POST", &uri, &key, json!({"confirm":true}))
            .await
            .1["state"],
        "running"
    );
    drop(guard);
    let result = settle(&state, &key, result).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["result"]["digest"], review["digest"]);
    assert_eq!(
        call(&state, "POST", &uri, &key, json!({"confirm":true}))
            .await
            .0,
        StatusCode::OK
    );
    let path = "/plugins/managed-demo/credentials";
    let (status, metadata) = call(&state, "GET", path, &key, json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(metadata["targets"][0]["fields"][0]["state"], "missing");
    let digest = metadata["targets"][0]["authorization_digest"].clone();
    let value_uri = format!("{path}/api/ANCHOR_TEST_MANAGEMENT_CREDENTIAL");
    let sessions = store(&state).unwrap();
    let owner = private_owner(
        &state,
        &HeaderMap::from_iter([(
            header::AUTHORIZATION,
            format!("Bearer {key}").parse().unwrap(),
        )]),
    );
    let (status, created) = call(&state, "POST", "/sessions", &key, json!({})).await;
    assert_eq!(status, StatusCode::CREATED);
    let session_id = created["session"]["id"].as_str().unwrap();
    assert!(session_id.starts_with("pilot-private-"));
    assert!(
        call(&state, "GET", "/sessions", &key, json!({}))
            .await
            .1
            .to_string()
            .contains(session_id)
    );
    let session = sessions.get(&owner, session_id).unwrap();
    assert_eq!(owner, "local");
    let (turn, _) = sessions
        .create_turn(&owner, &session.id, "credential-fixture", Some("configure"))
        .unwrap();
    // A real pending request is optional: management itself never creates one.
    let request = crate::credential_store::request(
        &state.data_root,
        state.application.library_root(),
        &sessions,
        &owner,
        &session,
        &turn.id,
        "managed-demo",
        "api",
        "ANCHOR_TEST_MANAGEMENT_CREDENTIAL",
        "configure declared API",
    )
    .unwrap();
    for value in [
        "management-private-secret-one",
        "management-private-secret-two",
    ] {
        let (status, metadata) = call(
            &state,
            "PUT",
            &value_uri,
            &key,
            json!({"authorization_digest":digest,"value":value}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{metadata}");
        assert_eq!(metadata["targets"][0]["fields"][0]["state"], "configured");
    }
    assert_eq!(
        crate::credential_store::get(&state.data_root, &sessions, &owner, &session, &request.id)
            .unwrap()
            .status,
        "configured"
    );
    assert_eq!(
        call(
            &state,
            "POST",
            &format!(
                "/sessions/{}/credential-requests/{}/value",
                session.id, request.id
            ),
            &key,
            json!({"value":"management-private-secret-late"})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(
            &state,
            "PUT",
            &value_uri,
            &key,
            json!({"authorization_digest":"old","value":"management-private-secret"})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(
            &state,
            "DELETE",
            &value_uri,
            &key,
            json!({"authorization_digest":digest})
        )
        .await
        .1["targets"][0]["fields"][0]["state"],
        "missing"
    );
    let (status, update) = call(
        &state,
        "POST",
        "/plugins/install-preflight",
        &key,
        candidate("Updated demo"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let update = settle(&state, &key, update).await;
    assert_eq!(update["replacing"], true);
    let update = settle(&state, &key, update).await;
    let update_uri = format!(
        "/plugins/preflights/{}/confirm",
        update["token"].as_str().unwrap()
    );
    assert_eq!(
        call(&state, "POST", &update_uri, &key, json!({"confirm":true}))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let (_, update) = call(
        &state,
        "POST",
        "/plugins/install-preflight",
        &key,
        candidate("Updated demo"),
    )
    .await;
    let update = settle(&state, &key, update).await;
    let update_uri = format!(
        "/plugins/preflights/{}/confirm",
        update["token"].as_str().unwrap()
    );
    let (status, result) = call(
        &state,
        "POST",
        &update_uri,
        &key,
        json!({"confirm":true,"replace_existing":true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(settle(&state, &key, result).await["state"], "succeeded");
    let (status, removal) = call(
        &state,
        "POST",
        "/plugins/managed-demo/uninstall-preflight",
        &key,
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{removal}");
    let remove_uri = format!(
        "/plugins/preflights/{}/confirm",
        removal["token"].as_str().unwrap()
    );
    assert_eq!(
        call(&state, "POST", &remove_uri, &key, json!({"confirm":true}))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let (_, removal) = call(
        &state,
        "POST",
        "/plugins/managed-demo/uninstall-preflight",
        &key,
        json!({}),
    )
    .await;
    let remove_uri = format!(
        "/plugins/preflights/{}/confirm",
        removal["token"].as_str().unwrap()
    );
    let (status, result) = call(
        &state,
        "POST",
        &remove_uri,
        &key,
        json!({"confirm":true,"confirm_data_loss":true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(settle(&state, &key, result).await["state"], "succeeded");
    assert_eq!(
        call(
            &state,
            "POST",
            &remove_uri,
            &key,
            json!({"confirm":true,"confirm_data_loss":true})
        )
        .await
        .1["state"],
        "succeeded"
    );
    assert!(
        !state
            .application
            .library_root()
            .to_path_buf()
            .join("plugins/managed-demo")
            .exists()
    );
}

#[test]
fn start_up_removes_staging_an_earlier_process_left() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(lifecycle::sweep_staging(root.path()), 0);
    let staging = lifecycle::staging_root(root.path());
    for name in ["upload-a", "upload-b"] {
        std::fs::create_dir_all(staging.join(name).join("candidate")).unwrap();
        std::fs::write(staging.join(name).join("candidate/plugin.json"), "{}").unwrap();
    }
    assert_eq!(lifecycle::sweep_staging(root.path()), 2);
    assert_eq!(std::fs::read_dir(&staging).unwrap().count(), 0);
}

#[tokio::test]
async fn upload_rejects_links_unknown_fields_and_traversal_privately() {
    let (_root, state) = crate::api::tests::fixture();
    for body in [
        json!({"id":"demo","files":[{"path":"../escape","content_base64":""}]}),
        json!({"id":"demo","files":[{"path":"link","content_base64":"","type":"symlink"}]}),
        json!({"id":"demo","directory":"/etc"}),
    ] {
        let (status, result) = call(&state, "POST", "/plugins/install-preflight", "", body).await;
        if status == StatusCode::OK {
            assert_eq!(settle(&state, "", result).await["state"], "failed");
        } else {
            assert!(
                status == StatusCode::BAD_REQUEST || status == StatusCode::UNPROCESSABLE_ENTITY,
                "{status}"
            );
        }
    }
}
