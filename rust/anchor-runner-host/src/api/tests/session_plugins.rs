use super::*;
use anchor_platform_session::SessionStore;
use std::path::Path;

/// Set (or clear) one process environment variable for a test, and restore it.
///
/// The mount filter, the Session view and the HTTP graph routes all read the
/// deployment's pilot Graph from the environment, so a test has to name it
/// exactly as a deployment would — and put it back, even if an assertion fails.
struct EnvVar(&'static str, Option<std::ffi::OsString>);

impl EnvVar {
    fn set(name: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = env::var_os(name);
        unsafe { env::set_var(name, value) };
        Self(name, previous)
    }

    fn unset(name: &'static str) -> Self {
        let previous = env::var_os(name);
        unsafe { env::remove_var(name) };
        Self(name, previous)
    }
}

impl Drop for EnvVar {
    fn drop(&mut self) {
        unsafe {
            match &self.1 {
                Some(value) => env::set_var(self.0, value),
                None => env::remove_var(self.0),
            }
        }
    }
}

/// One installed Plugin, exactly as the Library stores it.
fn write_plugin(state: &ApiState, id: &str, manifest: Value) {
    let plugin = state.catalog_root.join("plugins").join(id);
    std::fs::create_dir_all(&plugin).unwrap();
    std::fs::write(plugin.join("plugin.json"), manifest.to_string()).unwrap();
}

/// Declare Plugins in the Graph *file*, which the Session view must not answer
/// from: the mount filter reads the admitted Run's frozen snapshot instead.
fn declare_plugins_in_the_file(state: &ApiState, references: &[(&str, bool, Option<bool>)]) {
    let declared = references
        .iter()
        .map(|(id, required, default)| {
            let mut reference = json!({"id": id, "required": required});
            if let Some(default) = default {
                reference["default"] = json!(default);
            }
            reference
        })
        .collect::<Vec<_>>();
    let graph = json!({
        "objective":"fixture","entry":"work","agents":{"worker":{"model":"fixture"}},"ops":{},
        "nodes":[{"id":"work","agent":"worker","plugins":declared}],
        "edges":[]
    });
    std::fs::write(state.bundle_root.join("graph.json"), graph.to_string()).unwrap();
    let manifest = json!({
        "format":1,"graph":"graph.json",
        "plugins":references.iter().map(|(id, _, _)| json!({"id": id})).collect::<Vec<_>>()
    });
    std::fs::write(
        state.bundle_root.join("manifest.json"),
        manifest.to_string(),
    )
    .unwrap();
}

/// One admitted Run for the Session whose frozen snapshot declares `references`,
/// with the Graph name its metadata records. This is the snapshot the page and
/// the mount filter must agree on.
fn freeze_run(
    state: &ApiState,
    session: &str,
    graph: &str,
    references: &[(&str, bool, Option<bool>)],
) {
    static RUNS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let declared = references
        .iter()
        .map(|(id, required, default)| {
            let mut reference = json!({"id": id, "required": required});
            if let Some(default) = default {
                reference["default"] = json!(default);
            }
            reference
        })
        .collect::<Vec<_>>();
    let snapshot = GraphSnapshot::admit(json!({
        "objective":"session plugins","entry":"work",
        "agents":{"worker":{"model":"fixture"}},"ops":{},
        "nodes":[{"id":"work","agent":"worker","plugins":declared}],
        "edges":[]
    }))
    .unwrap();
    let run = format!(
        "run-{session}-{}",
        RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let record = GraphRunRecord::create_with_id(snapshot, json!({}), run).unwrap();
    FileRunStore::new(state.data_root.join("runs"))
        .save(&record)
        .unwrap();
    let metadata = RunMetadata::new(
        record.run_id.clone(),
        graph.into(),
        record.graph_digest.clone(),
        &state.bundle_root,
    )
    .unwrap();
    metadata::save(&state.data_root, &metadata).unwrap();
    session_store(state)
        .attach_run("local", session, &record.run_id)
        .unwrap();
}

/// Every byte the Library owns: Plugin directories, dependency views and the
/// shared store. Anything changing under here would mean a refusal installed,
/// built a view or fetched a package after all.
fn library_bytes(state: &ApiState) -> Vec<(String, Vec<u8>)> {
    fn walk(root: &Path, directory: &Path, files: &mut Vec<(String, Vec<u8>)>) {
        let mut entries = std::fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(root, &path, files);
            } else {
                let relative = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                files.push((relative, std::fs::read(&path).unwrap()));
            }
        }
    }
    let root = crate::library_root(&state.catalog_root);
    let mut files = Vec::new();
    for subtree in ["plugins", "dependencies"] {
        let path = root.join(subtree);
        if path.is_dir() {
            walk(&root, &path, &mut files);
        }
    }
    files
}

