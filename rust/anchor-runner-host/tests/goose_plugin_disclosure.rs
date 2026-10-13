#[allow(dead_code)]
#[path = "support/runtime_fixture.rs"]
mod fixture;
#[allow(dead_code)]
#[path = "support/goose_fixture.rs"]
mod goose;

use goose::{Host, Provider, Step};
use serde_json::{Value, json};
use std::fs;

fn install(host: &Host, id: &str, mcp: bool) {
    let source = host.base.root.path().join("checkout").join(id);
    fs::create_dir_all(source.join("skills/first")).unwrap();
    fs::create_dir_all(source.join("skills/second")).unwrap();
    fs::write(
        source.join("instructions.md"),
        "ONLY PLUGIN LEVEL INSTRUCTIONS",
    )
    .unwrap();
    fs::write(
        source.join("skills/first/SKILL.md"),
        "---\nname: First\ndescription: 中文证据搜索\n---\nONLY SELECTED SKILL BODY",
    )
    .unwrap();
    fs::write(
        source.join("skills/second/SKILL.md"),
        "# Second\nUNSELECTED SKILL BODY",
    )
    .unwrap();
    fs::write(source.join("note.txt"), "ONLY SELECTED RESOURCE").unwrap();
    let mut manifest = json!({"name":"Disclosure fixture","description":"中文多词发现"});
    if mcp {
        let binary = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("examples/http_fixture");
        assert!(
            binary.is_file(),
            "build anchor-mcp-host --example http_fixture first"
        );
        fs::copy(binary, source.join("mcp-fixture")).unwrap();
        // Debug DWARF is ~90 MiB and irrelevant to a protocol fixture. Keep this
        // installed package small so mount/read hashing fits normal HTTP limits.
        assert!(
            std::process::Command::new("strip")
                .arg(source.join("mcp-fixture"))
                .status()
                .unwrap()
                .success()
        );
        manifest["mcpServers"] = json!({"service":{"command":"mcp-fixture","args":["--stdio","echo","--prefix","native:"]}});
    }
    fs::write(source.join("plugin.json"), manifest.to_string()).unwrap();
    anchor_library::Library::new(host.base.root.path())
        .install_directory(id, &source, false)
        .unwrap();
}

fn payload_with(value: &Value, key: &str) -> Option<Value> {
    match value {
        Value::Object(fields) if fields.contains_key(key) => Some(value.clone()),
        Value::Object(fields) => fields.values().find_map(|value| payload_with(value, key)),
        Value::Array(values) => values
            .iter()
            .rev()
            .find_map(|value| payload_with(value, key)),
        Value::String(text) => serde_json::from_str::<Value>(text)
            .ok()
            .and_then(|value| payload_with(&value, key)),
        _ => None,
    }
}

