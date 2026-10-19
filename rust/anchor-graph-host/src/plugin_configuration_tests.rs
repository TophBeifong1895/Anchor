use super::*;
use crate::FilePluginCatalog;
use serde_json::json;
use std::fs;

fn configured_manifest() -> Value {
    json!({
        "name": "Configured",
        "configuration": {
            "fields": {
                "base_url": {"type":"url", "default":"https://example.test"},
                "api_key": {"type":"secret", "label":"API key"}
            },
            "environment": {
                "remote": {"SERVICE_URL":"base_url", "SERVICE_TOKEN":"api_key"},
                "local": {"LOCAL_TOKEN":"api_key"}
            }
        },
        "mcpServers": {
            "remote": {"url":"${SERVICE_URL}/mcp", "bearer_token_env_var":"SERVICE_TOKEN"},
            "local": {"command":"fixture", "env_vars":["LOCAL_TOKEN"]}
        }
    })
}

fn catalog(manifest: &Value) -> (tempfile::TempDir, FilePluginCatalog) {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("plugins/configured");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("plugin.json"),
        serde_json::to_vec(manifest).unwrap(),
    )
    .unwrap();
    let catalog = FilePluginCatalog::new(temp.path());
    (temp, catalog)
}

#[test]
fn named_secret_is_explicitly_shared_without_changing_transport_environment_names() {
    let (_temp, catalog) = catalog(&configured_manifest());
    let definition = catalog.definition("configured").unwrap();
    let configuration = &definition.configuration;
    assert_eq!(configuration.fields["base_url"].label, "base_url");
    assert!(configuration.fields["api_key"].is_secret());
    assert!(configuration.fields["api_key"].required);
    assert_eq!(
        configuration.environment_for("remote").unwrap()["SERVICE_TOKEN"],
        "api_key"
    );
    assert_eq!(
        configuration.environment_for("local").unwrap()["LOCAL_TOKEN"],
        "api_key"
    );
    let resolved = catalog
        .resolve_mcp_servers_with(&definition, &|key| match key {
            "SERVICE_URL" => Some("https://configured.test".into()),
            "SERVICE_TOKEN" | "LOCAL_TOKEN" => Some("private-fixture-value".into()),
            _ => None,
        })
        .unwrap();
    let remote = resolved
        .iter()
        .find(|server| server.name == "remote")
        .unwrap();
    let local = resolved
        .iter()
        .find(|server| server.name == "local")
        .unwrap();
    assert_eq!(remote.config["url"], "https://configured.test/mcp");
    assert_eq!(
        remote.config["headers"]["Authorization"],
        "Bearer private-fixture-value"
    );
    assert_eq!(local.config["env"]["LOCAL_TOKEN"], "private-fixture-value");
    assert!(!format!("{definition:?}").contains("private-fixture-value"));
    assert!(!format!("{resolved:?}").contains("private-fixture-value"));
}

#[test]
fn malformed_or_ambiguous_configuration_declarations_are_rejected() {
    let mut cases = Vec::new();
    let mut manifest = configured_manifest();
    manifest["configuration"]["fields"]["api_key"]["default"] = json!("do-not-package");
    cases.push(manifest);
    let mut manifest = configured_manifest();
    manifest["configuration"]["environment"]["remote"]["SERVICE_TOKEN"] = json!("undeclared");
    cases.push(manifest);
    let mut manifest = configured_manifest();
    manifest["configuration"]["environment"]["unknown"] = json!({"TOKEN":"api_key"});
    cases.push(manifest);
    let mut manifest = configured_manifest();
    manifest["configuration"]["environment"]["remote"]["NOT_CONSUMED"] = json!("api_key");
    cases.push(manifest);
    let mut manifest = configured_manifest();
    manifest["configuration"]["fields"]["api_key"]["typo"] = json!(true);
    cases.push(manifest);
    let mut manifest = configured_manifest();
    manifest["configuration"]["fields"]["unused"] = json!({"type":"string"});
    cases.push(manifest);
    for manifest in cases {
        let (_temp, catalog) = catalog(&manifest);
        assert!(catalog.definition("configured").is_err());
        assert!(catalog.mcp_servers("configured", false).is_err());
    }
}

