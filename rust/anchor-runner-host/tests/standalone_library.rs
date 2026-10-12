//! Standalone entrypoints must not infer Plugin authority from bundle neighbors.
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    process::{Command, Stdio},
};

#[test]
fn bundle_and_inline_admission_require_an_explicit_library_for_plugin_references() {
    let root = tempfile::tempdir().unwrap();
    let graph = json!({
        "objective":"reject a neighboring unauthorized Plugin",
        "entry":"work",
        "agents":{"worker":{"model":"fixture","instructions":"use the Plugin"}},
        "nodes":[{"id":"work","agent":"worker","plugins":["neighbor"]}],
        "edges":[]
    });
    let bundle = root.path().join("bundle");
    let neighbor = root.path().join("plugins/neighbor");
    fs::create_dir_all(&bundle).unwrap();
    fs::create_dir_all(&neighbor).unwrap();
    fs::write(bundle.join("graph.json"), graph.to_string()).unwrap();
    fs::write(
        bundle.join("manifest.json"),
        json!({"format":1,"graph":"graph.json","plugins":[{"id":"neighbor"}]}).to_string(),
    )
    .unwrap();
    fs::write(
        neighbor.join("plugin.json"),
        r#"{"name":"Neighbor","description":"Not an authorized Library"}"#,
    )
    .unwrap();

    for operation in ["start_bundle", "start_or_resume"] {
        let mut request = json!({"op":operation,"version":1,"request_id":operation,"run_id":operation,"input":{}});
        if operation == "start_or_resume" {
            request["snapshot"] = graph.clone();
        }
        let mut child = Command::new(env!("CARGO_BIN_EXE_anchor-runner-host"))
            .env("ANCHOR_RUNNER_STATE_ROOT", root.path().join("state"))
            .env("ANCHOR_RUNNER_WORKSPACE_ROOT", root.path().join("work"))
            .env("ANCHOR_RUNNER_BUNDLE_ROOT", &bundle)
            .env_remove("ANCHOR_RUNNER_LIBRARY_ROOT")
            .env_remove("ANCHOR_RUNNER_CATALOG_ROOT")
            .env_remove("ANCHOR_RUNNER_FAKE_PLUGINS")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let bytes = serde_json::to_vec(&request).unwrap();
        let mut input = child.stdin.take().unwrap();
        input
            .write_all(&(bytes.len() as u32).to_be_bytes())
            .unwrap();
        input.write_all(&bytes).unwrap();
        drop(input);
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let response: Value = serde_json::from_slice(&output.stdout[4..]).unwrap();
        assert_eq!(response["kind"], "rejected", "{response}");
        assert!(
            response["reason"]
                .as_str()
                .unwrap()
                .contains("Library root"),
            "{response}"
        );
        assert!(
            !root
                .path()
                .join(format!("state/runs/{operation}.json"))
                .exists()
        );
    }
}
