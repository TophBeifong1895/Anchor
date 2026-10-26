use super::*;
use crate::application::run_access::fixtures;
use crate::plugin_library::Catalog;
use anchor_graph_host::PluginCatalog;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::test_support::Environment;

fn environment(root: &Path) -> Environment {
    let names = [
        "ANCHOR_RUNNER_WORKSPACE_ROOT",
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
    let mut environment = Environment::cleared(&names);
    fs::create_dir_all(root.join("library")).unwrap();
    let binary = root.join("unstarted-goose");
    let bytes = b"This fixture pauses before any Agent or MCP client can start";
    fs::write(&binary, bytes).unwrap();

    environment.set("ANCHOR_RUNNER_WORKSPACE_ROOT", root.join("workspaces"));

    environment.set("ANCHOR_RUNNER_ALLOWED_COMMANDS", "true");
    environment.set("ANCHOR_RUNNER_AGENT_RUNTIME", "goose");
    environment.set("ANCHOR_GOOSE_BINARY", binary);
    environment.set(
        "ANCHOR_GOOSE_BINARY_SHA256",
        format!("{:x}", sha2::Sha256::digest(bytes)),
    );
    environment.set("ANCHOR_GOOSE_LOCAL_NETWORK", "0");
    environment.set("ANCHOR_GOOSE_ALLOW_SHARED_NETWORK", "1");
    environment.set("ANCHOR_MODEL_DIRECT", "0");
    crate::model_configuration::ModelConfiguration::from_env(&root.join("state"))
            .unwrap()
            .save(serde_json::from_value(json!({"endpoint":"http://127.0.0.1:1/v1","wire_api":"chat","model":"preparation-fixture","api_key":{"action":"set","value":"preparation-fixture-only"}})).unwrap())
            .unwrap();
    environment
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
    let binding = Catalog::new(root.join("library"))
        .resolve(&["research".into()])
        .unwrap()
        .remove(0);
    assert!(!binding.authorization_digest.is_empty());
    (directory, binding)
}

/// The in-memory record a caller holds before the durable copy is read. An empty
/// authorization cannot exist in memory, so the old shape lives only in the file.
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
    record
        .plugin_bindings
        .insert(current.id.clone(), current.clone());
    record.plugin_bindings_initialized = true;
    record
}

/// Retired storage is refused before its record or Plugin bindings are decoded.
fn legacy_file(root: &Path, record: &GraphRunRecord) -> (PathBuf, Vec<u8>) {
    let directory = root.join("state/runs");
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!("{}.json", record.run_id));
    let bytes = b"retired record must never be decoded".to_vec();
    fs::write(&path, &bytes).unwrap();
    (path, bytes)
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
        },
        bindings,
        &crate::HostConfiguration {
            state_root: Some(root.join("state")),
            library_root: Some(root.join("library")),
            ..Default::default()
        },
        None,
        Some(root.join("library")),
    )
}

#[tokio::test]
async fn legacy_mcp_preparation_and_load_refuse_retired_records_without_rewrite() {
    let _env_guard = crate::PROCESS_ENV_LOCK.lock().await;
    let root = tempfile::tempdir().unwrap();
    let _environment = environment(root.path());
    let (_, current) = plugin(root.path());
    let record = legacy_record("legacy-mcp", &current);
    let (path, original_bytes) = legacy_file(root.path(), &record);
    let error = match prepare(root.path(), &record, record.plugin_bindings.clone()) {
        Ok(_) => panic!("retired Run was accepted"),
        Err(error) => error,
    };
    assert!(error.contains("legacy_run_record"), "{error}");
    let error = crate::application::run_access::load(&root.path().join("state"), &record.run_id)
        .unwrap_err()
        .to_string();
    assert!(error.contains("legacy_run_record"), "{error}");
    assert_eq!(fs::read(path).unwrap(), original_bytes);
    assert_no_execution_state(root.path(), &record.run_id);
}

/// A refused preparation leaves no execution handle; nothing beyond the
/// untouched durable record may exist for the Run.
fn assert_no_execution_state(root: &Path, run_id: &str) {
    let mut found = Vec::new();
    let mut pending = vec![root.join("state"), root.join("workspaces")];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path.clone());
            }
            if path.to_string_lossy().contains(run_id)
                && path != root.join(format!("state/runs/{run_id}.json"))
            {
                found.push(path);
            }
        }
    }
    assert!(found.is_empty(), "refused Run created state: {found:?}");
}

#[tokio::test]
async fn a_waiting_parents_preparation_validates_its_legacy_child_without_upgrading_either() {
    let _env_guard = crate::PROCESS_ENV_LOCK.lock().await;
    let root = tempfile::tempdir().unwrap();
    let _environment = environment(root.path());
    let (_, current) = plugin(root.path());
    let child = legacy_record("legacy-child", &current);
    let (child_path, child_bytes) = legacy_file(root.path(), &child);
    let spec = json!({"graph":"child","mode":"wait"});
    let snapshot = GraphSnapshot::admit(json!({
        "objective":"validate a waiting child's legacy identity",
        "entry":"invoke","ops":{"invoke":{"call":spec}},
        "nodes":[{"id":"invoke","op":"invoke"}],"edges":[]
    }))
    .unwrap();
    let parent = GraphRunRecord::create_with_id(snapshot, Value::Null, "waiting-parent").unwrap();
    let application = crate::node_host::test_runs::application(&root.path().join("state"));
    fixtures::create(&application, &parent);
    fixtures::bind_child(&application, &parent.run_id, "invoke", &child.run_id);
    let parent = fixtures::record(&application, &parent.run_id);
    drop(application);
    let parent_path = root
        .path()
        .join("state/kernel-runs/run%2Fwaiting-parent/resource.json");
    let parent_bytes = fs::read(&parent_path).unwrap();
    let error = match prepare(root.path(), &parent, BTreeMap::new()) {
        Ok(_) => panic!("legacy child was accepted"),
        Err(error) => error,
    };
    // The Kernel parent is current. Its independently owned child remains
    // retired and must be refused without modifying either fact.
    assert!(error.contains("legacy_run_record"), "{error}");
    assert_eq!(fs::read(&child_path).unwrap(), child_bytes);
    assert_eq!(fs::read(&parent_path).unwrap(), parent_bytes);
}

#[tokio::test]
async fn current_run_preparation_refuses_mutable_declaration_files() {
    let _env_guard = crate::PROCESS_ENV_LOCK.lock().await;
    let root = tempfile::tempdir().unwrap();
    let _environment = environment(root.path());
    for (index, (path, config)) in [(".mcp.json", ".mcp.json"), ("state/", "state/mcp.json")]
        .into_iter()
        .enumerate()
    {
        let (directory, current) = plugin(root.path());
        let mut record = legacy_record(&format!("mutable-mcp-{index}"), &current);
        record.plugin_bindings.insert(current.id.clone(), current);
        let application = crate::node_host::test_runs::application(&root.path().join("state"));
        fixtures::create(&application, &record);
        fixtures::control(&application, &record.run_id, "stop", "paused");
        drop(application);
        let record_path = root.path().join(format!(
            "state/kernel-runs/run%2Fmutable-mcp-{index}/resource.json"
        ));
        let original_bytes = fs::read(&record_path).unwrap();
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
