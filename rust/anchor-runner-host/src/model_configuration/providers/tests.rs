use super::*;

fn fixture() -> (tempfile::TempDir, ModelConfiguration) {
    let root = tempfile::tempdir().unwrap();
    let configuration = ModelConfiguration::new(root.path(), BTreeMap::new()).unwrap();
    (root, configuration)
}
fn update(endpoint: &str, flash: &str, pro: &str, api_key: Value) -> ProviderUpdate {
    serde_json::from_value(
        json!({"endpoint":endpoint,"wire_api":"chat","flash":flash,"pro":pro,"api_key":api_key}),
    )
    .unwrap()
}
fn legacy(configuration: &ModelConfiguration, endpoint: &str) {
    configuration.save(serde_json::from_value(json!({"endpoint":endpoint,"wire_api":"chat","model":"legacy-model","aliases":{"models.research":"legacy-research"},"api_key":{"action":"set","value":"legacy-private-key"}})).unwrap()).unwrap();
}
fn saved(configuration: &ModelConfiguration) -> Vec<u8> {
    fs::read(configuration.root.join("model-config.json")).unwrap()
}
fn slot_key(configuration: &ModelConfiguration, slot: &str) -> Option<String> {
    configuration
        .load()
        .unwrap()
        .credentials
        .get(&format!("slot:{slot}"))
        .cloned()
}

#[test]
fn partial_provider_settings_do_not_configure_or_route_execution() {
    let (_root, configuration) = fixture();
    let initial = configuration.status().unwrap();
    assert_eq!(initial["providers"]["default"]["source"], "empty");
    assert_eq!(initial["providers"]["backup"]["fields"]["pro"], "");
    let status = configuration
        .save_provider("default", update("", "", "", json!({"action":"keep"})))
        .unwrap();
    assert_eq!(status["providers"]["default"]["source"], "host");
    assert_eq!(status["providers"]["default"]["configured"], false);
    let status = configuration
        .save_provider(
            "backup",
            update(
                "https://provider.example/v1",
                "quick",
                "",
                json!({"action":"set","value":"slot-private-key"}),
            ),
        )
        .unwrap();
    assert_eq!(status["providers"]["backup"]["configured"], false);
    assert_eq!(status["providers"]["backup"]["api_key_present"], true);
    let status = configuration
        .save_provider(
            "backup",
            update(
                "https://provider.example/v1",
                "quick",
                "strong",
                json!({"action":"keep"}),
            ),
        )
        .unwrap();
    assert_eq!(status["providers"]["backup"]["configured"], true);
    assert_eq!(status["configured"], false);
    assert!(configuration.snapshot(None, None).is_err());
    assert!(configuration.load().unwrap().history.is_empty());
    assert!(!status.to_string().contains("slot-private-key"));
}

#[test]
fn v1_legacy_preview_imports_only_default_without_modifying_execution() {
    let (root, configuration) = fixture();
    let endpoint = "https://legacy.example/v1";
    legacy(&configuration, endpoint);
    let mut v1: Value = serde_json::from_slice(&saved(&configuration)).unwrap();
    v1["format"] = json!(1);
    v1.as_object_mut().unwrap().remove("providers");
    fs::write(
        configuration.root.join("model-config.json"),
        serde_json::to_vec(&v1).unwrap(),
    )
    .unwrap();
    let before = saved(&configuration);
    let status = configuration.status().unwrap();
    assert_eq!(status["providers"]["default"]["source"], "legacy");
    assert_eq!(
        status["providers"]["default"]["fields"]["flash"],
        "legacy-model"
    );
    assert_eq!(status["providers"]["default"]["fields"]["pro"], "");
    assert_eq!(status["providers"]["default"]["api_key_present"], true);
    assert_eq!(status["providers"]["backup"]["api_key_present"], false);
    assert_eq!(saved(&configuration), before);
    let binding = configuration
        .snapshot(None, None)
        .unwrap()
        .resolve(None)
        .unwrap()
        .identity;
    configuration
        .save_provider(
            "default",
            update(endpoint, "fast", "strong", json!({"action":"keep"})),
        )
        .unwrap();
    let v2: Value = serde_json::from_slice(&saved(&configuration)).unwrap();
    assert_eq!(v2["format"], 3);
    for field in ["current", "history"] {
        assert_eq!(v2[field], v1[field]);
    }
    assert_eq!(
        slot_key(&configuration, "default").as_deref(),
        Some("legacy-private-key")
    );
    let registry = configuration.snapshot(Some(&binding), None).unwrap();
    assert_eq!(registry.resolve(None).unwrap().model, "legacy-model");
    assert_eq!(
        registry.resolve(Some("models.research")).unwrap().model,
        "legacy-research"
    );
    assert_eq!(
        registry.connection_test_upstream().api_key,
        "legacy-private-key"
    );
    let restored = ModelConfiguration::new(root.path(), BTreeMap::new()).unwrap();
    assert_eq!(
        restored.status().unwrap()["providers"]["default"]["source"],
        "host"
    );
    legacy(&restored, "https://changed.example/v1");
    assert_eq!(
        restored.status().unwrap()["providers"]["default"]["fields"]["endpoint"],
        endpoint
    );
}

