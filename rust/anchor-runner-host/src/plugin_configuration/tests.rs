use super::*;
use anchor_graph_host::FilePluginCatalog;
use std::fs;

fn fixture() -> (tempfile::TempDir, SessionStore, PluginDefinition) {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("plugins/demo")).unwrap();
    fs::create_dir_all(root.path().join("state/platform")).unwrap();
    fs::write(
        root.path().join("plugins/demo/plugin.json"),
        manifest("https://example.com").to_string(),
    )
    .unwrap();
    let sessions = SessionStore::open(root.path().join("state/platform/sessions.sqlite")).unwrap();
    let definition = FilePluginCatalog::new(root.path())
        .definition("demo")
        .unwrap();
    (root, sessions, definition)
}
fn manifest(url: &str) -> Value {
    json!({"name":"Demo","configuration":{
        "fields":{"endpoint":{"type":"url","label":"服务地址","default":url},
            "api_key":{"type":"secret","label":"访问凭据"}},
        "environment":{"remote":{"ANCHOR_TEST_CONNECTION_URL":"endpoint","PATH":"api_key"},
            "attachments":{"PATH":"api_key"}}},
        "mcpServers":{"remote":{"url":"${ANCHOR_TEST_CONNECTION_URL}/mcp","bearer_token_env_var":"PATH"},
            "attachments":{"command":"bin/tools","env_vars":["PATH"]}}})
}
fn update(definition: &PluginDefinition, revision: u64, fields: Value) -> ConfigurationUpdate {
    serde_json::from_value(json!({"authorization_digest":definition.authorization_digest,"revision":revision,"fields":fields})).unwrap()
}
fn configure_fields(
    root: &Path,
    sessions: &SessionStore,
    definition: &PluginDefinition,
    owner: &str,
    revision: u64,
    fields: Value,
) -> Result<Value, String> {
    configure(
        &root.join("state"),
        root,
        owner,
        &definition.id,
        update(definition, revision, fields),
        sessions,
    )
}

fn owner_snapshot(
    data: &Path,
    owner: &str,
    definition: &PluginDefinition,
) -> Result<ConfigurationSnapshot, String> {
    snapshot(
        data,
        &crate::credential_store::binding_owner(owner),
        definition,
    )
}