#[test]
fn actual_endpoint_is_validated_after_configuration_expansion() {
    let (_temp, catalog) = catalog(&configured_manifest());
    let definition = catalog.definition("configured").unwrap();
    for endpoint in [
        "file:///etc",
        "https://user:password@example.test",
        "https://example.test/#fragment",
        "https://bad host.test",
    ] {
        let result = catalog.resolve_mcp_servers_with(&definition, &|key| {
            Some(
                if key == "SERVICE_URL" {
                    endpoint
                } else {
                    "private-fixture-value"
                }
                .into(),
            )
        });
        let error = result.unwrap_err().to_string();
        assert!(error.contains("resolved url"), "{error}");
        assert!(!error.contains("private-fixture-value"));
        assert!(!error.contains(endpoint));
    }
}

#[test]
fn configured_plugins_cannot_leave_connection_templates_in_legacy_credentials() {
    for target in ["remote", "local"] {
        let mut manifest = configured_manifest();
        if target == "remote" {
            manifest["configuration"]["fields"]
                .as_object_mut()
                .unwrap()
                .remove("base_url");
            manifest["configuration"]["environment"]["remote"]
                .as_object_mut()
                .unwrap()
                .remove("SERVICE_URL");
        } else {
            manifest["mcpServers"]["local"]["args"] = json!(["--endpoint", "${OTHER_URL}"]);
        }
        let (_temp, catalog) = catalog(&manifest);
        let error = catalog.definition("configured").unwrap_err().to_string();
        assert!(
            error.contains("explicit non-secret configuration bindings"),
            "{error}"
        );
        assert!(catalog.mcp_servers("configured", false).is_err());
    }
}

#[test]
fn configuration_authority_tracks_bindings_and_defaults_but_not_labels() {
    let manifest = configured_manifest();
    let (_temp, catalog) = catalog(&manifest);
    let before = catalog
        .definition("configured")
        .unwrap()
        .authorization_digest;
    let mut renamed = manifest.clone();
    renamed["configuration"]["fields"]["api_key"]["label"] = json!("New label");
    let (_temp, catalog) = self::catalog(&renamed);
    assert_eq!(
        before,
        catalog
            .definition("configured")
            .unwrap()
            .authorization_digest
    );
    let mut changed = manifest;
    changed["configuration"]["fields"]["base_url"]["default"] = json!("https://other.test");
    let (_temp, catalog) = self::catalog(&changed);
    assert_ne!(
        before,
        catalog
            .definition("configured")
            .unwrap()
            .authorization_digest
    );
}

#[test]
fn url_values_reject_credentials_fragments_and_non_http_schemes() {
    let field = PluginConfigurationField {
        kind: PluginConfigurationFieldType::Url,
        label: "Endpoint".into(),
        required: true,
        default: None,
    };
    for invalid in [
        "",
        "file:///etc",
        "https://@example.test",
        "https://user:pass@example.test",
        "https://example.test/#x",
        "https://example.test\n",
        "\u{0008}https://example.test",
        "https://example.test\\other.test",
    ] {
        assert!(field.validate_value(invalid).is_err());
    }
    assert!(
        field
            .validate_value("https://example.test:8443/path")
            .is_ok()
    );
    assert!(field.validate_value("http://127.0.0.1:8080").is_ok());
}

#[test]
fn plugins_without_configuration_retain_their_original_authority_identity() {
    let empty = PluginConfiguration::default();
    assert_eq!(empty.authorization_digest("legacy".into()), "legacy");
    let (_temp, catalog) =
        catalog(&json!({"name":"Legacy", "mcpServers":{"remote":{"url":"https://example.test"}}}));
    assert!(
        catalog
            .definition("configured")
            .unwrap()
            .configuration
            .is_empty()
    );
}

#[test]
fn secrets_cannot_expand_into_observable_urls_arguments_or_oauth_metadata() {
    for (target, field, value) in [
        (
            "remote",
            "url",
            json!("https://example.test/${SERVICE_TOKEN}"),
        ),
        (
            "remote",
            "oauth_resource",
            json!("https://example.test/${SERVICE_TOKEN}"),
        ),
        ("remote", "oauth_client_id", json!("${SERVICE_TOKEN}")),
        ("local", "command", json!("${LOCAL_TOKEN}")),
        ("local", "args", json!(["--token", "${LOCAL_TOKEN}"])),
        ("local", "cwd", json!("${LOCAL_TOKEN}")),
    ] {
        let mut manifest = configured_manifest();
        manifest["mcpServers"][target][field] = value;
        // Keep every configuration field consumed so the secret-specific
        // diagnostic proves why this declaration is refused.
        if field == "url" {
            manifest["mcpServers"]["remote"]["headers"] = json!({"X-Base-Url":"${SERVICE_URL}"});
        }
        if field.starts_with("oauth_") {
            manifest["mcpServers"]["remote"]["auth"] = json!("oauth");
            manifest["mcpServers"]["remote"]
                .as_object_mut()
                .unwrap()
                .remove("bearer_token_env_var");
        }
        let (_temp, catalog) = catalog(&manifest);
        let error = catalog.definition("configured").unwrap_err().to_string();
        assert!(
            error.contains("secret configuration fields"),
            "{field}: {error}"
        );
    }
}