#[test]
fn editable_slots_do_not_take_over_environment_or_share_credentials() {
    let root = tempfile::tempdir().unwrap();
    let endpoint = "https://provider.example/v1";
    let configuration = ModelConfiguration::new(
        root.path(),
        BTreeMap::from([
            ("ANCHOR_MODEL_URL".into(), endpoint.into()),
            ("ANCHOR_MODEL_WIRE_API".into(), "chat".into()),
            ("ANCHOR_MODEL_NAME".into(), "deployment-model".into()),
            (
                "ANCHOR_MODEL_API_KEY".into(),
                "deployment-private-key".into(),
            ),
        ]),
    )
    .unwrap();
    configuration
        .save_provider(
            "default",
            update(endpoint, "fast", "strong", json!({"action":"keep"})),
        )
        .unwrap();
    configuration
        .save_provider(
            "backup",
            update(
                endpoint,
                "fast",
                "strong",
                json!({"action":"set","value":"backup-private-key"}),
            ),
        )
        .unwrap();
    configuration
        .save_provider(
            "default",
            update(
                endpoint,
                "fast",
                "strong",
                json!({"action":"set","value":"default-private-key"}),
            ),
        )
        .unwrap();
    assert_eq!(
        slot_key(&configuration, "default").as_deref(),
        Some("default-private-key")
    );
    assert_eq!(
        slot_key(&configuration, "backup").as_deref(),
        Some("backup-private-key")
    );
    configuration
        .save_provider(
            "default",
            update(endpoint, "fast", "strong", json!({"action":"clear"})),
        )
        .unwrap();
    let status = configuration.status().unwrap();
    assert_eq!(status["providers"]["default"]["api_key_present"], false);
    assert_eq!(status["providers"]["backup"]["api_key_present"], true);
    assert_eq!(
        slot_key(&configuration, "backup").as_deref(),
        Some("backup-private-key")
    );
    let registry = configuration.snapshot(None, None).unwrap();
    assert_eq!(registry.resolve(None).unwrap().model, "deployment-model");
    assert_eq!(
        registry.connection_test_upstream().api_key,
        "deployment-private-key"
    );
    let serialized = String::from_utf8(saved(&configuration)).unwrap();
    assert!(!serialized.contains("default-private-key"));
    assert_eq!(serialized.matches("backup-private-key").count(), 1);
    assert!(!status.to_string().contains("private-key"));
    assert_eq!(
        fs::metadata(configuration.root.join("model-config.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[test]
fn changed_address_or_protocol_never_reuses_or_reinherits_a_key() {
    let (_root, configuration) = fixture();
    let endpoint = "https://provider.example/v1";
    legacy(&configuration, endpoint);
    configuration
        .save_provider(
            "default",
            update(
                "https://new.example/v1",
                "fast",
                "strong",
                json!({"action":"keep"}),
            ),
        )
        .unwrap();
    assert_eq!(slot_key(&configuration, "default"), None);
    configuration
        .save_provider(
            "default",
            update(endpoint, "fast", "strong", json!({"action":"keep"})),
        )
        .unwrap();
    assert_eq!(slot_key(&configuration, "default"), None);
    configuration
        .save_provider(
            "default",
            update(
                endpoint,
                "fast",
                "strong",
                json!({"action":"set","value":"private-key"}),
            ),
        )
        .unwrap();
    let mut changed = update(endpoint, "fast", "strong", json!({"action":"keep"}));
    changed.fields.wire_api = "responses".into();
    configuration.save_provider("default", changed).unwrap();
    assert_eq!(slot_key(&configuration, "default"), None);
    assert_eq!(
        configuration
            .snapshot(None, None)
            .unwrap()
            .connection_test_upstream()
            .api_key,
        "legacy-private-key"
    );
}

#[test]
fn rejected_provider_edits_leave_saved_settings_intact_and_redact_keys() {
    let (_root, configuration) = fixture();
    configuration
        .save_provider("backup", update("", "", "", json!({"action":"keep"})))
        .unwrap();
    let before = saved(&configuration);
    for (endpoint, model, key) in [
        ("", "fast", "valid-key"),
        ("http://untrusted.example/v1", "fast", "valid-key"),
        ("https://provider.example/v1", "fast\ninvalid", "valid-key"),
        ("https://provider.example/v1", "fast", "private-中文-key"),
        ("https://provider.example/v1", "fast", "private key"),
    ] {
        let reason = configuration
            .save_provider(
                "backup",
                update(endpoint, model, "pro", json!({"action":"set","value":key})),
            )
            .unwrap_err();
        assert!(!reason.contains(key));
        assert_eq!(saved(&configuration), before);
    }
    assert!(
        configuration
            .save_provider("unknown", update("", "", "", json!({"action":"clear"})))
            .is_err()
    );
    assert_eq!(saved(&configuration), before);
    assert!(
        serde_json::from_value::<ProviderUpdate>(json!({"endpoint":"", "model":"invented"}))
            .is_err()
    );
    assert!(serde_json::from_value::<ProviderTest>(json!({"tier":"automatic"})).is_err());
    assert!(
        serde_json::from_value::<ProviderTest>(json!({"tier":"flash", "fallback":true})).is_err()
    );
}

#[tokio::test]
async fn provider_tests_call_the_explicit_tier_without_saving_or_routing() {
    use axum::{
        Json, Router,
        http::{HeaderMap, StatusCode},
        response::IntoResponse,
        routing::post,
    };
    let observations = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let observed = observations.clone();
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move |headers: HeaderMap, Json(body): Json<Value>| {
            observed.lock().unwrap().push((
                headers
                    .get("authorization")
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_owned(),
                body.clone(),
            ));
            async move {
                if body["model"] == "failure" {
                    return (StatusCode::UNAUTHORIZED, "draft-private-key").into_response();
                }
                Json(json!({"choices":[{"message":{"role":"assistant","content":"OK"}}]}))
                    .into_response()
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let (_root, configuration) = fixture();
    let make_test = |tier: &str, pro: &str| {
        serde_json::from_value(json!({"endpoint":endpoint,"wire_api":"chat","flash":"fast","pro":pro,"tier":tier,"api_key":{"action":"set","value":"draft-private-key"}})).unwrap()
    };
    for (tier, model) in [("flash", "fast"), ("pro", "strong")] {
        let result = configuration
            .test_provider("backup", make_test(tier, "strong"))
            .await
            .unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["saved"], false);
        assert_eq!(result["model"], model);
    }
    let failure = configuration
        .test_provider("backup", make_test("pro", "failure"))
        .await
        .unwrap_err();
    assert!(failure.contains("401"));
    assert!(!failure.contains("draft-private-key"));
    assert!(!configuration.root.join("model-config.json").exists());
    legacy(&configuration, &endpoint);
    let before = saved(&configuration);
    let mut inherited: ProviderTest = serde_json::from_value(
        json!({"endpoint":endpoint,"wire_api":"chat","flash":"fast","pro":"strong","tier":"pro"}),
    )
    .unwrap();
    configuration
        .test_provider("default", inherited)
        .await
        .unwrap();
    assert_eq!(saved(&configuration), before);
    inherited = serde_json::from_value(json!({"endpoint":"https://another.example/v1","wire_api":"chat","flash":"fast","pro":"strong","tier":"pro"})).unwrap();
    assert!(
        configuration
            .test_provider("default", inherited)
            .await
            .unwrap_err()
            .contains("no API key")
    );
    assert_eq!(saved(&configuration), before);
    assert_eq!(
        configuration
            .snapshot(None, None)
            .unwrap()
            .resolve(None)
            .unwrap()
            .model,
        "legacy-model"
    );
    let observations = observations.lock().unwrap();
    assert_eq!(observations.len(), 4);
    assert_eq!(observations[0].0, "Bearer draft-private-key");
    assert_eq!(observations[1].1["model"], "strong");
    assert_eq!(observations[3].0, "Bearer legacy-private-key");
    server.abort();
}