fn session_store(state: &ApiState) -> SessionStore {
    SessionStore::open(state.data_root.join("platform/sessions.sqlite")).unwrap()
}

async fn create_session(app: Router, id: &str) {
    let body = json!({"id": id}).to_string();
    let (status, created) = call(app, "POST", "/sessions", Some(&body)).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
}

#[tokio::test]
async fn a_session_lists_installed_plugins_from_the_frozen_run_not_the_graph_file() {
    let (root, state) = fixture();
    let _env_guard = PROCESS_ENV.lock().await;
    let _pilot_graph = EnvVar::set("ANCHOR_RUNNER_GRAPH_NAME", "pilot-graph");
    write_plugin(
        &state,
        "pinned",
        json!({"name":"Pinned","description":"always on"}),
    );
    write_plugin(&state, "optional", json!({"name":"Optional"}));
    write_plugin(
        &state,
        "extra",
        json!({"name":"Extra","mcpServers":{"remote":{"url":"https://mcp.example"}}}),
    );
    write_plugin(
        &state,
        "needsview",
        json!({"name":"Needs view","dependencies":{"python":["idna==3.7"]}}),
    );
    // A directory the catalog cannot resolve is not installed.
    let broken = state.catalog_root.join("plugins/broken");
    std::fs::create_dir_all(&broken).unwrap();
    std::fs::write(broken.join("plugin.json"), "{ not json").unwrap();
    let app = router(state.clone());
    create_session(app.clone(), "sess").await;
    freeze_run(
        &state,
        "sess",
        "pilot-graph",
        &[("pinned", true, None), ("optional", false, Some(false))],
    );
    // The Graph *file* disagrees, and declares one Plugin the Run never saw. The
    // page must still answer from the admitted Run the mount filter reads.
    declare_plugins_in_the_file(
        &state,
        &[("pinned", false, Some(true)), ("fileonly", true, None)],
    );
    write_plugin(&state, "fileonly", json!({"name":"File only"}));
    // A switch that names a Plugin the Library does not have must not appear.
    session_store(&state)
        .set_plugin_mount("local", "sess", "ghost", true, "test", "stale")
        .unwrap();

    let (status, listed) = call(app, "GET", "/sessions/sess/plugins", None).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let rows = listed["plugins"].as_array().unwrap();
    let ids = rows
        .iter()
        .map(|row| row["id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        ["extra", "fileonly", "needsview", "optional", "pinned"]
    );
    let row = |id: &str| {
        rows.iter()
            .find(|row| row["id"] == id)
            .unwrap_or_else(|| panic!("no row for {id}"))
            .clone()
    };
    assert_eq!(row("pinned")["required"], true, "the frozen snapshot wins");
    assert_eq!(row("pinned")["on"], true);
    assert_eq!(row("pinned")["description"], "always on");
    assert_eq!(row("pinned")["mcpServerNames"], json!([]));
    assert_eq!(row("pinned")["available"], true);
    assert_eq!(row("pinned")["error"], Value::Null);
    assert_eq!(row("optional")["required"], false);
    assert_eq!(
        row("optional")["on"],
        false,
        "the frozen declaration's default:false stays off"
    );
    // The file-only declaration is not part of the Run that was admitted.
    assert_eq!(row("fileonly")["required"], false);
    assert_eq!(row("fileonly")["on"], false);
    assert_eq!(row("extra")["mcpServerNames"], json!(["remote"]));
    assert_eq!(row("extra")["on"], false, "undeclared is off until clicked");
    assert!(
        row("extra").get("mcpServers").is_none(),
        "the Session row names servers, it does not carry the catalog object"
    );
    assert_eq!(row("needsview")["available"], false);
    assert!(
        row("needsview")["error"]
            .as_str()
            .is_some_and(|error| error.contains("view")),
        "{}",
        row("needsview")
    );
    assert!(root.path().join("plugins/needsview").is_dir());
}

#[tokio::test]
async fn switching_an_uninstalled_plugin_is_refused_and_changes_no_bytes() {
    let (root, state) = fixture();
    let _env_guard = PROCESS_ENV.lock().await;
    write_plugin(&state, "demo", json!({"name":"Demo"}));
    std::fs::write(
        state.catalog_root.join("plugins/demo/notes.txt"),
        "plugin bytes",
    )
    .unwrap();
    // A view and shared-store file that already exist: refusals must leave them
    // byte-identical, not merely avoid creating new ones.
    for (path, bytes) in [
        (
            "dependencies/views/demo/aaaa/pyvenv.cfg",
            "home = /usr/bin\n",
        ),
        ("dependencies/store/idna/3.7/MARKER", "package bytes"),
    ] {
        let file = state.catalog_root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, bytes).unwrap();
    }
    let app = router(state.clone());
    create_session(app.clone(), "sess").await;

    let before = library_bytes(&state);
    assert!(!before.is_empty(), "the fixture must have bytes to compare");
    // The view and the shared store are inside the comparison, not beside it.
    assert!(
        before
            .iter()
            .any(|(path, _)| path.ends_with("views/demo/aaaa/pyvenv.cfg")),
        "{before:?}"
    );
    assert!(
        before
            .iter()
            .any(|(path, _)| path.ends_with("store/idna/3.7/MARKER")),
        "{before:?}"
    );
    let (status, refused) = call(
        app.clone(),
        "POST",
        "/sessions/sess/plugins/ghost",
        Some(r#"{"on":true}"#),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(
        refused["error"], "插件 `ghost` 尚未安装，不能挂载",
        "a missing Plugin has its own reason"
    );
    // A malformed id is refused as an id, not as a missing Plugin.
    let (status, refused) = call(
        app.clone(),
        "POST",
        "/sessions/sess/plugins/-bad",
        Some(r#"{"on":true}"#),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["error"], "插件 id `-bad` 不合法");

    assert_eq!(library_bytes(&state), before);
    assert!(!root.path().join("plugins/ghost").exists());
    assert!(
        session_store(&state)
            .get("local", "sess")
            .unwrap()
            .plugin_mounts
            .is_empty()
    );
}

#[tokio::test]
async fn a_successful_switch_writes_the_session_mount_and_its_audit_event() {
    let (_root, state) = fixture();
    let _env_guard = PROCESS_ENV.lock().await;
    let _pilot_graph = EnvVar::set("ANCHOR_RUNNER_GRAPH_NAME", "pilot-graph");
    write_plugin(&state, "demo", json!({"name":"Demo"}));
    write_plugin(&state, "pinned", json!({"name":"Pinned"}));
    write_plugin(
        &state,
        "needsview",
        json!({"name":"Needs view","dependencies":{"python":["idna==3.7"]}}),
    );
    let app = router(state.clone());
    create_session(app.clone(), "sess").await;
    freeze_run(&state, "sess", "pilot-graph", &[("pinned", true, None)]);

    let (status, mounted) = call(
        app.clone(),
        "POST",
        "/sessions/sess/plugins/demo",
        Some(r#"{"on":true}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{mounted}");
    assert_eq!(mounted["plugin"]["id"], "demo");
    assert_eq!(mounted["plugin"]["on"], true);
    assert_eq!(mounted["plugin"]["required"], false);
    assert_eq!(mounted["plugin"]["available"], true);
    assert_eq!(mounted["plugin"]["error"], Value::Null);

    let store = session_store(&state);
    assert_eq!(
        store.get("local", "sess").unwrap().plugin_mounts,
        BTreeMap::from([("demo".to_owned(), true)])
    );
    let events = store.events("local", "sess", 0).unwrap();
    assert!(
        events.iter().any(|event| {
            event.kind == "session.plugin_mount.changed"
                && event.data["plugin"] == "demo"
                && event.data["on"] == true
        }),
        "{events:?}"
    );

    let (status, listed) = call(app.clone(), "GET", "/sessions/sess/plugins", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        listed["plugins"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["id"] == "demo" && row["on"] == true),
        "{listed}"
    );

    // A Plugin the admitted Run froze as required cannot be switched off.
    let (status, refused) = call(
        app.clone(),
        "POST",
        "/sessions/sess/plugins/pinned",
        Some(r#"{"on":false}"#),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(
        refused["error"],
        "该插件在图里被声明为必需（required: true）"
    );
    // A Plugin whose own declaration has no usable view cannot be mounted: that
    // would be building a view, which mounting never does.
    let (status, refused) = call(
        app.clone(),
        "POST",
        "/sessions/sess/plugins/needsview",
        Some(r#"{"on":true}"#),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert!(
        refused["error"]
            .as_str()
            .is_some_and(|error| error.contains("view")),
        "{refused}"
    );
    // Neither refusal moved the switch or appended an event.
    assert_eq!(
        store.get("local", "sess").unwrap().plugin_mounts,
        BTreeMap::from([("demo".to_owned(), true)])
    );
    assert_eq!(
        store.events("local", "sess", 0).unwrap().len(),
        events.len()
    );

    let (status, off) = call(
        app.clone(),
        "POST",
        "/sessions/sess/plugins/demo",
        Some(r#"{"on":false}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{off}");
    assert_eq!(off["plugin"]["on"], false);
    assert_eq!(
        store.get("local", "sess").unwrap().plugin_mounts,
        BTreeMap::from([("demo".to_owned(), false)])
    );

    // An archived Session is a closed border.
    let (status, _) = call(
        app.clone(),
        "POST",
        "/sessions/sess/status",
        Some(r#"{"status":"archived","reason":"done"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, refused) = call(
        app,
        "POST",
        "/sessions/sess/plugins/demo",
        Some(r#"{"on":true}"#),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert!(refused["error"].is_string(), "{refused}");
}

#[tokio::test]
async fn only_the_configured_pilot_graph_may_declare_a_plugin_required() {
    let (_root, state) = fixture();
    let _env_guard = PROCESS_ENV.lock().await;
    let _pilot_graph = EnvVar::set("ANCHOR_RUNNER_GRAPH_NAME", "pilot-graph");
    write_plugin(&state, "trusted", json!({"name":"Trusted"}));
    write_plugin(&state, "made-here", json!({"name":"Made here"}));
    let app = router(state.clone());
    create_session(app.clone(), "pilot-sess").await;
    create_session(app.clone(), "own-sess").await;
    // `required: true` from the deployment's own pilot Graph is an authorization.
    freeze_run(
        &state,
        "pilot-sess",
        "pilot-graph",
        &[("trusted", true, None)],
    );
    // A Graph the assistant created in this Session is not that Graph: its
    // `required` is downgraded to optional, exactly as the mount filter does, so
    // the human keeps the switch.
    freeze_run(
        &state,
        "own-sess",
        "graph-the-assistant-made",
        &[("made-here", true, None)],
    );

    let (status, listed) = call(app.clone(), "GET", "/sessions/pilot-sess/plugins", None).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let row = |listed: &Value, id: &str| {
        listed["plugins"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == id)
            .unwrap_or_else(|| panic!("no row for {id}"))
            .clone()
    };
    assert_eq!(row(&listed, "trusted")["required"], true);
    let (status, refused) = call(
        app.clone(),
        "POST",
        "/sessions/pilot-sess/plugins/trusted",
        Some(r#"{"on":false}"#),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");

    let (status, listed) = call(app.clone(), "GET", "/sessions/own-sess/plugins", None).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(
        row(&listed, "made-here")["required"],
        false,
        "a Graph the assistant made may not lock a switch"
    );
    assert_eq!(
        row(&listed, "made-here")["on"],
        true,
        "downgraded to optional still mounts by default"
    );
    let (status, switched) = call(
        app,
        "POST",
        "/sessions/own-sess/plugins/made-here",
        Some(r#"{"on":false}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{switched}");
    assert_eq!(switched["plugin"]["required"], false);
    assert_eq!(switched["plugin"]["on"], false);
}

#[tokio::test]
async fn a_run_the_assistant_added_cannot_unlock_a_plugin_the_pilot_graph_requires() {
    let (_root, state) = fixture();
    let _env_guard = PROCESS_ENV.lock().await;
    let _pilot_graph = EnvVar::set("ANCHOR_RUNNER_GRAPH_NAME", "pilot-graph");
    write_plugin(&state, "pinned", json!({"name":"Pinned"}));
    let app = router(state.clone());
    create_session(app.clone(), "sess").await;
    // The operator's pilot Run requires `pinned`.
    freeze_run(&state, "sess", "pilot-graph", &[("pinned", true, None)]);
    // The assistant then admits a Run of a Graph it made itself, which does not
    // require it. Reading only the newest Run would hide the lock and offer a
    // switch whose click would write an authorization the filter ignores.
    freeze_run(
        &state,
        "sess",
        "graph-the-assistant-made",
        &[("other", false, None)],
    );

    let (status, listed) = call(app.clone(), "GET", "/sessions/sess/plugins", None).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let row = listed["plugins"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "pinned")
        .unwrap()
        .clone();
    assert_eq!(
        row["required"], true,
        "any attached Run that requires it keeps the lock"
    );
    assert_eq!(row["on"], true);
    let (status, refused) = call(
        app,
        "POST",
        "/sessions/sess/plugins/pinned",
        Some(r#"{"on":false}"#),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    // And no fake human authorization was recorded.
    assert!(
        session_store(&state)
            .events("local", "sess", 0)
            .unwrap()
            .iter()
            .all(|event| event.kind != "session.plugin_mount.changed"),
        "a refused switch must not write an authorization event"
    );
}

#[tokio::test]
async fn a_pilot_graph_named_by_the_bundle_root_keeps_its_lock() {
    let (_root, state) = fixture();
    let _env_guard = PROCESS_ENV.lock().await;
    // The deployment named no Graph explicitly: the configured bundle's own
    // directory name is the pilot Graph, and that is still the operator's lock.
    let _unnamed = EnvVar::unset("ANCHOR_RUNNER_GRAPH_NAME");
    let _bundle = EnvVar::set("ANCHOR_RUNNER_BUNDLE_ROOT", &state.bundle_root);
    write_plugin(&state, "pinned", json!({"name":"Pinned"}));
    let app = router(state.clone());
    create_session(app.clone(), "sess").await;
    let named = state
        .bundle_root
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    freeze_run(&state, "sess", &named, &[("pinned", true, None)]);

    let (status, listed) = call(app.clone(), "GET", "/sessions/sess/plugins", None).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let row = listed["plugins"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "pinned")
        .unwrap()
        .clone();
    assert_eq!(
        row["required"], true,
        "the bundle's own name is the pilot Graph when no name is configured"
    );
    let (status, refused) = call(
        app,
        "POST",
        "/sessions/sess/plugins/pinned",
        Some(r#"{"on":false}"#),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
}
