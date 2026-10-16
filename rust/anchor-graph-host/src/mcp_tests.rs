use super::*;
use crate::{FilePluginCatalog, plugin_test_support::catalog_with_dependency_plugin};
use std::fs;

#[test]
fn the_view_token_is_kept_verbatim_and_requires_a_declaration() {
    let (_tmp, catalog) = catalog_with_dependency_plugin();
    // The declaration names exactly one view...
    let view = catalog.view_path("probe").unwrap().unwrap();
    assert!(!view.ends_with("bin/python"));
    // ...but the token inside the manifest is a *deployment* path: the
    // catalog keeps it as written, because only the host that grants the view
    // may name it. The catalog root a Plugin happens to be read through must
    // never become part of the command.
    for resolve_env in [true, false] {
        let servers = catalog.mcp_servers("probe", resolve_env).unwrap();
        assert_eq!(
            servers[0].config.get("command").and_then(Value::as_str),
            Some("${ANCHOR_PLUGIN_VIEW}/bin/python")
        );
        assert_eq!(
            servers[0].config.get("args").unwrap(),
            &serde_json::json!(["server.py"])
        );
    }

    // Using the token without declaring anything cannot point anywhere.
    let empty = catalog_with_dependency_plugin().0;
    fs::create_dir_all(empty.path().join("plugins/empty")).unwrap();
    fs::write(
        empty.path().join("plugins/empty/plugin.json"),
        r#"{"name":"Empty","mcpServers":{"probe":{"command":"${ANCHOR_PLUGIN_VIEW}/bin/python"}}}"#,
    )
    .unwrap();
    let error = FilePluginCatalog::new(empty.path())
        .mcp_servers("empty", true)
        .unwrap_err();
    assert!(error.to_string().contains("dependencies.python"), "{error}");
}

/// The strict key check also guards the path that reads MCP declarations, so
/// no caller can act on a manifest `definition` would have rejected.

#[test]
fn a_mistyped_manifest_field_is_refused_on_the_mcp_read_path_too() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("plugins/typo");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("plugin.json"),
        r#"{"name":"Typo","dependencie":{},"mcpServers":{"probe":{"command":"run"}}}"#,
    )
    .unwrap();
    let error = FilePluginCatalog::new(tmp.path())
        .mcp_servers("typo", true)
        .unwrap_err();
    assert!(
        error.to_string().contains("unsupported manifest field"),
        "{error}"
    );
    // Without the declaration the token still cannot point anywhere.
    let dir = tmp.path().join("plugins/orphan");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("plugin.json"),
        r#"{"name":"Orphan","mcpServers":{"probe":{"command":"${ANCHOR_PLUGIN_VIEW}/bin/python"}}}"#,
    )
    .unwrap();
    let error = FilePluginCatalog::new(tmp.path())
        .mcp_servers("orphan", true)
        .unwrap_err();
    assert!(error.to_string().contains("dependencies.python"), "{error}");
}

/// The old declaration is refused by name and points at the replacement.
#[test]
fn a_server_environment_is_refused_and_points_at_the_dependency_declaration() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("plugins/demo");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("plugin.json"),
        r#"{"name":"Demo","mcpServers":{"probe":{"command":"run","anchor":{"environment":"research"}}}}"#,
    )
    .unwrap();
    let error = FilePluginCatalog::new(tmp.path())
        .definition("demo")
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("`anchor.environment` no longer selects a managed environment")
            && error.to_string().contains("dependencies"),
        "{error}"
    );
}

#[test]
fn mcp_environment_and_paths_are_validated_before_resolution() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("plugins/invalid");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("plugin.json"),
        r#"{"name":"Invalid","mcpServers":{"bad":{"command":"echo","env_vars":["bad-name"]}}}"#,
    )
    .unwrap();
    let catalog = FilePluginCatalog::new(tmp.path());
    assert!(catalog.mcp_servers("invalid", true).is_err());

    fs::write(
        dir.join("plugin.json"),
        r#"{"name":"Invalid","mcpServers":{"bad":{"url":"https://example.test","headers":{"Authorization":"x"},"bearer_token_env_var":"MISSING_ANCHOR_TOKEN"}}}"#,
    )
    .unwrap();
    assert!(catalog.mcp_servers("invalid", true).is_err());

    fs::write(
        dir.join("plugin.json"),
        r#"{"name":"Invalid","mcpServers":{"oauth":{"url":"https://example.test","auth":"oauth","headers":{"authorization":"static-secret"}}}}"#,
    )
    .unwrap();
    assert!(catalog.mcp_servers("invalid", true).is_err());

    fs::write(
        dir.join("plugin.json"),
        r#"{"name":"Invalid","mcpServers":{"oauth":{"url":"https://example.test","oauth_client_id":"client"}}}"#,
    )
    .unwrap();
    assert!(catalog.mcp_servers("invalid", true).is_err());
}

#[test]
fn host_scoped_mcp_credentials_do_not_change_the_declaration_or_process_environment() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("plugins/scoped");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("plugin.json"), serde_json::json!({"name":"Scoped","mcpServers":{
        "stdio":{"command":"echo","env":{"DIRECT":"${ANCHOR_SCOPED_SECRET}","DEFAULT":"${ANCHOR_SCOPED_MISSING:-fallback}"},"env_vars":["ANCHOR_SCOPED_SECRET"],"optional_env_vars":["ANCHOR_SCOPED_OPTIONAL"]},
        "http":{"url":"https://example.test/mcp","bearer_token_env_var":"ANCHOR_SCOPED_SECRET","env_http_headers":{"X-Token":"ANCHOR_SCOPED_OPTIONAL"}}
    }}).to_string()).unwrap();
    let catalog = FilePluginCatalog::new(tmp.path());
    let declaration = catalog.definition("scoped").unwrap();
    let before = declaration.mcp_servers.clone();
    let lookup = |key: &str| match key {
        "ANCHOR_SCOPED_SECRET" => Some("owner-a-token".into()),
        "ANCHOR_SCOPED_OPTIONAL" => Some("owner-a-optional".into()),
        _ => None,
    };
    let resolved = catalog
        .resolve_mcp_servers_with(&declaration, &lookup)
        .unwrap();
    let stdio = resolved
        .iter()
        .find(|server| server.name == "stdio")
        .unwrap();
    assert_eq!(stdio.config["env"]["DIRECT"], "owner-a-token");
    assert_eq!(stdio.config["env"]["DEFAULT"], "fallback");
    assert_eq!(stdio.config["env"]["ANCHOR_SCOPED_SECRET"], "owner-a-token");
    let http = resolved
        .iter()
        .find(|server| server.name == "http")
        .unwrap();
    assert_eq!(
        http.config["headers"]["Authorization"],
        "Bearer owner-a-token"
    );
    assert_eq!(http.config["headers"]["X-Token"], "owner-a-optional");
    assert_eq!(declaration.mcp_servers, before);
    assert!(
        catalog
            .resolve_mcp_servers_with(&declaration, &|_| None)
            .is_err()
    );
    let other = catalog
        .resolve_mcp_servers_with(&declaration, &|key| {
            lookup(key).map(|_| "owner-b-token".into())
        })
        .unwrap();
    assert_eq!(
        other
            .iter()
            .find(|server| server.name == "http")
            .unwrap()
            .config["headers"]["Authorization"],
        "Bearer owner-b-token"
    );
}
