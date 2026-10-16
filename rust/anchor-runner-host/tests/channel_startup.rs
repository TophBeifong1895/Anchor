#[allow(dead_code)]
#[path = "support/runtime_fixture.rs"]
mod fixture;

use fixture::{Host, wait_until};
use serde_json::json;
use std::{fs, os::unix::fs::PermissionsExt};

#[test]
fn deployment_channel_starts_from_an_editable_catalog_graph_without_a_pilot_entry() {
    let mut graph = json!({
        "objective":"editable channel", "entry":"assistant",
        "agents":{"worker":{"model":"fixture"}},
        "nodes":[{"id":"assistant","agent":"worker","plugins":["channel"]}],
        "edges":[]
    });
    let host = Host::new(&graph);
    let root = host.root.path();
    fs::rename(root.join("bundle"), root.join("assistant")).unwrap();
    fs::write(
        root.join("assistant/manifest.json"),
        json!({"format":1,"graph":"graph.json","plugins":[{"id":"channel"}]}).to_string(),
    )
    .unwrap();
    let plugin = root.join("plugins/channel");
    fs::create_dir_all(plugin.join("bin")).unwrap();
    fs::write(
        plugin.join("plugin.json"),
        r#"{"name":"Local channel fixture"}"#,
    )
    .unwrap();
    fs::write(
        plugin.join("channel.json"),
        json!({"platform":"wecom","transport":"websocket","entrypoint":"bin/gateway","required_environment":[]}).to_string(),
    )
    .unwrap();
    let gateway = plugin.join("bin/gateway");
    fs::write(
        &gateway,
        "#!/bin/sh\n: > \"$WECOM_CHANNEL_STATE/started\"\nexec /bin/sleep 60\n",
    )
    .unwrap();
    fs::set_permissions(gateway, fs::Permissions::from_mode(0o700)).unwrap();

    let server = host.serve_configured_without_model(|process| {
        process
            .env_remove("ANCHOR_RUNNER_GRAPH_NAME")
            .env_remove("ANCHOR_RUNNER_BUNDLE_ROOT")
            .env("ANCHOR_WECOM_GRAPH", "assistant")
            .env("ANCHOR_WECOM_REPLY_NODE", "assistant")
            .env("ANCHOR_WECOM_USERS", "fixture-member");
    });
    wait_until("selected channel process", || {
        host.state_root().join("channels/wecom/started").is_file()
    });
    let (status, channel) = server.request("GET", "/channels/wecom/configuration", None);
    assert_eq!(status, 200, "{channel}");
    assert_eq!(channel["enabled"], true);
    assert_eq!(channel["binding"]["graph"], "assistant");
    assert_eq!(channel["binding"]["plugin"], "channel");
    graph["objective"] = json!("updated channel definition");
    let (status, updated) = server.request(
        "PUT",
        "/graphs/assistant",
        Some(&json!({"definition":graph})),
    );
    assert_eq!(status, 200, "{updated}");
    assert_eq!(updated["definition"], graph);
    assert!(!root.join("dev").exists());
}
