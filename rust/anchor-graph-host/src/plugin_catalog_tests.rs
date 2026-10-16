use super::*;
use crate::plugin_test_support::{catalog_with_dependency_plugin, catalog_with_plugin};
use crate::views_root;
use std::fs;

#[test]
fn a_network_requirement_is_strict_and_part_of_the_approved_plugin_code() {
    let (tmp, catalog) = catalog_with_dependency_plugin();
    let path = tmp.path().join("plugins/probe/plugin.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let original = catalog.definition("probe").unwrap();
    assert!(!original.network);
    manifest["network"] = Value::Bool(true);
    fs::write(&path, manifest.to_string()).unwrap();
    let networked = catalog.definition("probe").unwrap();
    assert!(networked.network);
    assert_ne!(original.digest, networked.digest);
    manifest["network"] = "true".into();
    fs::write(path, manifest.to_string()).unwrap();
    assert!(
        catalog
            .definition("probe")
            .unwrap_err()
            .to_string()
            .contains("network must be a boolean")
    );
    assert!(
        catalog
            .mcp_servers("probe", false)
            .unwrap_err()
            .to_string()
            .contains("network must be a boolean")
    );
}

#[test]
fn a_view_path_is_derived_from_the_declaration_both_halves_agree_on() {
    let (tmp, catalog) = catalog_with_dependency_plugin();
    let definition = catalog.definition("probe").unwrap();
    assert_eq!(definition.dependencies.python, vec!["idna==3.6"]);
    let view = catalog.view_path("probe").unwrap().expect("a view path");
    let root = tmp.path().canonicalize().unwrap();
    assert_eq!(
        view,
        plugin_view_path(
            &root,
            "probe",
            &definition.dependencies.declaration_digest()
        )
    );
    assert!(view.starts_with(views_root(&root)));
    // The same declaration in a different order is the same view; a changed
    // one is not.
    let dir = tmp.path().join("plugins/other");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("plugin.json"),
        r#"{"name":"Other","dependencies":{"python":["idna==3.6"]}}"#,
    )
    .unwrap();
    // The same declaration is the same store content, but a view belongs to
    // one Plugin: `other` gets its own directory under the same digest.
    assert_eq!(
        view.file_name(),
        catalog.view_path("other").unwrap().unwrap().file_name()
    );
    assert_ne!(catalog.view_path("other").unwrap(), Some(view));

    // Declaring nothing means no view at all.
    let plain = tmp.path().join("plugins/plain");
    fs::create_dir_all(&plain).unwrap();
    fs::write(plain.join("plugin.json"), r#"{"name":"Plain"}"#).unwrap();
    assert_eq!(catalog.view_path("plain").unwrap(), None);
}

/// The digest covers code only: a declared mutable path is the Plugin's own
/// state, and what it contains is never part of "which code is this".
#[test]
fn writing_into_a_declared_mutable_path_does_not_change_the_digest() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("plugins/probe");
    fs::create_dir_all(dir.join("skills")).unwrap();
    fs::write(
        dir.join("plugin.json"),
        r#"{"name":"Probe","mutable":["data/","cache.db"],"skills":"skills/"}"#,
    )
    .unwrap();
    fs::write(dir.join("skills/SKILL.md"), "code").unwrap();
    let catalog = FilePluginCatalog::new(tmp.path());
    let before = catalog.definition("probe").unwrap();
    assert_eq!(before.mutable.paths(), ["data/", "cache.db"]);
    // The mutable paths are not resources either.
    assert_eq!(before.digest.len(), 64);
    assert!(before.mutable.covers("data/state.json"));
    assert!(before.mutable.covers("cache.db"));
    assert!(!before.mutable.covers("cache.db.bak"));
    assert!(!before.mutable.covers("skills/SKILL.md"));
    // The Plugin writes its state, and the code digest does not move. Its own
    // state may even contain what the platform refuses in *code*: a symlink and
    // a credentials file are the Plugin's business inside a declared mutable
    // path (invariant 9).
    fs::create_dir_all(dir.join("data")).unwrap();
    fs::write(dir.join("data/state.json"), "state").unwrap();
    std::os::unix::fs::symlink("/etc/passwd", dir.join("data/link")).unwrap();
    fs::write(dir.join("data/.env"), "TOKEN=plugin-own").unwrap();
    fs::write(dir.join("cache.db"), "cache").unwrap();
    let after = catalog.definition("probe").unwrap();
    assert_eq!(after.digest, before.digest);
    assert_eq!(after.resources, before.resources);
    let binding = catalog.resolve(&["probe".into()]).unwrap().remove(0);
    assert!(
        !binding
            .resources
            .iter()
            .any(|path| path.starts_with("data/"))
    );
    assert!(!binding.resources.contains(&"cache.db".to_owned()));
    // A code change does move it.
    fs::write(dir.join("skills/SKILL.md"), "changed").unwrap();
    assert_ne!(catalog.definition("probe").unwrap().digest, before.digest);
}

