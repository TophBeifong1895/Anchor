use super::*;
fn fixture() -> (tempfile::TempDir, ModelConfiguration) {
    let root = tempfile::tempdir().unwrap();
    let configuration = ModelConfiguration::new(root.path(), BTreeMap::new()).unwrap();
    (root, configuration)
}
fn update(endpoint: &str, model: &str, key: Value) -> ModelUpdate {
    serde_json::from_value(json!({"endpoint":endpoint,"model":model,"wire_api":"chat","context_window":32000,"max_tokens":256,"aliases":{"models.research":"research"},"api_key":key})).unwrap()
}
#[test]
fn first_run_can_be_configured_without_environment_or_secret_projection() {
    let (root, configuration) = fixture();
    assert_eq!(configuration.status().unwrap()["configured"], false);
    configuration
        .save(update(
            "https://provider.example/v1",
            "first",
            json!({"action":"set","value":"private-first-key"}),
        ))
        .unwrap();
    let status = configuration.status().unwrap();
    assert_eq!(status["configured"], true);
    assert_eq!(status["api_key_present"], true);
    assert!(!status.to_string().contains("private-first-key"));
    assert_eq!(
        fs::metadata(root.path().join("managed/model-config.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let restored = ModelConfiguration::new(root.path(), BTreeMap::new()).unwrap();
    assert_eq!(
        restored
            .snapshot(None, None)
            .unwrap()
            .resolve(None)
            .unwrap()
            .model,
        "first"
    );
}
#[test]
fn model_changes_preserve_history_and_rotation_updates_one_provider_credential() {
    let (root, configuration) = fixture();
    let endpoint = "https://provider.example/v1";
    configuration
        .save(update(
            endpoint,
            "first",
            json!({"action":"set","value":"old-private-key"}),
        ))
        .unwrap();
    let frozen = configuration.snapshot(None, None).unwrap();
    let old_binding = frozen.resolve(None).unwrap().identity;
    configuration
        .save(update(
            endpoint,
            "second",
            json!({"action":"set","value":"rotated-private-key"}),
        ))
        .unwrap();
    let retained = configuration.snapshot(Some(&old_binding), None).unwrap();
    assert_eq!(retained.resolve(None).unwrap().model, "first");
    assert_eq!(
        retained.connection_test_upstream().api_key,
        "rotated-private-key"
    );
    assert_eq!(frozen.connection_test_upstream().api_key, "old-private-key");
    let binding = configuration
        .snapshot(None, None)
        .unwrap()
        .resolve(None)
        .unwrap()
        .identity;
    configuration
        .save(update(
            endpoint,
            "second",
            json!({"action":"set","value":"third-private-key"}),
        ))
        .unwrap();
    assert_eq!(
        configuration
            .snapshot(None, None)
            .unwrap()
            .resolve(None)
            .unwrap()
            .identity,
        binding
    );
    let bytes = fs::read(root.path().join("managed/model-config.json")).unwrap();
    let serialized = String::from_utf8(bytes).unwrap();
    assert!(!serialized.contains("old-private-key"));
    assert!(!serialized.contains("rotated-private-key"));
    assert_eq!(serialized.matches("third-private-key").count(), 1);
    configuration
        .save(update(endpoint, "second", json!({"action":"clear"})))
        .unwrap();
    assert!(configuration.snapshot(Some(&old_binding), None).is_err());
    assert!(
        !String::from_utf8(fs::read(root.path().join("managed/model-config.json")).unwrap())
            .unwrap()
            .contains("third-private-key")
    );
}
#[test]
fn old_endpoint_can_continue_until_its_visible_credential_is_cleared() {
    let (_root, configuration) = fixture();
    configuration
        .save(update(
            "https://old.example/v1",
            "first",
            json!({"action":"set","value":"old-key"}),
        ))
        .unwrap();
    let old = configuration
        .snapshot(None, None)
        .unwrap()
        .resolve(None)
        .unwrap()
        .identity;
    configuration
        .save(update(
            "https://new.example/v1",
            "second",
            json!({"action":"set","value":"new-key"}),
        ))
        .unwrap();
    let retained = configuration.snapshot(Some(&old), None).unwrap();
    assert_eq!(
        retained.connection_test_upstream().url.as_str(),
        "https://old.example/v1/chat/completions"
    );
    assert_eq!(retained.connection_test_upstream().api_key, "old-key");
    let status = configuration.status().unwrap();
    let provider = status["retained_providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|provider| provider["current"] == false)
        .unwrap();
    configuration
        .clear_provider_credential(provider["id"].as_str().unwrap())
        .unwrap();
    assert!(configuration.snapshot(Some(&old), None).is_err());
    assert_eq!(
        configuration
            .snapshot(None, None)
            .unwrap()
            .connection_test_upstream()
            .api_key,
        "new-key"
    );
}
#[test]
fn rejected_edit_keeps_previous_file_and_deployment_fields_are_read_only() {
    let (root, configuration) = fixture();
    configuration
        .save(update(
            "https://provider.example/v1",
            "first",
            json!({"action":"set","value":"private-key"}),
        ))
        .unwrap();
    let path = root.path().join("managed/model-config.json");
    let before = fs::read(&path).unwrap();
    assert!(
        configuration
            .save(update(
                "http://untrusted.example/v1",
                "bad",
                json!({"action":"keep"})
            ))
            .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    let external = ModelConfiguration::new(
        root.path(),
        BTreeMap::from([
            ("ANCHOR_MODEL_NAME".into(), "deployment-model".into()),
            (
                "ANCHOR_MODEL_API_KEY".into(),
                "deployment-private-key".into(),
            ),
        ]),
    )
    .unwrap();
    assert!(
        external
            .save(update(
                "https://provider.example/v1",
                "change",
                json!({"action":"keep"})
            ))
            .is_err()
    );
    assert!(
        external
            .save(update(
                "https://provider.example/v1",
                "deployment-model",
                json!({"action":"clear"})
            ))
            .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    assert!(
        !external
            .status()
            .unwrap()
            .to_string()
            .contains("deployment-private-key")
    );
}
#[tokio::test]
async fn connection_test_calls_provider_without_saving_and_redacts_error_body() {
    use axum::{
        Json, Router,
        http::{HeaderMap, StatusCode},
        response::IntoResponse,
        routing::post,
    };
    let (_root, configuration) = fixture();
    let app = Router::new().route(
        "/v1/chat/completions",
        post(|headers: HeaderMap, Json(body): Json<Value>| async move {
            assert_eq!(
                headers.get("authorization").unwrap(),
                "Bearer connection-private-key"
            );
            assert_eq!(body["stream"], false);
            if body["model"] == "failed" {
                return (StatusCode::UNAUTHORIZED, "connection-private-key").into_response();
            }
            Json(json!({"choices":[{"message":{"role":"assistant","content":"OK"}}]}))
                .into_response()
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let result = configuration
        .test(update(
            &endpoint,
            "working",
            json!({"action":"set","value":"connection-private-key"}),
        ))
        .await
        .unwrap();
    assert_eq!(result["ok"], true);
    assert_eq!(result["saved"], false);
    assert!(!configuration.root.join("model-config.json").exists());
    let failure = configuration
        .test(update(
            &endpoint,
            "failed",
            json!({"action":"set","value":"connection-private-key"}),
        ))
        .await
        .unwrap_err();
    assert!(failure.contains("401"));
    assert!(!failure.contains("connection-private-key"));
    server.abort();
}

#[test]
fn retained_key_can_be_restored_without_switching_the_current_model() {
    let (_root, configuration) = fixture();
    configuration
        .save(update(
            "https://old.example/v1",
            "first",
            json!({"action":"set","value":"old-key"}),
        ))
        .unwrap();
    let old_binding = configuration
        .snapshot(None, None)
        .unwrap()
        .resolve(None)
        .unwrap()
        .identity;
    configuration
        .save(update(
            "https://new.example/v1",
            "second",
            json!({"action":"set","value":"current-key"}),
        ))
        .unwrap();
    let status = configuration.status().unwrap();
    let old_provider = status["retained_providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|provider| provider["current"] == false)
        .unwrap()["id"]
        .as_str()
        .unwrap();
    configuration
        .clear_provider_credential(old_provider)
        .unwrap();
    configuration
        .set_provider_credential(old_provider, "restored-key".into())
        .unwrap();
    assert_eq!(
        configuration
            .snapshot(Some(&old_binding), None)
            .unwrap()
            .connection_test_upstream()
            .api_key,
        "restored-key"
    );
    let current = configuration.snapshot(None, None).unwrap();
    assert_eq!(current.resolve(None).unwrap().model, "second");
    assert_eq!(current.connection_test_upstream().api_key, "current-key");
}

#[test]
fn malformed_header_keys_are_rejected_without_echoing_or_changing_saved_configuration() {
    let (root, configuration) = fixture();
    let endpoint = "https://provider.example/v1";
    configuration
        .save(update(
            endpoint,
            "model",
            json!({"action":"set","value":"valid-key"}),
        ))
        .unwrap();
    let path = root.path().join("managed/model-config.json");
    let before = fs::read(&path).unwrap();
    for key in ["private-中文-key", "private key", "private-key\n"] {
        let failure = configuration
            .save(update(
                endpoint,
                "model",
                json!({"action":"set","value":key}),
            ))
            .unwrap_err();
        assert!(failure.contains("ANCHOR_MODEL_API_KEY"));
        assert!(!failure.contains(key));
        assert_eq!(fs::read(&path).unwrap(), before);
    }
}

#[test]
fn managed_model_file_refuses_fifo_and_symlink_without_waiting_for_a_writer() {
    let (_root, configuration) = fixture();
    let path = configuration.root.join("model-config.json");
    rustix::fs::mkfifoat(
        rustix::fs::CWD,
        &path,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
    )
    .unwrap();
    assert!(configuration.status().is_err());
    fs::remove_file(&path).unwrap();
    let target = configuration.root.join("private.json");
    fs::write(&target, b"{}").unwrap();
    std::os::unix::fs::symlink(&target, &path).unwrap();
    assert!(configuration.status().is_err());
}