#[test]
fn secret_templates_are_refused_in_plain_environment_and_http_header_values() {
    for (target, field, value) in [
        ("remote", "headers", json!({"X-Token":"${SERVICE_TOKEN}"})),
        (
            "remote",
            "http_headers",
            json!({"X-Token":"${SERVICE_TOKEN}"}),
        ),
        ("local", "env", json!({"TOKEN":"${LOCAL_TOKEN}"})),
    ] {
        let mut manifest = configured_manifest();
        manifest["mcpServers"][target][field] = value;
        let (_temp, catalog) = catalog(&manifest);
        let error = catalog.definition("configured").unwrap_err().to_string();
        assert!(error.contains("secret configuration fields"), "{error}");
    }
}

#[test]
fn credential_selectors_cannot_be_changed_by_configuration_templates() {
    for (target, field, value) in [
        ("remote", "bearer_token_env_var", json!("${SERVICE_TOKEN}")),
        (
            "remote",
            "env_http_headers",
            json!({"X-Token":"${SERVICE_TOKEN}"}),
        ),
        ("local", "env_vars", json!(["${LOCAL_TOKEN}"])),
        ("local", "optional_env_vars", json!(["${LOCAL_TOKEN}"])),
    ] {
        let mut manifest = configured_manifest();
        manifest["mcpServers"][target][field] = value;
        let (_temp, catalog) = catalog(&manifest);
        assert!(catalog.definition("configured").is_err(), "{field}");
    }
}

#[test]
fn expanded_headers_and_process_arguments_are_validated_without_echoing_values() {
    let mut manifest = configured_manifest();
    manifest["mcpServers"]["local"]["args"] = json!(["${FIXTURE_ARGUMENT}"]);
    manifest["configuration"]["fields"]["argument"] = json!({"type":"string"});
    manifest["configuration"]["environment"]["local"]["FIXTURE_ARGUMENT"] = json!("argument");
    let (_temp, catalog) = catalog(&manifest);
    let definition = catalog.definition("configured").unwrap();
    for (key, value) in [
        ("SERVICE_TOKEN", "private-fixture-value\r\nX-Injected: yes"),
        ("FIXTURE_ARGUMENT", "private-fixture-value\0"),
    ] {
        let error = catalog
            .resolve_mcp_servers_with(&definition, &|name| {
                Some(
                    if name == key {
                        value
                    } else if name == "SERVICE_URL" {
                        "https://configured.test"
                    } else {
                        "valid-fixture-value"
                    }
                    .into(),
                )
            })
            .unwrap_err()
            .to_string();
        assert!(!error.contains("private-fixture-value"), "{error}");
    }
}

#[test]
fn http_header_alias_and_environment_headers_are_validated_after_resolution() {
    let mut manifest = configured_manifest();
    manifest["mcpServers"]["remote"]["http_headers"] = json!({"X-Literal":"fixture"});
    manifest["mcpServers"]["remote"]["env_http_headers"] = json!({"X-Token":"SERVICE_TOKEN"});
    let (_temp, catalog) = catalog(&manifest);
    let definition = catalog.definition("configured").unwrap();
    let valid = |key: &str| {
        Some(
            if key == "SERVICE_URL" {
                "https://configured.test"
            } else {
                "valid-fixture-value"
            }
            .into(),
        )
    };
    let resolved = catalog
        .resolve_mcp_servers_with(&definition, &valid)
        .unwrap();
    let headers = &resolved
        .iter()
        .find(|server| server.name == "remote")
        .unwrap()
        .config["headers"];
    assert_eq!(headers["X-Literal"], "fixture");
    assert_eq!(headers["X-Token"], "valid-fixture-value");
    assert!(
        catalog
            .resolve_mcp_servers_with(&definition, &|key| {
                if key == "SERVICE_TOKEN" {
                    Some("private-fixture-value\r\nInjected: yes".into())
                } else {
                    valid(key)
                }
            })
            .is_err()
    );
}