/// A mutable declaration that could leave the Plugin directory is refused.
#[test]
fn an_escaping_or_mistyped_mutable_declaration_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    for (field, declared) in [
        ("absolute", "/etc/passwd"),
        ("traversal", "../outside"),
        ("the manifest itself", "plugin.json"),
        ("empty", ""),
    ] {
        let dir = tmp.path().join("plugins/escaping");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("plugin.json"),
            serde_json::json!({"name":"Probe","mutable":[declared]}).to_string(),
        )
        .unwrap();
        assert!(
            FilePluginCatalog::new(tmp.path())
                .definition("escaping")
                .is_err(),
            "{field} was accepted"
        );
    }
}

#[test]
fn a_mistyped_dependencies_field_is_refused_by_the_catalog() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("plugins/typo");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("plugin.json"),
        r#"{"name":"Typo","dependencie":{"python":["idna==3.6"]}}"#,
    )
    .unwrap();
    let error = FilePluginCatalog::new(tmp.path())
        .definition("typo")
        .unwrap_err();
    assert!(
        error.to_string().contains("unsupported manifest field"),
        "{error}"
    );
}

#[test]
fn resolves_sorted_bundle_digest_resources_and_secret_free_mcp_names() {
    let (_tmp, catalog) = catalog_with_plugin();
    let binding = catalog.resolve(&["demo".into()]).unwrap().remove(0);
    assert_eq!(binding.id, "demo");
    assert_eq!(
        binding.resources,
        vec![".mcp.json", "plugin.json", "skills/example/SKILL.md"]
    );
    assert_eq!(binding.mcp_servers, vec!["config", "inline"]);
    let serialized = serde_json::to_string(&binding).unwrap();
    assert!(!serialized.contains("top-secret"));
    assert!(!serialized.contains("secret-value"));
    assert!(!serialized.contains("TOKEN"));
}

#[test]
fn metadata_is_live_and_separate_from_the_execution_content_identity() {
    let (tmp, catalog) = catalog_with_plugin();
    let resource = tmp.path().join("plugins/demo/notes.txt");
    fs::write(&resource, "first revision").unwrap();
    let first = catalog.definition("demo").unwrap();
    assert_eq!(catalog.metadata("demo").unwrap(), first.metadata());
    fs::write(&resource, "second revision").unwrap();
    assert_eq!(catalog.metadata("demo").unwrap(), first.metadata());
    let next = catalog.definition("demo").unwrap();
    assert_ne!(first.binding().digest, next.binding().digest);
    assert_eq!(catalog.metadata("demo").unwrap(), next.metadata());
    fs::write(
        tmp.path().join("plugins/demo/plugin.json"),
        r#"{"name":"Renamed","description":"current facts","skills":"skills/"}"#,
    )
    .unwrap();
    let metadata = catalog.metadata("demo").unwrap();
    assert_eq!(metadata.name, "Renamed");
    assert_eq!(metadata.description, "current facts");
}

#[test]
fn bundle_mutation_changes_digest() {
    let (tmp, catalog) = catalog_with_plugin();
    let before = catalog.resolve(&["demo".into()]).unwrap().remove(0).digest;
    fs::write(
        tmp.path().join("plugins/demo/skills/example/SKILL.md"),
        "changed",
    )
    .unwrap();
    let after = catalog.resolve(&["demo".into()]).unwrap().remove(0).digest;
    assert_ne!(before, after);
}

