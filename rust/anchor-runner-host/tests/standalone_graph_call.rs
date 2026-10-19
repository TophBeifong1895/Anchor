//! Framed/standalone (stdio `start_bundle`) Op.call regression, provider-free.
//!
//! Framing uses the same application facade and Kernel as HTTP. A top-level
//! Run records its manual identity before the Kernel admits an Op.call child.
use anchor_kernel::{FileStore, Resource, Store};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Child, Command, Stdio},
};

fn write_bundle(dir: &Path, graph: &Value) {
    fs::create_dir_all(dir).unwrap();
    fs::write(dir.join("graph.json"), graph.to_string()).unwrap();
    fs::write(
        dir.join("manifest.json"),
        r#"{"format":1,"graph":"graph.json","plugins":[]}"#,
    )
    .unwrap();
}

fn parent_graph() -> Value {
    json!({
        "objective":"parent",
        "entry":"invoke",
        "agents":{},
        "ops":{"invoke":{"call":{"graph":"child","mode":"wait","input":{"from":"parent"}}}},
        "nodes":[{"id":"invoke","op":"invoke","plugins":[]}],
        "edges":[]
    })
}

fn child_graph() -> Value {
    json!({
        "objective":"child",
        "entry":"work",
        "agents":{},
        "ops":{"work":{"run":"true"}},
        "nodes":[{"id":"work","op":"work","plugins":[]}],
        "edges":[]
    })
}

fn spawn(root: &Path, run: &str, graph_name: Option<&str>) -> Child {
    let mut command = Command::new(env!("CARGO_BIN_EXE_anchor-runner-host"));
    command
        .env("ANCHOR_RUNNER_STATE_ROOT", root.join("state"))
        .env("ANCHOR_RUNNER_WORKSPACE_ROOT", root.join("work"))
        .env("ANCHOR_RUNNER_BUNDLE_ROOT", root.join("parent"))
        .env("ANCHOR_RUNNER_CATALOG_ROOT", root.join("catalog"))
        .env("ANCHOR_RUNNER_ALLOWED_COMMANDS", "true,sh,cat")
        .env_remove("ANCHOR_MODEL_API_KEY")
        .env_remove("ANCHOR_MODEL_URL")
        .env_remove("ANCHOR_MODEL_NAME")
        .env_remove("ANCHOR_RUST_MCP_SERVERS");
    match graph_name {
        Some(name) => command.env("ANCHOR_RUNNER_GRAPH_NAME", name),
        None => command.env_remove("ANCHOR_RUNNER_GRAPH_NAME"),
    };
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let request = serde_json::to_vec(
        &json!({"op":"start_bundle","version":1,"request_id":"test","run_id":run,"input":{}}),
    )
    .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin
        .write_all(&(request.len() as u32).to_be_bytes())
        .unwrap();
    stdin.write_all(&request).unwrap();
    drop(stdin);
    child
}

fn finish(child: Child) -> Value {
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.len() >= 4, "missing host response");
    let len = u32::from_be_bytes(output.stdout[..4].try_into().unwrap()) as usize;
    serde_json::from_slice(&output.stdout[4..4 + len]).unwrap()
}

fn resource(root: &Path, run: &str) -> Resource {
    FileStore::new(root.join("state/kernel-runs"))
        .unwrap()
        .load(&anchor_runtime::graph::validator::run_resource_id(run))
        .unwrap()
        .unwrap()
}

fn record(root: &Path, run: &str) -> Value {
    resource(root, run).state
}

fn assert_done(response: &Value) {
    assert_eq!(response["projection"]["phase"], "settled", "{response}");
    assert_eq!(response["projection"]["outcome"], "done", "{response}");
    assert!(response["projection"]["reason"].is_null(), "{response}");
}

fn metadata(root: &Path, run: &str) -> Value {
    serde_json::from_slice(
        &fs::read(root.join("state/run-metadata").join(format!("{run}.json"))).unwrap(),
    )
    .unwrap()
}

fn fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    write_bundle(&root.path().join("parent"), &parent_graph());
    write_bundle(&root.path().join("catalog/child"), &child_graph());
    root
}

#[test]
fn standalone_start_bundle_persists_parent_metadata_before_child_admission() {
    let root = fixture();
    let run = "standalone-parent";
    let response = finish(spawn(root.path(), run, None));
    assert_done(&response);

    let saved = record(root.path(), run);
    let meta = metadata(root.path(), run);
    assert_eq!(meta["graph"], "parent");
    assert_eq!(meta["trigger_source"], "manual");
    assert_eq!(meta["graph_digest"], saved["graph_digest"]);
    assert_eq!(
        meta["bundle_source"],
        json!(
            fs::canonicalize(root.path().join("parent"))
                .unwrap()
                .to_string_lossy()
        )
    );
    assert!(meta["graph_call"].is_null());

    // The child was admitted through the parent metadata and never had manual
    // metadata fabricated for it.
    let child_id = saved["graph_calls"]
        .as_object()
        .unwrap()
        .values()
        .next()
        .unwrap()["child_run_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let child = resource(root.path(), &child_id);
    assert!(
        matches!(
            child.phase(),
            Some(anchor_kernel::Phase::Settled {
                result: anchor_kernel::Outcome::Done
            })
        ),
        "{child:?}"
    );
    let child_meta = metadata(root.path(), &child_id);
    assert_eq!(child_meta["trigger_source"], "graph_call");
    assert_eq!(child_meta["graph"], "child");

    // Idempotent replay records the same metadata and does not re-admit the child.
    let response = finish(spawn(root.path(), run, None));
    assert_done(&response);
    assert_eq!(meta, metadata(root.path(), run));
    assert_eq!(saved, record(root.path(), run));
}

#[test]
fn standalone_graph_name_override_is_recorded_in_parent_metadata() {
    let root = fixture();
    let run = "standalone-named";
    let response = finish(spawn(root.path(), run, Some("custom-parent")));
    assert_done(&response);
    let meta = metadata(root.path(), run);
    assert_eq!(meta["graph"], "custom-parent");
    assert_eq!(meta["trigger_source"], "manual");
}

#[test]
fn standalone_refuses_retired_storage_without_reading_or_rewriting_it() {
    let root = fixture();
    let run = "retired-parent";
    let directory = root.path().join("state/runs");
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!("{run}.json"));
    let original = b"retired storage is deliberately not valid JSON";
    fs::write(&path, original).unwrap();

    let response = finish(spawn(root.path(), run, None));
    assert_eq!(response["kind"], "rejected", "{response}");
    assert!(
        response["reason"]
            .as_str()
            .unwrap()
            .contains("legacy_run_record"),
        "{response}"
    );
    assert_eq!(fs::read(&path).unwrap(), original);
    assert!(
        !root
            .path()
            .join(format!("state/run-metadata/{run}.json"))
            .exists()
    );
    assert!(
        !root
            .path()
            .join(format!("state/kernel-runs/run%2F{run}"))
            .exists()
    );
}