#[test]
fn docmost_servers_use_the_same_declared_key_and_service_address() {
    let mut manifest: Value =
        serde_json::from_str(include_str!("../../../plugins/docmost/plugin.json")).unwrap();
    // This fixture exercises the actual configuration/MCP declarations; the
    // package test independently verifies the bundled Skill and executable.
    manifest.as_object_mut().unwrap().remove("skills");
    let (_temp, catalog) = catalog(&manifest);
    let definition = catalog.definition("configured").unwrap();
    assert!(definition.configuration.fields["base_url"].required);
    assert!(
        definition.configuration.fields["base_url"]
            .default
            .is_none()
    );
    for target in ["docmost", "attachments"] {
        assert_eq!(
            definition.configuration.environment_for(target).unwrap()["DOCMOST_API_KEY"],
            "api_key"
        );
        assert_eq!(
            definition.configuration.environment_for(target).unwrap()["DOCMOST_BASE_URL"],
            "base_url"
        );
    }
    let resolved = catalog
        .resolve_mcp_servers_with(&definition, &|key| match key {
            "DOCMOST_BASE_URL" => Some("https://docmost.fixture.test".into()),
            "DOCMOST_API_KEY" => Some("private-fixture-value".into()),
            _ => None,
        })
        .unwrap();
    let remote = &resolved
        .iter()
        .find(|server| server.name == "docmost")
        .unwrap()
        .config;
    let attachments = &resolved
        .iter()
        .find(|server| server.name == "attachments")
        .unwrap()
        .config;
    assert_eq!(remote["url"], "https://docmost.fixture.test/mcp");
    assert_eq!(
        remote["headers"]["Authorization"],
        "Bearer private-fixture-value"
    );
    assert_eq!(
        attachments["args"],
        json!([
            "--endpoint",
            "https://docmost.fixture.test/api/files/upload"
        ])
    );
    assert_eq!(
        attachments["env"]["DOCMOST_API_KEY"],
        "private-fixture-value"
    );
    assert!(
        !attachments["args"]
            .to_string()
            .contains("private-fixture-value")
    );
}

#[test]
fn configuration_authority_tracks_consumer_mapping_without_transport_changes() {
    let mut manifest = configured_manifest();
    manifest["configuration"]["fields"]["extra_key"] = json!({"type":"secret"});
    manifest["configuration"]["environment"]["local"]["EXTRA_TOKEN"] = json!("extra_key");
    manifest["mcpServers"]["local"]["env_vars"] = json!(["LOCAL_TOKEN", "EXTRA_TOKEN"]);
    let (_temp, before_catalog) = catalog(&manifest);
    let before = before_catalog.definition("configured").unwrap();
    manifest["configuration"]["environment"]["local"]["LOCAL_TOKEN"] = json!("extra_key");
    let (_temp, after_catalog) = catalog(&manifest);
    let after = after_catalog.definition("configured").unwrap();
    assert_eq!(before.mcp_servers, after.mcp_servers);
    assert_ne!(before.authorization_digest, after.authorization_digest);

    let before = after;
    manifest["configuration"]["fields"]["api_key"]["required"] = json!(false);
    let (_temp, after_catalog) = catalog(&manifest);
    let after = after_catalog.definition("configured").unwrap();
    assert_ne!(before.authorization_digest, after.authorization_digest);
}

#[test]
fn configuration_cannot_bind_host_reserved_path_names_even_through_credential_selectors() {
    for reserved in [
        "PLUGIN_ROOT",
        "CODEX_PLUGIN_ROOT",
        "CLAUDE_PLUGIN_ROOT",
        "ANCHOR_PLUGIN_VIEW",
    ] {
        let mut manifest = configured_manifest();
        manifest["mcpServers"]["local"]["env_vars"] = json!(["LOCAL_TOKEN", reserved]);
        manifest["configuration"]["environment"]["local"][reserved] = json!("api_key");
        let (_temp, catalog) = catalog(&manifest);
        let error = catalog.definition("configured").unwrap_err().to_string();
        assert!(error.contains("Host-reserved"), "{reserved}: {error}");
        assert!(catalog.mcp_servers("configured", false).is_err());
    }
}