#[test]
fn unknown_plugin_is_rejected() {
    let (_tmp, catalog) = catalog_with_plugin();
    assert!(catalog.resolve(&["missing".into()]).is_err());
    assert!(catalog.resolve(&["../escape".into()]).is_err());
}

#[test]
fn package_local_credentials_are_rejected() {
    let (tmp, catalog) = catalog_with_plugin();
    fs::write(tmp.path().join("plugins/demo/.env"), "TOKEN=secret").unwrap();
    assert!(catalog.resolve(&["demo".into()]).is_err());
}

#[cfg(unix)]
#[test]
fn symlink_escape_is_rejected() {
    use std::os::unix::fs::symlink;
    let (tmp, catalog) = catalog_with_plugin();
    let outside = tmp.path().join("outside.json");
    fs::write(&outside, r#"{"mcpServers":{"escape":{}}}"#).unwrap();
    symlink(&outside, tmp.path().join("plugins/demo/escape.json")).unwrap();
    assert!(catalog.resolve(&["demo".into()]).is_err());
}

#[cfg(unix)]
#[test]
fn plugin_root_symlink_is_accepted() {
    use std::os::unix::fs::symlink;
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source/demo");
    fs::create_dir_all(source.join("skills/example")).unwrap();
    fs::write(
        source.join("plugin.json"),
        r#"{"name":"Demo","mcpServers":{}}"#,
    )
    .unwrap();
    fs::write(source.join("skills/example/SKILL.md"), "hello").unwrap();
    fs::create_dir_all(tmp.path().join("plugins")).unwrap();
    symlink(&source, tmp.path().join("plugins/demo")).unwrap();

    let binding = FilePluginCatalog::new(tmp.path())
        .resolve(&["demo".into()])
        .unwrap()
        .remove(0);
    assert_eq!(
        binding.resources,
        vec!["plugin.json", "skills/example/SKILL.md"]
    );
}

#[test]
fn adopts_manifest_skills_description_channels_and_mcp_merge() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("plugins/contract");
    fs::create_dir_all(dir.join("custom/one")).unwrap();
    fs::write(dir.join("custom/one/SKILL.md"), "skill").unwrap();
    fs::write(dir.join("gateway.sh"), "# entrypoint").unwrap();
    fs::write(
        dir.join("plugin.json"),
        r#"{
            "name":"Contract",
            "description":"",
            "interface":{"longDescription":"from interface"},
            "skills":["custom"],
            "hooks":[],
            "mcpServers":"mcp-config.json"
        }"#,
    )
    .unwrap();
    fs::write(
        dir.join("mcp-config.json"),
        r#"{"mcpServers":{"gateway":{"url":"https://${MISSING:-example.test}/mcp","headers":{"X-Root":"${CODEX_PLUGIN_ROOT}","X-Literal":"${bad-name}"}}}}"#,
    )
    .unwrap();
    fs::write(
        dir.join("channel.json"),
        r#"{"platform":"demo_platform","transport":"websocket","entrypoint":"gateway.sh","required_environment":["DEMO_TOKEN"],"description":"channel","sdk":"sdk"}"#,
    )
    .unwrap();

    let catalog = FilePluginCatalog::new(tmp.path());
    let definition = catalog.definition("contract").unwrap();
    assert_eq!(definition.description, "from interface");
    assert_eq!(definition.skills, vec!["custom/one/SKILL.md"]);
    assert!(definition.unsupported.is_empty());
    assert_eq!(definition.mcp_servers.len(), 1);
    assert_eq!(definition.channels[0].platform, "demo_platform");
    assert_eq!(
        definition.channels[0].required_environment,
        vec!["DEMO_TOKEN"]
    );

    let servers = catalog.mcp_servers("contract", true).unwrap();
    let config = servers[0].config.as_object().unwrap();
    assert_eq!(config["url"], "https://example.test/mcp");
    assert_eq!(config["headers"]["X-Root"], dir.to_string_lossy().as_ref());
    assert_eq!(config["headers"]["X-Literal"], "${bad-name}");
}