#[test]
fn shared_secret_is_atomic_owner_bound_private_and_rotatable_without_environment_fallback() {
    let (root, sessions, definition) = fixture();
    let data = root.path().join("state");
    let metadata = configuration(&data, root.path(), "owner-a", "demo").unwrap();
    assert_eq!(metadata["revision"], 0);
    assert_eq!(metadata["fields"][0]["state"], "missing");
    assert_eq!(metadata["fields"][0]["editable"], true);
    // PATH exists in the Host, yet an unconfigured managed field stays missing.
    assert!(std::env::var_os("PATH").is_some());
    assert!(owner_snapshot(&data, "owner-a", &definition).is_err());
    let result = configure_fields(
        root.path(),
        &sessions,
        &definition,
        "owner-a",
        0,
        json!({"api_key":{"action":"set","value":"private-first"}}),
    )
    .unwrap();
    assert_eq!(result["revision"], 1);
    assert!(!result.to_string().contains("private-first"));
    assert!(result["fields"][0].get("value").is_none());
    assert_eq!(result["fields"][1]["value"], "https://example.com");
    let private = owner_snapshot(&data, "owner-a", &definition).unwrap();
    assert_eq!(private.environments["remote"]["PATH"], "private-first");
    assert_eq!(private.environments["attachments"]["PATH"], "private-first");
    assert!(owner_snapshot(&data, "owner-b", &definition).is_err());
    assert!(
        configure_fields(
            root.path(),
            &sessions,
            &definition,
            "owner-a",
            0,
            json!({"api_key":{"action":"set","value":"lost-update"}})
        )
        .is_err()
    );
    configure_fields(
        root.path(),
        &sessions,
        &definition,
        "owner-a",
        1,
        json!({"api_key":{"action":"set","value":"private-second"}}),
    )
    .unwrap();
    assert_eq!(
        owner_snapshot(&data, "owner-a", &definition)
            .unwrap()
            .environments["remote"]["PATH"],
        "private-second"
    );
    configure_fields(
        root.path(),
        &sessions,
        &definition,
        "owner-a",
        2,
        json!({"api_key":{"action":"clear"}}),
    )
    .unwrap();
    assert!(owner_snapshot(&data, "owner-a", &definition).is_err());
    assert_eq!(
        configuration(&data, root.path(), "owner-a", "demo").unwrap()["fields"][0]["state"],
        "missing"
    );
}
#[test]
fn atomic_validation_rejects_invalid_endpoint_and_stale_authorization() {
    let (root, sessions, definition) = fixture();
    let data = root.path().join("state");
    assert!(configure_fields(root.path(), &sessions, &definition, "owner-a", 0, json!({"api_key":{"action":"set","value":"private-value"},"endpoint":{"action":"set","value":"https://user:password@example.com"}})).is_err());
    assert_eq!(
        configuration(&data, root.path(), "owner-a", "demo").unwrap()["revision"],
        0
    );
    configure_fields(
        root.path(),
        &sessions,
        &definition,
        "owner-a",
        0,
        json!({"api_key":{"action":"set","value":"private-value"}}),
    )
    .unwrap();
    fs::write(
        root.path().join("plugins/demo/plugin.json"),
        manifest("https://other.example").to_string(),
    )
    .unwrap();
    let metadata = configuration(&data, root.path(), "owner-a", "demo").unwrap();
    assert_eq!(metadata["fields"][0]["state"], "stale");
    assert!(
        configure_fields(
            root.path(),
            &sessions,
            &definition,
            "owner-a",
            1,
            json!({"api_key":{"action":"set","value":"wrong-authority"}})
        )
        .is_err()
    );
    let changed = FilePluginCatalog::new(root.path())
        .definition("demo")
        .unwrap();
    assert!(
        snapshot(
            &data,
            &crate::credential_store::binding_owner("owner-a"),
            &changed
        )
        .is_err()
    );
}
#[test]
fn legacy_api_and_pilot_share_logical_fact_without_duplicate_management() {
    use anchor_platform_session::CreateSession;
    let (root, sessions, definition) = fixture();
    let data = root.path().join("state");
    let session = sessions
        .create(
            "owner-a",
            CreateSession {
                id: Some("pilot-config".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let (turn, _) = sessions
        .create_turn("owner-a", &session.id, "test", Some("configure"))
        .unwrap();
    let request = crate::credential_store::request(
        &data,
        root.path(),
        &sessions,
        "owner-a",
        &session,
        &turn.id,
        "demo",
        "remote",
        "PATH",
        "configure",
    )
    .unwrap();
    assert_eq!(
        crate::credential_store::credentials(&data, root.path(), "owner-a", "demo").unwrap()["targets"],
        json!([])
    );
    configure_fields(
        root.path(),
        &sessions,
        &definition,
        "owner-a",
        0,
        json!({"api_key":{"action":"set","value":"private-shared"}}),
    )
    .unwrap();
    assert_eq!(
        crate::credential_store::get(&data, &sessions, "owner-a", &session, &request.id)
            .unwrap()
            .status,
        "configured"
    );
    assert!(
        crate::credential_store::submit(
            &data,
            root.path(),
            &sessions,
            "owner-a",
            &session,
            &request.id,
            "private-shared"
        )
        .is_ok()
    );
    assert!(
        crate::credential_store::submit(
            &data,
            root.path(),
            &sessions,
            "owner-a",
            &session,
            &request.id,
            "late-overwrite"
        )
        .is_err()
    );
    crate::credential_store::configure(
        &data,
        root.path(),
        "owner-a",
        "demo",
        "attachments",
        "PATH",
        &definition.authorization_digest,
        Some("private-rotated"),
        &sessions,
    )
    .unwrap();
    let values = crate::credential_store::environment_snapshot(
        &data,
        &crate::credential_store::binding_owner("owner-a"),
        &definition,
    )
    .unwrap();
    assert_eq!(values.revision, 2);
    assert_eq!(values.environments["remote"]["PATH"], "private-rotated");
    assert_eq!(
        values.environments["attachments"]["PATH"],
        "private-rotated"
    );
}
fn pending_logical_request_fixture() -> (
    tempfile::TempDir,
    SessionStore,
    PluginDefinition,
    anchor_platform_session::Session,
    crate::credential_store::CredentialRequest,
) {
    let (root, sessions, definition) = fixture();
    let data = root.path().join("state");
    configure_fields(
        root.path(),
        &sessions,
        &definition,
        "owner-a",
        0,
        json!({"api_key":{"action":"set","value":"original-fixture-key"}}),
    )
    .unwrap();
    let session = sessions
        .create(
            "owner-a",
            anchor_platform_session::CreateSession {
                id: Some("pending-logical-configuration".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let (turn, _) = sessions
        .create_turn("owner-a", &session.id, "test", Some("configure"))
        .unwrap();
    let request = crate::credential_store::request(
        &data,
        root.path(),
        &sessions,
        "owner-a",
        &session,
        &turn.id,
        "demo",
        "remote",
        "PATH",
        "replace the current connection key",
    )
    .unwrap();
    assert_eq!(request.status, "pending");
    assert_eq!(
        configuration(&data, root.path(), "owner-a", "demo").unwrap()["revision"],
        1
    );
    (root, sessions, definition, session, request)
}

fn assert_pending_logical_request_rejects_update(fields: Value, restore_pending_fact: bool) {
    let (root, sessions, definition, session, request) = pending_logical_request_fixture();
    let data = root.path().join("state");
    let request_scope = fs::read_dir(data.join("platform/credential-requests"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let request_path = request_scope.join(format!("{}.json", request.id));
    let pending_fact = fs::read(&request_path).unwrap();
    let saved =
        configure_fields(root.path(), &sessions, &definition, "owner-a", 1, fields).unwrap();
    assert_eq!(saved["revision"], 2);
    if restore_pending_fact {
        assert_eq!(
            crate::credential_store::get(&data, &sessions, "owner-a", &session, &request.id)
                .unwrap()
                .status,
            "configured"
        );
        fs::write(&request_path, &pending_fact).unwrap();
        assert_eq!(
            crate::credential_store::get(&data, &sessions, "owner-a", &session, &request.id)
                .unwrap()
                .status,
            "pending"
        );
    }
    let configuration_path = path(
        &data,
        &crate::credential_store::binding_owner("owner-a"),
        "demo",
    );
    let current_fact = fs::read(&configuration_path).unwrap();
    let current = configuration(&data, root.path(), "owner-a", "demo").unwrap();
    let submitted = crate::credential_store::submit(
        &data,
        root.path(),
        &sessions,
        "owner-a",
        &session,
        &request.id,
        "late-fixture-key",
    );
    assert!(
        submitted.is_err(),
        "a stale pending request must be rejected"
    );
    assert_eq!(
        configuration(&data, root.path(), "owner-a", "demo").unwrap(),
        current
    );
    assert!(
        fs::read(&configuration_path).unwrap() == current_fact,
        "rejected submission must preserve the latest private configuration"
    );
}

#[test]
fn pending_logical_request_rejects_url_change_without_mutating_current_configuration() {
    assert_pending_logical_request_rejects_update(
        json!({"endpoint":{"action":"set","value":"https://other.example"}}),
        false,
    );
}

#[test]
fn pending_logical_request_rejects_key_clear_without_mutating_current_configuration() {
    assert_pending_logical_request_rejects_update(json!({"api_key":{"action":"clear"}}), false);
}

#[test]
fn pending_logical_request_rejects_revision_change_even_when_pending_fact_is_restored() {
    assert_pending_logical_request_rejects_update(
        json!({"api_key":{"action":"set","value":"latest-fixture-key"}}),
        true,
    );
}

#[test]
fn pending_logical_request_accepts_submission_at_its_pinned_revision() {
    let (root, sessions, definition, session, request) = pending_logical_request_fixture();
    let data = root.path().join("state");
    let submitted = crate::credential_store::submit(
        &data,
        root.path(),
        &sessions,
        "owner-a",
        &session,
        &request.id,
        "replacement-fixture-key",
    )
    .unwrap();
    assert_eq!(submitted.status, "configured");
    let current = crate::credential_store::environment_snapshot(
        &data,
        &crate::credential_store::binding_owner("owner-a"),
        &definition,
    )
    .unwrap();
    assert_eq!(current.revision, 2);
    assert_eq!(
        current.environments["remote"]["PATH"],
        "replacement-fixture-key"
    );
    assert_eq!(
        current.environments["attachments"]["PATH"],
        "replacement-fixture-key"
    );
}

#[test]
fn pending_logical_request_can_be_requested_again_in_the_same_turn_after_url_change() {
    let (root, sessions, definition, session, previous) = pending_logical_request_fixture();
    let data = root.path().join("state");
    configure_fields(
        root.path(),
        &sessions,
        &definition,
        "owner-a",
        1,
        json!({"endpoint":{"action":"set","value":"https://other.example"}}),
    )
    .unwrap();
    let current = crate::credential_store::request(
        &data,
        root.path(),
        &sessions,
        "owner-a",
        &session,
        &previous.turn,
        "demo",
        "remote",
        "PATH",
        "configure the new connection",
    )
    .unwrap();
    assert_ne!(current.id, previous.id);
    assert_eq!(current.status, "pending");
    let submitted = crate::credential_store::submit(
        &data,
        root.path(),
        &sessions,
        "owner-a",
        &session,
        &current.id,
        "new-connection-fixture-key",
    )
    .unwrap();
    assert_eq!(submitted.status, "configured");
    let configured = crate::credential_store::environment_snapshot(
        &data,
        &crate::credential_store::binding_owner("owner-a"),
        &definition,
    )
    .unwrap();
    assert_eq!(configured.revision, 3);
    assert_eq!(
        configured.environments["remote"]["ANCHOR_TEST_CONNECTION_URL"],
        "https://other.example"
    );
    assert_eq!(
        configured.environments["remote"]["PATH"],
        "new-connection-fixture-key"
    );
    assert_eq!(
        configured.environments["attachments"]["PATH"],
        "new-connection-fixture-key"
    );
}

#[tokio::test]
async fn explicitly_external_binding_is_read_only_and_cannot_fallback_after_removal() {
    let _environment = crate::PROCESS_ENV_LOCK.lock().await;
    let mut environment = crate::test_support::Environment::default();
    let (root, sessions, _) = fixture();
    fs::rename(
        root.path().join("plugins/demo"),
        root.path().join("plugins/external-demo"),
    )
    .unwrap();
    let definition = FilePluginCatalog::new(root.path())
        .definition("external-demo")
        .unwrap();
    let data = root.path().join("state");
    let owner = crate::credential_store::binding_owner("owner-a");
    let fact = read(&data, &owner, &definition).unwrap();
    let managed = binding_digest(
        &owner,
        &definition,
        &resolve_fields(&definition, &fact).unwrap(),
    )
    .unwrap();
    environment.set(
        EXTERNAL_CONFIGURATION,
        r#"{"external-demo":{"api_key":"PATH"}}"#,
    );
    assert_ne!(
        managed,
        owner_snapshot(&data, "owner-a", &definition)
            .unwrap()
            .binding_digest
    );
    let metadata = configuration(&data, root.path(), "owner-a", "external-demo").unwrap();
    assert_eq!(metadata["fields"][0]["state"], "deployment");
    assert_eq!(metadata["fields"][0]["editable"], false);
    assert!(metadata["fields"][0].get("value").is_none());
    assert!(
        configure_fields(
            root.path(),
            &sessions,
            &definition,
            "owner-a",
            0,
            json!({"endpoint":{"action":"set","value":"https://other.example"}})
        )
        .is_err()
    );
    assert_eq!(
        configuration(&data, root.path(), "owner-a", "external-demo").unwrap()["revision"],
        0
    );
    assert!(
        configure_fields(
            root.path(),
            &sessions,
            &definition,
            "owner-a",
            0,
            json!({"api_key":{"action":"set","value":"cannot-override"}})
        )
        .is_err()
    );
    unsafe {
        std::env::set_var(
            EXTERNAL_CONFIGURATION,
            r#"{"external-demo":{"api_key":"OTHER_UNDECLARED_ENV"}}"#,
        );
    }
    assert!(configuration(&data, root.path(), "owner-a", "external-demo").is_err());
    unsafe {
        std::env::remove_var(EXTERNAL_CONFIGURATION);
    }
    assert!(owner_snapshot(&data, "owner-a", &definition).is_err());
}
#[test]
fn private_configuration_is_removed_with_plugin_and_uses_private_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let (root, sessions, definition) = fixture();
    let data = root.path().join("state");
    configure_fields(
        root.path(),
        &sessions,
        &definition,
        "owner-a",
        0,
        json!({"api_key":{"action":"set","value":"private-value"}}),
    )
    .unwrap();
    let file = path(
        &data,
        &crate::credential_store::binding_owner("owner-a"),
        "demo",
    );
    assert_eq!(
        fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    crate::credential_store::delete_plugin(&data, "demo").unwrap();
    assert!(!file.exists());
}

#[test]
fn clearing_default_endpoint_is_a_tombstone_and_never_restores_the_manifest_default() {
    let (root, sessions, definition) = fixture();
    let data = root.path().join("state");
    let result = configure_fields(
        root.path(),
        &sessions,
        &definition,
        "owner-a",
        0,
        json!({"api_key":{"action":"set","value":"private-key"},"endpoint":{"action":"clear"}}),
    )
    .unwrap();
    assert_eq!(result["revision"], 1);
    assert_eq!(result["fields"][1]["state"], "missing");
    assert!(result["fields"][1].get("value").is_none());
    assert!(owner_snapshot(&data, "owner-a", &definition).is_err());
    let other = configuration(&data, root.path(), "owner-b", "demo").unwrap();
    assert_eq!(other["fields"][1]["value"], "https://example.com");
}

#[test]
fn concurrent_configuration_writes_accept_exactly_one_revision() {
    let (root, sessions, definition) = fixture();
    let data = root.path().join("state");
    let barrier = std::sync::Barrier::new(2);
    let results = std::thread::scope(|scope| {
        let first = scope.spawn(|| {
            barrier.wait();
            configure_fields(
                root.path(),
                &sessions,
                &definition,
                "owner-a",
                0,
                json!({"api_key":{"action":"set","value":"first-private-key"}}),
            )
        });
        let second = scope.spawn(|| {
            barrier.wait();
            configure_fields(
                root.path(),
                &sessions,
                &definition,
                "owner-a",
                0,
                json!({"api_key":{"action":"set","value":"second-private-key"}}),
            )
        });
        [first.join().unwrap(), second.join().unwrap()]
    });
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        configuration(&data, root.path(), "owner-a", "demo").unwrap()["revision"],
        1
    );
    let snapshot = crate::credential_store::environment_snapshot(
        &data,
        &crate::credential_store::binding_owner("owner-a"),
        &definition,
    )
    .unwrap();
    assert_eq!(
        snapshot.environments["remote"]["PATH"],
        snapshot.environments["attachments"]["PATH"]
    );
}

#[test]
fn legacy_mcp_rotation_and_clear_invalidate_the_connection_test_version() {
    let (root, sessions, _) = fixture();
    fs::write(
        root.path().join("plugins/demo/plugin.json"),
        json!({"name":"Demo",
        "mcpServers":{"remote":{"url":"https://example.com/mcp","bearer_token_env_var":"PATH"}}})
        .to_string(),
    )
    .unwrap();
    let definition = FilePluginCatalog::new(root.path())
        .definition("demo")
        .unwrap();
    let data = root.path().join("state");
    let owner = crate::credential_store::binding_owner("owner-a");
    assert_eq!(
        crate::credential_store::environment_snapshot(&data, &owner, &definition)
            .unwrap()
            .revision,
        0
    );
    crate::credential_store::configure(
        &data,
        root.path(),
        "owner-a",
        "demo",
        "remote",
        "PATH",
        &definition.authorization_digest,
        Some("legacy-private-key"),
        &sessions,
    )
    .unwrap();
    assert_eq!(
        crate::credential_store::environment_snapshot(&data, &owner, &definition)
            .unwrap()
            .revision,
        1
    );
    crate::credential_store::configure(
        &data,
        root.path(),
        "owner-a",
        "demo",
        "remote",
        "PATH",
        &definition.authorization_digest,
        None,
        &sessions,
    )
    .unwrap();
    let snapshot =
        crate::credential_store::environment_snapshot(&data, &owner, &definition).unwrap();
    assert_eq!(snapshot.revision, 2);
    assert!(snapshot.environments["remote"].is_empty());
}

#[test]
fn recovery_binding_tracks_owner_endpoint_and_source_but_never_secret_values() {
    let (root, sessions, definition) = fixture();
    let data = root.path().join("state");
    configure_fields(
        root.path(),
        &sessions,
        &definition,
        "owner-a",
        0,
        json!({"api_key":{"action":"set","value":"first-private-key"}}),
    )
    .unwrap();
    let owner = crate::credential_store::binding_owner("owner-a");
    let original = snapshot(&data, &owner, &definition).unwrap().binding_digest;
    configure_fields(
        root.path(),
        &sessions,
        &definition,
        "owner-a",
        1,
        json!({"api_key":{"action":"set","value":"second-private-key"}}),
    )
    .unwrap();
    assert_eq!(
        original,
        snapshot(&data, &owner, &definition).unwrap().binding_digest
    );
    configure_fields(
        root.path(),
        &sessions,
        &definition,
        "owner-b",
        0,
        json!({"api_key":{"action":"set","value":"second-private-key"}}),
    )
    .unwrap();
    assert_ne!(
        original,
        owner_snapshot(&data, "owner-b", &definition)
            .unwrap()
            .binding_digest
    );
    configure_fields(
        root.path(),
        &sessions,
        &definition,
        "owner-a",
        2,
        json!({"endpoint":{"action":"set","value":"https://other.example"},
                "api_key":{"action":"set","value":"second-private-key"}}),
    )
    .unwrap();
    assert_ne!(
        original,
        snapshot(&data, &owner, &definition).unwrap().binding_digest
    );
}

#[test]
fn changing_connection_identity_clears_unconfirmed_keys_in_the_same_revision() {
    let (root, sessions, definition) = fixture();
    let data = root.path().join("state");
    configure_fields(
        root.path(),
        &sessions,
        &definition,
        "owner-a",
        0,
        json!({"api_key":{"action":"set","value":"key-for-original-service"}}),
    )
    .unwrap();
    let changed = configure_fields(
        root.path(),
        &sessions,
        &definition,
        "owner-a",
        1,
        json!({"endpoint":{"action":"set","value":"https://other.example"}}),
    )
    .unwrap();
    assert_eq!(changed["revision"], 2);
    assert_eq!(changed["fields"][0]["state"], "missing");
    assert_eq!(changed["fields"][1]["value"], "https://other.example");
    assert!(owner_snapshot(&data, "owner-a", &definition).is_err());
    configure_fields(
        root.path(),
        &sessions,
        &definition,
        "owner-a",
        2,
        json!({"api_key":{"action":"set","value":"key-for-new-service"}}),
    )
    .unwrap();
    let same = configure_fields(
        root.path(),
        &sessions,
        &definition,
        "owner-a",
        3,
        json!({"endpoint":{"action":"set","value":"https://other.example"}}),
    )
    .unwrap();
    assert_eq!(same["fields"][0]["state"], "configured");
    configure_fields(
        root.path(),
        &sessions,
        &definition,
        "owner-a",
        4,
        json!({"endpoint":{"action":"set","value":"https://third.example"},
            "api_key":{"action":"set","value":"explicit-key-for-third-service"}}),
    )
    .unwrap();
    let snapshot = owner_snapshot(&data, "owner-a", &definition).unwrap();
    assert_eq!(snapshot.revision, 5);
    assert_eq!(
        snapshot.environments["remote"]["PATH"],
        "explicit-key-for-third-service"
    );
    assert_eq!(
        snapshot.environments["remote"]["ANCHOR_TEST_CONNECTION_URL"],
        "https://third.example"
    );
}

#[test]
fn connection_changes_cannot_carry_unmapped_legacy_credentials_to_a_new_destination() {
    let (root, sessions, _) = fixture();
    let mut mixed = manifest("https://example.com");
    mixed["mcpServers"]["attachments"]["env_vars"] =
        json!(["PATH", "ANCHOR_LEGACY_CONNECTION_KEY"]);
    fs::write(
        root.path().join("plugins/demo/plugin.json"),
        mixed.to_string(),
    )
    .unwrap();
    let definition = FilePluginCatalog::new(root.path())
        .definition("demo")
        .unwrap();
    let data = root.path().join("state");
    configure_fields(
        root.path(),
        &sessions,
        &definition,
        "owner-a",
        0,
        json!({"api_key":{"action":"set","value":"managed-private-key"}}),
    )
    .unwrap();
    crate::credential_store::configure(
        &data,
        root.path(),
        "owner-a",
        "demo",
        "attachments",
        "ANCHOR_LEGACY_CONNECTION_KEY",
        &definition.authorization_digest,
        Some("legacy-private-key"),
        &sessions,
    )
    .unwrap();
    assert!(
        configure_fields(
            root.path(),
            &sessions,
            &definition,
            "owner-a",
            2,
            json!({"endpoint":{"action":"set","value":"https://other.example"}})
        )
        .is_err()
    );
    assert_eq!(
        configuration(&data, root.path(), "owner-a", "demo").unwrap()["revision"],
        2
    );
    crate::credential_store::configure(
        &data,
        root.path(),
        "owner-a",
        "demo",
        "attachments",
        "ANCHOR_LEGACY_CONNECTION_KEY",
        &definition.authorization_digest,
        None,
        &sessions,
    )
    .unwrap();
    assert!(
        configure_fields(
            root.path(),
            &sessions,
            &definition,
            "owner-a",
            3,
            json!({"endpoint":{"action":"set","value":"https://other.example"}})
        )
        .is_ok()
    );
}

#[test]
fn root_origin_slash_is_normalized_without_clearing_keys_or_changing_recovery_identity() {
    let (root, sessions, definition) = fixture();
    let data = root.path().join("state");
    configure_fields(
        root.path(),
        &sessions,
        &definition,
        "owner-a",
        0,
        json!({"api_key":{"action":"set","value":"private-same-service"}}),
    )
    .unwrap();
    let owner = crate::credential_store::binding_owner("owner-a");
    let before = snapshot(&data, &owner, &definition).unwrap().binding_digest;
    let changed = configure_fields(
        root.path(),
        &sessions,
        &definition,
        "owner-a",
        1,
        json!({"endpoint":{"action":"set","value":"https://example.com/"}}),
    )
    .unwrap();
    assert_eq!(changed["fields"][0]["state"], "configured");
    assert_eq!(changed["fields"][1]["value"], "https://example.com");
    let after = snapshot(&data, &owner, &definition).unwrap();
    assert_eq!(before, after.binding_digest);
    assert_eq!(
        after.environments["remote"]["ANCHOR_TEST_CONNECTION_URL"],
        "https://example.com"
    );
    let stored = read(&data, &owner, &definition).unwrap();
    assert_eq!(
        stored.fields["endpoint"].value.as_deref(),
        Some("https://example.com")
    );
    for unchanged in [
        "https://example.com/path/",
        "https://example.com//",
        "https://example.com/path/../",
        "https://example.com/?query=value",
    ] {
        assert_eq!(
            normalized_value(&definition.configuration.fields["endpoint"], unchanged),
            unchanged
        );
    }
    assert_eq!(
        normalized_value(
            &definition.configuration.fields["endpoint"],
            "http://127.0.0.1:8077/"
        ),
        "http://127.0.0.1:8077"
    );
    assert_eq!(
        normalized_value(
            &definition.configuration.fields["api_key"],
            "secret-ending-in/"
        ),
        "secret-ending-in/"
    );
}
