use super::*;
use anchor_graph_host::{FilePluginCatalog, PluginCatalog};
use anchor_runtime::graph::{GraphCallRecord, GraphCallStatus, RunCursor};
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
};

struct Environment(Vec<(&'static str, Option<OsString>)>);

impl Environment {
    fn configure(root: &Path) -> Self {
        let names = [
            "ANCHOR_RUNNER_STATE_ROOT",
            "ANCHOR_RUNNER_WORKSPACE_ROOT",
            "ANCHOR_RUNNER_CATALOG_ROOT",
            "ANCHOR_RUNNER_LIBRARY_ROOT",
            "ANCHOR_RUNNER_ALLOWED_COMMANDS",
            "ANCHOR_RUNNER_LOCAL_INPUTS_ROOT",
            "ANCHOR_RUNNER_FAKE_PLUGINS",
            "ANCHOR_RUNNER_AGENT_RUNTIME",
            "ANCHOR_GOOSE_BINARY",
            "ANCHOR_GOOSE_BINARY_SHA256",
            "ANCHOR_GOOSE_LOCAL_NETWORK",
            "ANCHOR_GOOSE_ALLOW_SHARED_NETWORK",
            "ANCHOR_MODEL_URL",
            "ANCHOR_MODEL_API_KEY",
            "ANCHOR_MODEL_NAME",
            "ANCHOR_MODEL_WIRE_API",
            "ANCHOR_MODEL_DIRECT",
            "ANCHOR_MODEL_ALIASES",
            "ANCHOR_MODEL_CONTEXT_WINDOW",
        ];
        let previous = names
            .iter()
            .map(|name| (*name, std::env::var_os(name)))
            .collect();
        fs::create_dir_all(root.join("library")).unwrap();
        let binary = root.join("unstarted-goose");
        let bytes = b"This fixture pauses before any Agent or MCP client can start";
        fs::write(&binary, bytes).unwrap();
        unsafe {
            for name in names {
                std::env::remove_var(name);
            }
            std::env::set_var("ANCHOR_RUNNER_STATE_ROOT", root.join("state"));
            std::env::set_var("ANCHOR_RUNNER_WORKSPACE_ROOT", root.join("workspaces"));
            std::env::set_var("ANCHOR_RUNNER_CATALOG_ROOT", root);
            std::env::set_var("ANCHOR_RUNNER_LIBRARY_ROOT", root.join("library"));
            std::env::set_var("ANCHOR_RUNNER_ALLOWED_COMMANDS", "true");
            std::env::set_var("ANCHOR_RUNNER_AGENT_RUNTIME", "goose");
            std::env::set_var("ANCHOR_GOOSE_BINARY", binary);
            std::env::set_var(
                "ANCHOR_GOOSE_BINARY_SHA256",
                format!("{:x}", sha2::Sha256::digest(bytes)),
            );
            std::env::set_var("ANCHOR_GOOSE_LOCAL_NETWORK", "0");
            std::env::set_var("ANCHOR_GOOSE_ALLOW_SHARED_NETWORK", "1");
            std::env::set_var("ANCHOR_MODEL_URL", "http://127.0.0.1:1/v1");
            std::env::set_var("ANCHOR_MODEL_API_KEY", "preparation-fixture-only");
            std::env::set_var("ANCHOR_MODEL_NAME", "preparation-fixture");
            std::env::set_var("ANCHOR_MODEL_WIRE_API", "chat");
            std::env::set_var("ANCHOR_MODEL_DIRECT", "0");
        }
        Self(previous)
    }
}

impl Drop for Environment {
    fn drop(&mut self) {
        for (name, previous) in &self.0 {
            unsafe {
                match previous {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }
}

fn plugin(root: &Path) -> (PathBuf, PluginBinding) {
    let directory = root.join("library/plugins/research");
    fs::create_dir_all(directory.join("state")).unwrap();
    fs::write(
        directory.join("plugin.json"),
        json!({"name":"Research","mcpServers":"mcp-config.json","mutable":["state/"]}).to_string(),
    )
    .unwrap();
    fs::write(
        directory.join("mcp-config.json"),
        json!({"mcpServers":{"papers":{"type":"http","url":"http://127.0.0.1:9/mcp"}}}).to_string(),
    )
    .unwrap();
    fs::write(directory.join("README.md"), "original code resources").unwrap();
    fs::write(directory.join("state/results.json"), "[]").unwrap();
    let binding = FilePluginCatalog::new(root.join("library"))
        .resolve(&["research".into()])
        .unwrap()
        .remove(0);
    assert!(!binding.authorization_digest.is_empty());
    (directory, binding)
}

fn legacy_record(id: &str, current: &PluginBinding) -> GraphRunRecord {
    let snapshot = GraphSnapshot::admit(json!({
        "objective":"legacy Plugin authorization recovery",
        "entry":"research",
        "agents":{"worker":{"model":"preparation-fixture"}},
        "nodes":[{"id":"research","agent":"worker","plugins":["research"]}],
        "edges":[]
    }))
    .unwrap();
    let mut record = GraphRunRecord::create_with_id(snapshot, Value::Null, id).unwrap();
    let mut legacy = current.clone();
    legacy.authorization_digest.clear();
    record.plugin_bindings.insert(legacy.id.clone(), legacy);
    record.plugin_bindings_initialized = true;
    record.status = RunStatus::Paused;
    record
}

fn legacy_file(root: &Path, record: &GraphRunRecord) -> (FileRunStore, PathBuf, Vec<u8>) {
    let directory = root.join("state/runs");
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!("{}.json", record.run_id));
    let mut value = serde_json::to_value(record).unwrap();
    value["format"] = json!(12);
    let bytes = serde_json::to_vec(&value).unwrap();
    fs::write(&path, &bytes).unwrap();
    (FileRunStore::new(directory), path, bytes)
}

fn prepare(
    root: &Path,
    record: &GraphRunRecord,
    bindings: BTreeMap<String, PluginBinding>,
) -> Result<PreparedExecution, String> {
    PreparedExecution::prepare_with_bindings(
        record,
        HostControl {
            cancellation: Default::default(),
            pause: Arc::new(AtomicBool::new(true)),
        },
        bindings,
        Some(root.to_path_buf()),
        None,
        Some(root.join("library")),
    )
}

#[tokio::test]
async fn legacy_mcp_preparation_is_read_only_and_runner_upgrade_requires_the_run_lease() {
    let _env_guard = crate::PROCESS_ENV_LOCK.lock().await;
    let root = tempfile::tempdir().unwrap();
    let _environment = Environment::configure(root.path());
    let (directory, current) = plugin(root.path());
    let record = legacy_record("legacy-mcp", &current);
    let (store, path, original_bytes) = legacy_file(root.path(), &record);
    // Ordinary Plugin-owned data never participates in either code identity or
    // the authority upgrade. Only MCP declarations must stay immutable.
    fs::write(directory.join("state/results.json"), "[1,2,3]").unwrap();

    let prepared = prepare(root.path(), &record, record.plugin_bindings.clone()).unwrap();
    assert_eq!(store.load(&record.run_id).unwrap(), Some(record.clone()));
    assert_eq!(fs::read(&path).unwrap(), original_bytes);
    let lease = store.acquire_lease(&record.run_id).unwrap();
    let error = prepared.run(record.clone()).await.unwrap_err();
    assert_eq!(
        error,
        GraphError::RunBusy(record.run_id.clone()).to_string()
    );
    assert_eq!(fs::read(&path).unwrap(), original_bytes);
    drop(lease);

    let prepared = prepare(root.path(), &record, record.plugin_bindings.clone()).unwrap();
    let resumed = prepared.run(record).await.unwrap();
    assert_eq!(resumed.status, RunStatus::Paused);
    assert_eq!(resumed.plugin_bindings["research"], current);
    assert!(resumed.cursor.is_none());
    assert!(resumed.invocations.is_empty());
    assert_eq!(store.load(&resumed.run_id).unwrap(), Some(resumed));
    let saved: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(saved["format"], 13);
    assert!(
        !saved["plugin_bindings"]["research"]["authorization_digest"]
            .as_str()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn changed_or_ambiguous_legacy_mcp_bindings_fail_before_durable_state_changes() {
    let _env_guard = crate::PROCESS_ENV_LOCK.lock().await;
    let root = tempfile::tempdir().unwrap();
    let _environment = Environment::configure(root.path());
    for change in ["code", "resources", "servers", "plugin_id"] {
        let (directory, original) = plugin(root.path());
        let mut record = legacy_record(&format!("legacy-{change}"), &original);
        match change {
            "code" => fs::write(directory.join("README.md"), "replaced code").unwrap(),
            "resources" => record
                .plugin_bindings
                .get_mut("research")
                .unwrap()
                .resources
                .clear(),
            "servers" => record
                .plugin_bindings
                .get_mut("research")
                .unwrap()
                .mcp_servers
                .clear(),
            "plugin_id" => record.plugin_bindings.get_mut("research").unwrap().id = "other".into(),
            _ => unreachable!(),
        }
        let (store, path, original_bytes) = legacy_file(root.path(), &record);
        let current = FilePluginCatalog::new(root.path().join("library"))
            .resolve(&["research".into()])
            .unwrap()
            .remove(0);
        // The application prepares from the durable legacy bindings; the CLI
        // may supply current bindings before it loads the saved record. Both
        // paths must validate the same historical baseline.
        for bindings in [
            record.plugin_bindings.clone(),
            BTreeMap::from([("research".into(), current.clone())]),
        ] {
            let error = match prepare(root.path(), &record, bindings) {
                Ok(_) => panic!("legacy {change} change was accepted"),
                Err(error) => error,
            };
            assert!(error.contains("legacy Run"), "{error}");
            assert_eq!(store.load(&record.run_id).unwrap(), Some(record.clone()));
            assert_eq!(fs::read(&path).unwrap(), original_bytes);
        }
    }
}

#[tokio::test]
async fn a_waiting_parents_preparation_validates_its_legacy_child_without_upgrading_either() {
    let _env_guard = crate::PROCESS_ENV_LOCK.lock().await;
    let root = tempfile::tempdir().unwrap();
    let _environment = Environment::configure(root.path());
    let (directory, current) = plugin(root.path());
    let child = legacy_record("legacy-child", &current);
    let (_, child_path, child_bytes) = legacy_file(root.path(), &child);
    let spec = json!({"graph":"child","mode":"wait"});
    let snapshot = GraphSnapshot::admit(json!({
        "objective":"validate a waiting child's legacy identity",
        "entry":"invoke","ops":{"invoke":{"call":spec}},
        "nodes":[{"id":"invoke","op":"invoke"}],"edges":[]
    }))
    .unwrap();
    let mut parent =
        GraphRunRecord::create_with_id(snapshot, Value::Null, "waiting-parent").unwrap();
    let identity = CallIdentity {
        parent_run_id: parent.run_id.clone(),
        parent_graph_digest: parent.graph_digest.clone(),
        node_id: "invoke".into(),
        invocation: 1,
        call_spec_digest: format!(
            "{:x}",
            sha2::Sha256::digest(serde_json::to_vec(&spec).unwrap())
        ),
    };
    parent.graph_calls.insert(
        identity.durable_key(),
        GraphCallRecord {
            identity,
            graph: "child".into(),
            child_run_id: Some(child.run_id),
            mode: "wait".into(),
            status: GraphCallStatus::Waiting,
            output: None,
            error: None,
        },
    );
    parent.cursor = Some(RunCursor {
        node_id: "invoke".into(),
        key: InvocationKey {
            run_id: parent.run_id.clone(),
            graph_digest: parent.graph_digest.clone(),
            node_id: "invoke".into(),
            invocation: 1,
        },
        input_commits: vec![],
        prepared_input: json!({"input":Value::Null,"committed_inputs":[]}),
    });
    parent.invocations.insert("invoke".into(), 1);
    parent.passes.insert("invoke".into(), 1);
    parent.status = RunStatus::WaitingCall;
    let (_, parent_path, parent_bytes) = legacy_file(root.path(), &parent);
    prepare(root.path(), &parent, BTreeMap::new()).unwrap();
    assert_eq!(fs::read(&child_path).unwrap(), child_bytes);
    assert_eq!(fs::read(&parent_path).unwrap(), parent_bytes);

    fs::write(directory.join("README.md"), "replaced child code").unwrap();
    let error = match prepare(root.path(), &parent, BTreeMap::new()) {
        Ok(_) => panic!("parent preparation ignored its child's legacy identity"),
        Err(error) => error,
    };
    assert!(error.contains("legacy Run"), "{error}");
    assert_eq!(fs::read(child_path).unwrap(), child_bytes);
    assert_eq!(fs::read(parent_path).unwrap(), parent_bytes);
}

#[tokio::test]
async fn legacy_mcp_preparation_refuses_mutable_declaration_files() {
    let _env_guard = crate::PROCESS_ENV_LOCK.lock().await;
    let root = tempfile::tempdir().unwrap();
    let _environment = Environment::configure(root.path());
    for (path, config) in [(".mcp.json", ".mcp.json"), ("state/", "state/mcp.json")] {
        let (directory, current) = plugin(root.path());
        let record = legacy_record("mutable-mcp", &current);
        let (_, record_path, original_bytes) = legacy_file(root.path(), &record);
        fs::write(
            directory.join("plugin.json"),
            json!({
                "name":"Research","mcpServers":config,"mutable":[path]
            })
            .to_string(),
        )
        .unwrap();
        let error = match prepare(root.path(), &record, record.plugin_bindings.clone()) {
            Ok(_) => panic!("mutable MCP declaration was accepted"),
            Err(error) => error,
        };
        assert!(error.contains("MCP declaration"), "{error}");
        assert_eq!(fs::read(record_path).unwrap(), original_bytes);
    }
}
