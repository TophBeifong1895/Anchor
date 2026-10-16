use super::*;
use std::fs;

fn graph_json(plugins: &[&str]) -> Value {
    let mut graph = serde_json::json!({
        "objective":"demo",
        "entry":"work",
        "agents":{},
        "ops":{"work":{"run":"true"}},
        "nodes":[{"id":"work","op":"work","plugins":plugins}],
        "edges":[]
    });
    if !plugins.is_empty() {
        graph["agents"] = serde_json::json!({
            "worker":{"model":"fixture","instructions":"work"}
        });
        graph["ops"] = serde_json::json!({});
        graph["nodes"][0] = serde_json::json!({
            "id":"work","agent":"worker","plugins":plugins
        });
    }
    graph
}

/// A bundle the current host exports: a Graph and the ids it references.
fn bundle() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(
        tmp.path().join("graph.json"),
        graph_json(&["demo"]).to_string(),
    )
    .unwrap();
    let manifest = serde_json::json!({
        "format":1,
        "graph":"graph.json",
        "plugins":[{"id":"demo"}]
    });
    fs::write(tmp.path().join("manifest.json"), manifest.to_string()).unwrap();
    tmp
}

#[test]
fn loads_graph_through_shared_admission_and_returns_only_plugin_references() {
    let tmp = bundle();
    let loaded = FileGraphBundleLoader::new(tmp.path()).load().unwrap();
    assert_eq!(loaded.snapshot.objective, "demo");
    assert_eq!(loaded.authoring_definition, graph_json(&["demo"]));
    // A reference, not a copy: the id is all a bundle carries.
    assert_eq!(loaded.plugins, vec!["demo".to_owned()]);
}

/// A bundle from the previous model is refused, and the refusal names the one
/// thing the operator can do about it.
#[test]
fn a_bundle_that_still_carries_plugin_bytes_is_refused_with_a_re_export_hint() {
    // Plugin directories inside the bundle.
    let tmp = bundle();
    fs::create_dir_all(tmp.path().join("plugins/demo/skills")).unwrap();
    fs::write(
        tmp.path().join("plugins/demo/plugin.json"),
        r#"{"name":"Demo"}"#,
    )
    .unwrap();
    fs::write(tmp.path().join("plugins/demo/skills/SKILL.md"), "safe").unwrap();
    let error = FileGraphBundleLoader::new(tmp.path()).load().unwrap_err();
    assert!(error.to_string().contains("re-export"), "{error}");
    assert!(error.to_string().contains("plugins/"), "{error}");

    // The previous manifest shape: digested Plugin summaries.
    let tmp = bundle();
    fs::write(
        tmp.path().join("manifest.json"),
        serde_json::json!({
            "format":1,
            "graph":"graph.json",
            "plugins":[{"id":"demo","digest":"a".repeat(64),"resources":[],"mcp_servers":[]}]
        })
        .to_string(),
    )
    .unwrap();
    let error = FileGraphBundleLoader::new(tmp.path()).load().unwrap_err();
    assert!(error.to_string().contains("re-export"), "{error}");
    assert!(error.to_string().contains("digest"), "{error}");
}

#[test]
fn retains_authoring_graph_and_layout_while_loading_flat_runtime_snapshot() {
    let tmp = tempfile::tempdir().unwrap();
    let authoring = serde_json::json!({
        "objective":"module graph",
        "entry":"stage",
        "agents":{},
        "ops":{"work":{"run":"true"}},
        "nodes":[{"id":"stage","graph":"inner"}],
        "edges":[],
        "graphs":{"inner":{
            "entry":"inside",
            "exit":"inside",
            "nodes":[{"id":"inside","op":"work"}],
            "edges":[]
        }},
        "layout":{"positions":{"stage":{"x":17,"y":29}},
                   "edgeLabels":{"stage|done":"Finished"}}
    });
    fs::write(
        tmp.path().join("graph.json"),
        serde_json::to_vec(&authoring).unwrap(),
    )
    .unwrap();
    fs::write(
        tmp.path().join("manifest.json"),
        r#"{"format":1,"graph":"graph.json","plugins":[]}"#,
    )
    .unwrap();

    let loaded = FileGraphBundleLoader::new(tmp.path()).load().unwrap();
    assert_eq!(loaded.authoring_definition, authoring);
    assert_eq!(
        loaded.authoring_definition["layout"]["positions"]["stage"]["x"],
        17
    );
    assert_eq!(
        loaded.authoring_definition["graphs"]["inner"]["exit"],
        "inside"
    );
    assert_eq!(loaded.snapshot.nodes[0].id, "stage/inside");
    assert_eq!(loaded.snapshot.entry, "stage/inside");
}