#[test]
#[ignore = "requires pinned Goose binary, MCP http_fixture and local Bubblewrap"]
fn pilot_discovers_only_frozen_plugins_reads_selected_files_and_executes_original_target() {
    let provider = Provider::new(
        "goose-pilot-plugin-disclosure",
        vec![
            Step::tool("plugin_list", json!({"plugin":"unmounted"})),
            Step::tool("anchor_plugins", json!({"plugin":"unmounted"})).after("unmounted"),
            Step::tool("anchor_plugins", json!({"query":"中文 发现"})).after("next_offset"),
            Step::tool(
                "anchor_plugin_read",
                json!({"plugin":"demo","section":"instructions"}),
            )
            .after("demo"),
            Step::tool(
                "anchor_plugin_read",
                json!({"plugin":"demo","section":"skills"}),
            )
            .after("ONLY PLUGIN LEVEL INSTRUCTIONS"),
            Step::tool(
                "anchor_plugin_read",
                json!({"plugin":"demo","section":"skill","path":"skills/first/SKILL.md"}),
            )
            .after("First"),
            Step::tool(
                "anchor_plugin_read",
                json!({"plugin":"demo","section":"resource","path":"note.txt"}),
            )
            .after("ONLY SELECTED SKILL BODY"),
            Step::tool("anchor_tools", json!({"plugin":"demo","query":"echo"}))
                .after("ONLY SELECTED RESOURCE"),
            Step::tool("anchor_tool_detail", json!({"name":"demo-service_echo"}))
                .after("demo-service_echo"),
            Step::tool(
                "anchor_tools_call",
                json!({"name":"demo-service_echo","arguments":{"value":"pilot-marker"}}),
            )
            .after("parameters"),
            Step::text("Pilot native target execution verified.").after("native:pilot-marker"),
        ],
    );
    let host = Host::new(&json!({"entry":"idle","ops":{"idle":{"run":"true"}},"nodes":[{"id":"idle","op":"idle"}],"edges":[]})).default_runtime().with_extra_environment([("ANCHOR_NODE_TOOL_DISCLOSURE","1")]);
    install(&host, "demo", true);
    install(&host, "unmounted", false);
    let server = host.serve(&provider);
    assert_eq!(
        server
            .request("POST", "/sessions", Some(&json!({"id":"disclosure"})))
            .0,
        201
    );
    assert_eq!(
        server
            .request(
                "POST",
                "/sessions/disclosure/plugins/demo",
                Some(&json!({"on":true}))
            )
            .0,
        200
    );
    let (status,submitted) = server.request("POST","/sessions/disclosure/turns",Some(&json!({"request_id":"one","message":"Use only mounted demo; discover, read selected instructions and echo pilot-marker."})));
    assert_eq!(status, 202, "{submitted}");
    let turn = submitted["turn"]["id"].as_str().unwrap();
    let mut saved = Value::Null;
    goose::wait_until("Pilot disclosure terminal Turn", || {
        saved = server
            .request("GET", &format!("/sessions/disclosure/turns/{turn}"), None)
            .1["turn"]
            .clone();
        assert!(
            matches!(saved["status"].as_str(), Some("running" | "completed")),
            "{saved}"
        );
        saved["status"] == "completed"
    });
    provider.assert_consumed();
    let requests = provider.requests();
    assert!(
        requests
            .iter()
            .all(|request| goose::tool_definition(request, "demo-service_echo").is_err())
    );
    let selected = payload_with(
        &goose::tool_feedback(&requests[6]).last().unwrap()["content"],
        "sha256",
    )
    .unwrap();
    assert!(
        selected["text"]
            .as_str()
            .unwrap()
            .contains("ONLY SELECTED SKILL BODY")
    );
    assert!(!selected.to_string().contains("UNSELECTED SKILL BODY"));
    let detail = payload_with(
        &goose::tool_feedback(&requests[9]).last().unwrap()["content"],
        "parameters",
    )
    .unwrap();
    assert_eq!(
        detail["owner"],
        json!({"plugin":"demo","server":"service","tool":"echo"})
    );
    assert_eq!(detail["parameters"]["required"], json!(["value"]));
    let events = server.events(&format!("/sessions/disclosure/turns/{turn}/events"), 0);
    assert!(
        events.contains("\"toolName\":\"demo-service_echo\"")
            && !events.contains("\"toolName\":\"anchor_tools_call\"")
    );
    let scope = saved["goose"]["scope"].as_str().unwrap();
    let fact = fixture::read_json(
        host.base
            .root
            .path()
            .join("state/platform/pilot")
            .join(scope)
            .join("goose.json"),
    );
    assert_eq!(fact["tool_observation"]["tool"], "demo-service_echo");
    assert_eq!(
        fact["tool_observation"]["arguments"],
        json!({"value":"pilot-marker"})
    );
    let history = host.session_conversation(
        &host
            .base
            .root
            .path()
            .join("state/platform/pilot")
            .join(scope)
            .join("process"),
        saved["goose"]["session"].as_str().unwrap(),
    );
    assert!(history.to_string().contains("native:pilot-marker"));
    fs::write(provider.root.join("evidence.json"),serde_json::to_vec_pretty(&json!({"status":"passed","runtime":"goose","real_model_requests":0,"case_source":"tests/goose_plugin_disclosure.rs","provider_requests":requests,"turn":saved,"pilot_fact":fact,"production_data_used":false})).unwrap()).unwrap();
    println!(
        "evidence: {}",
        provider.root.join("evidence.json").display()
    );
}
