#[allow(dead_code)]
#[path = "support/runtime_fixture.rs"]
mod fixture;
#[allow(dead_code)]
#[path = "support/goose_fixture.rs"]
mod goose;

use goose::{Host, HttpHost, Provider, Step};
use serde_json::{Value, json};
use std::{fs, path::Path};

const SESSION: &str = "plugin-summary";
const HEADER: &str = "Current Anchor Plugin capabilities for this invocation.";

fn install(host: &Host, id: &str, name: &str, description: &str, body: &str) {
    let source = host.base.root.path().join("checkout").join(id);
    fs::create_dir_all(&source).unwrap();
    fs::write(
        source.join("plugin.json"),
        json!({"name":name,"description":description}).to_string(),
    )
    .unwrap();
    fs::write(source.join("instructions.md"), body).unwrap();
    anchor_library::Library::new(host.base.root.path())
        .install_directory(id, &source, false)
        .unwrap();
}

fn mount(server: &HttpHost, id: &str, on: bool) {
    let (status, saved) = server.request(
        "POST",
        &format!("/sessions/{SESSION}/plugins/{id}"),
        Some(&json!({"on":on})),
    );
    assert_eq!(status, 200, "{saved}");
}

fn turn(server: &HttpHost, request: &str) -> Value {
    let (status, submitted) = server.request(
        "POST",
        &format!("/sessions/{SESSION}/turns"),
        Some(&json!({"request_id":request,"message":"Inspect the available capability relevant to this task."})),
    );
    assert_eq!(status, 202, "{submitted}");
    let id = submitted["turn"]["id"].as_str().unwrap();
    let mut saved = Value::Null;
    goose::wait_until("Pilot summary Turn completion", || {
        saved = server
            .request("GET", &format!("/sessions/{SESSION}/turns/{id}"), None)
            .1["turn"]
            .clone();
        assert!(
            matches!(saved["status"].as_str(), Some("running" | "completed")),
            "{saved}"
        );
        saved["status"] == "completed"
    });
    saved
}

fn system(request: &Value) -> String {
    request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "system")
        .filter_map(|message| message["content"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn check_system(request: &Value, expected: Option<&str>, absent: &[&str]) {
    let system = system(request);
    assert!(system.contains(HEADER), "{system}");
    assert!(system.contains("anchor_plugins"));
    assert!(system.contains("\"omitted\":0"));
    if let Some(expected) = expected {
        assert!(system.contains(expected), "{system}");
        assert!(system.contains("\"available\":true"));
    } else {
        assert!(system.contains("\"total\":0"), "{system}");
    }
    for absent in absent {
        assert!(
            !system.contains(absent),
            "stale/private {absent} in {system}"
        );
    }
    for body in ["OLD PLUGIN BODY", "NEW PLUGIN BODY"] {
        assert!(
            !system.contains(body),
            "full body leaked into system prompt"
        );
    }
}

fn pilot_load_uses_current_summary(flag: &str) {
    let provider = Provider::new(
        &format!("goose-plugin-summary-load-{flag}"),
        vec![
            Step::text("No capability mounted yet."),
            Step::tool(
                "anchor_plugin_read",
                json!({"plugin":"old","section":"instructions"}),
            )
            .expect_request_contains("Old brief capability"),
            Step::text("Inspected the selected capability.").after("OLD PLUGIN BODY"),
            Step::tool(
                "anchor_plugin_read",
                json!({"plugin":"new","section":"instructions"}),
            )
            .expect_request_contains("New brief capability"),
            Step::text("Inspected the new selected capability.").after("NEW PLUGIN BODY"),
            Step::text("Current capabilities are empty again."),
        ],
    );
    let host = Host::new(&json!({
        "entry":"idle","ops":{"idle":{"run":"true"}},
        "nodes":[{"id":"idle","op":"idle"}],"edges":[]
    }))
    .default_runtime()
    .with_extra_environment([
        ("ANCHOR_NODE_TOOL_DISCLOSURE", flag),
        // Prove persistence comes from Anchor MCP instructions, independently
        // of the optional native tom reminder extension.
        ("ANCHOR_GOOSE_ENABLED_EXTENSIONS", "[]"),
    ]);
    install(
        &host,
        "old",
        "Old capability",
        "Old brief capability",
        "OLD PLUGIN BODY",
    );
    install(
        &host,
        "new",
        "New capability",
        "New brief capability",
        "NEW PLUGIN BODY",
    );
    let mut server = host.serve(&provider);
    assert_eq!(
        server
            .request("POST", "/sessions", Some(&json!({"id":SESSION})))
            .0,
        201
    );
    let first = turn(&server, "one");
    mount(&server, "old", true);
    let second = turn(&server, "two");
    mount(&server, "old", false);
    mount(&server, "new", true);
    let third = turn(&server, "three");
    mount(&server, "new", false);
    server.kill();
    drop(server);
    let server = host.serve(&provider);
    let fourth = turn(&server, "four");
    provider.assert_consumed();
    let requests = provider.requests();
    assert_eq!(requests.len(), 6);
    check_system(
        &requests[0],
        None,
        &["Old brief capability", "New brief capability"],
    );
    for request in &requests[1..3] {
        check_system(
            request,
            Some("Old brief capability"),
            &["New brief capability"],
        );
    }
    for request in &requests[3..5] {
        check_system(
            request,
            Some("New brief capability"),
            &["Old brief capability"],
        );
    }
    check_system(
        &requests[5],
        None,
        &["Old brief capability", "New brief capability"],
    );
    assert!(
        requests[3]["messages"]
            .to_string()
            .contains("OLD PLUGIN BODY"),
        "old history should be retained while its capability is removed from system instructions"
    );
    for saved in [&second, &third, &fourth] {
        assert_eq!(saved["goose"], first["goose"]);
    }
    let process = host
        .base
        .root
        .path()
        .join("state/platform/pilot")
        .join(first["goose"]["scope"].as_str().unwrap())
        .join("process");
    let history = host.session_conversation(&process, first["goose"]["session"].as_str().unwrap());
    assert!(history.to_string().contains("OLD PLUGIN BODY"));
    assert!(history.to_string().contains("NEW PLUGIN BODY"));
    fs::write(provider.root.join("evidence.json"), serde_json::to_vec_pretty(&json!({
        "status":"passed","runtime":"real pinned Goose ACP over Host Pilot",
        "goose_binary_sha256":goose::GOOSE_SHA256,
        "host_binary_sha256":goose::digest(&host.base.root.path().join("anchor-runner-host")),
        "test_source_sha256":goose::digest(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/goose_plugin_summary.rs")),
        "real_model_requests":0,"tool_disclosure":flag,"native_tom_enabled":false,
        "new_turn_mount_and_unmount_replace_system_summary":true,"same_native_session":true,
        "host_restart_load_rebuilds_current_summary":true,"provider_requests":requests,
        "turns":[first,second,third,fourth],"native_history":history,
        "production_data_used":false,"dotenv_loaded":false
    })).unwrap()).unwrap();
    println!(
        "evidence: {}",
        provider.root.join("evidence.json").display()
    );
}

#[test]
#[ignore = "requires pinned Goose binary"]
fn native_pilot_load_replaces_plugin_summary_in_direct_schema_mode_without_tom() {
    pilot_load_uses_current_summary("0");
}

#[test]
#[ignore = "requires pinned Goose binary"]
fn native_pilot_load_replaces_plugin_summary_in_disclosure_mode_without_tom() {
    pilot_load_uses_current_summary("1");
}