#[test]
fn manifest_plugin_set_matches_references_after_module_expansion() {
    let tmp = bundle();
    let authoring = serde_json::json!({
        "objective":"module plugin graph",
        "entry":"stage",
        "agents":{"worker":{"model":"fixture","instructions":"work"}},
        "ops":{},
        "nodes":[{"id":"stage","graph":"inner"}],
        "edges":[],
        "graphs":{"inner":{
            "entry":"inside",
            "exit":"inside",
            "nodes":[{"id":"inside","agent":"worker","plugins":["demo"]}],
            "edges":[]
        }},
        "layout":{"positions":{"stage":{"x":31,"y":47}}}
    });
    fs::write(
        tmp.path().join("graph.json"),
        serde_json::to_vec(&authoring).unwrap(),
    )
    .unwrap();
    fs::write(
        tmp.path().join("manifest.json"),
        serde_json::to_vec(&serde_json::json!({
            "format":1,
            "graph":"graph.json",
            "plugins":[{"id":"demo"}]
        }))
        .unwrap(),
    )
    .unwrap();

    let loaded = FileGraphBundleLoader::new(tmp.path()).load().unwrap();
    assert_eq!(loaded.authoring_definition, authoring);
    assert_eq!(loaded.snapshot.nodes[0].id, "stage/inside");
    assert_eq!(loaded.snapshot.nodes[0].plugins[0].id, "demo");
    assert!(
        loaded.snapshot.nodes[0].plugins[0].required,
        "a declared Plugin is required unless the Graph says otherwise"
    );
    assert_eq!(loaded.plugins, vec!["demo".to_owned()]);
}

#[test]
fn rejects_manifest_unknown_fields_graph_paths_and_plugin_set_drift() {
    let tmp = bundle();
    let path = tmp.path().join("manifest.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["extra"] = Value::Bool(true);
    fs::write(&path, manifest.to_string()).unwrap();
    assert!(FileGraphBundleLoader::new(tmp.path()).load().is_err());

    let tmp = bundle();
    let path = tmp.path().join("manifest.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["graph"] = Value::String("../outside.json".into());
    fs::write(&path, manifest.to_string()).unwrap();
    assert!(FileGraphBundleLoader::new(tmp.path()).load().is_err());

    let tmp = bundle();
    let path = tmp.path().join("manifest.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["format"] = Value::Number(2.into());
    fs::write(&path, manifest.to_string()).unwrap();
    assert!(FileGraphBundleLoader::new(tmp.path()).load().is_err());

    let tmp = bundle();
    let path = tmp.path().join("manifest.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["plugins"] = serde_json::json!([]);
    fs::write(&path, manifest.to_string()).unwrap();
    assert!(FileGraphBundleLoader::new(tmp.path()).load().is_err());

    let tmp = bundle();
    fs::write(tmp.path().join("graph.json"), graph_json(&[]).to_string()).unwrap();
    assert!(FileGraphBundleLoader::new(tmp.path()).load().is_err());
}

#[test]
fn rejects_extra_top_level_files_and_the_old_manifest_shape() {
    let tmp = bundle();
    fs::write(tmp.path().join("unexpected.txt"), "payload").unwrap();
    assert!(FileGraphBundleLoader::new(tmp.path()).load().is_err());

    // Plugin credentials cannot arrive through a bundle any more, because a
    // bundle carries no Plugin content at all.
    let tmp = bundle();
    fs::create_dir_all(tmp.path().join("plugins/demo")).unwrap();
    fs::write(tmp.path().join("plugins/demo/.env"), "TOKEN=secret").unwrap();
    assert!(FileGraphBundleLoader::new(tmp.path()).load().is_err());
}

#[cfg(unix)]
#[test]
fn rejects_graph_symlinks_and_any_plugin_directory() {
    use std::os::unix::fs::symlink;

    let tmp = bundle();
    let outside = tmp.path().join("outside.json");
    fs::write(&outside, graph_json(&["demo"]).to_string()).unwrap();
    fs::remove_file(tmp.path().join("graph.json")).unwrap();
    symlink(&outside, tmp.path().join("graph.json")).unwrap();
    assert!(FileGraphBundleLoader::new(tmp.path()).load().is_err());

    // Even an empty or symlinked `plugins/` is Plugin content in a bundle, and
    // a bundle carries references only.
    let tmp = bundle();
    fs::create_dir(tmp.path().join("plugins")).unwrap();
    let error = FileGraphBundleLoader::new(tmp.path()).load().unwrap_err();
    assert!(error.to_string().contains("re-export"), "{error}");

    let tmp = bundle();
    let outside = tmp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    symlink(&outside, tmp.path().join("plugins")).unwrap();
    assert!(FileGraphBundleLoader::new(tmp.path()).load().is_err());
}
