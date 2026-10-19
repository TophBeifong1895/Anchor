#[allow(dead_code)]
#[path = "support/runtime_fixture.rs"]
mod fixture;
#[allow(dead_code)]
#[path = "support/goose_fixture.rs"]
mod goose;

use goose::{Gate, Host, HttpHost, Provider, Step, command, complete};
use serde_json::{Value, json};
use std::fs;

fn save(server: &HttpHost, slot: &str, provider: &Provider, flash: &str, pro: &str) {
    let (status, body) = server.request(
        "PUT",
        &format!("/settings/model/providers/{slot}"),
        Some(&json!({
            "endpoint":format!("{}/v1",provider.url),"wire_api":"chat","flash":flash,"pro":pro,
            "api_key":{"action":"set","value":format!("{slot}-fixture-key")}
        })),
    );
    assert_eq!(status, 200, "{body}");
}

fn submit(server: &HttpHost, request: &str) -> Value {
    let (status, body) = server.request("POST", "/sessions/model-update/turns", Some(&json!({
        "request_id":request,"message":"Continue the conversation using the available tools if needed."
    })));
    assert_eq!(status, 202, "{body}");
    body["turn"].clone()
}

fn wait(server: &HttpHost, turn: &Value) -> Value {
    let mut result = Value::Null;
    goose::wait_until("model update Pilot turn", || {
        let (status, body) = server.request(
            "GET",
            &format!(
                "/sessions/model-update/turns/{}",
                turn["id"].as_str().unwrap()
            ),
            None,
        );
        assert_eq!(status, 200, "{body}");
        result = body["turn"].clone();
        assert!(
            result["status"] == "running" || result["status"] == "completed",
            "{result}"
        );
        result["status"] == "completed"
    });
    result
}

fn evidence(host: &Host, providers: &[&Provider], checks: Value) {
    for provider in providers {
        provider.assert_consumed();
    }
    let record = json!({"status":"passed","runtime":"goose","real_model_requests":0,"actual_goose":true,
        "production_data_used":false,"dotenv_loaded":false,"goose_version":"1.53.0",
        "goose_binary_sha256":goose::GOOSE_SHA256,
        "host_binary_sha256":goose::digest(&host.base.root.path().join("anchor-runner-host")),
        "checks":checks,"provider_requests":providers[0].requests(),
        "all_provider_requests":providers.iter().map(|provider|provider.requests()).collect::<Vec<_>>()});
    let path = providers[0].root.join("evidence.json");
    fs::write(&path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
    println!("evidence: {}", path.display());
}

#[test]
#[ignore = "requires pinned Goose binary"]
fn pilot_settings_apply_next_turn_without_restart_and_preserve_native_history() {
    let gate = Gate::new();
    let first = Provider::with_model(
        "model-pilot-first",
        vec![
            Step::tool("graph_list", json!({})).gated(&gate),
            Step::text("first-model-history-marker").after("fixture"),
        ],
        "fixture-first",
    );
    let second = Provider::with_model(
        "model-pilot-second",
        vec![
            Step::text("second-model-history-marker")
                .expect_request_contains("first-model-history-marker"),
            Step::text("history-survived-restart")
                .expect_request_contains("second-model-history-marker"),
        ],
        "fixture-second",
    );
    let backup = Provider::with_model("model-pilot-unused-backup", vec![], "fixture-backup");
    let graph = json!({"entry":"ready","ops":{"ready":{"run":"true"}},"nodes":[{"id":"ready","op":"ready"}],"edges":[]});
    let host = Host::new(&graph).default_runtime().with_extra_environment([
        ("ANCHOR_MODEL_URL", "https://ignored-environment.example/v1"),
        ("ANCHOR_MODEL_API_KEY", "ignored-environment-key"),
        ("ANCHOR_MODEL_NAME", "ignored-environment-model"),
        ("ANCHOR_MODEL_WIRE_API", "invalid-environment-protocol"),
        ("ANCHOR_MODEL_ALIASES", "invalid-environment-aliases"),
    ]);
    let mut server = host.serve(&first);
    save(&server, "default", &first, &first.model, &first.model);
    save(&server, "backup", &backup, &backup.model, &backup.model);
    assert_eq!(
        server
            .request("POST", "/sessions", Some(&json!({"id":"model-update"})))
            .0,
        201
    );
    let one = submit(&server, "one");
    gate.wait_entered();
    save(&server, "default", &second, &second.model, &second.model);
    assert!(second.requests().is_empty());
    gate.open();
    let one = wait(&server, &one);
    assert_eq!(
        first.requests().len(),
        2,
        "in-flight tool continuation must use its frozen service"
    );
    let two = wait(&server, &submit(&server, "two"));
    assert_eq!(one["goose"], two["goose"]);
    assert_eq!(second.requests()[0]["model"], "fixture-second");
    server.kill();
    drop(server);
    let server = host.serve(&first);
    let three = wait(&server, &submit(&server, "three"));
    assert_eq!(one["goose"], three["goose"]);
    let (_, status) = server.request("GET", "/settings/model", None);
    assert_eq!(status["execution"]["fields"]["model"], "fixture-second");
    let (cleared, _) = server.request("PUT", "/settings/model/providers/default", Some(&json!({
        "endpoint":format!("{}/v1",second.url),"wire_api":"chat","flash":second.model,"pro":second.model,
        "api_key":{"action":"clear"}
    })));
    assert_eq!(cleared, 200);
    let (status, _) = server.request(
        "POST",
        "/sessions/model-update/turns",
        Some(&json!({"request_id":"no-key","message":"must not fall back"})),
    );
    assert_ne!(status, 202);
    assert!(backup.requests().is_empty());
    evidence(
        &host,
        &[&first, &second, &backup],
        json!({"new_turn_latest":true,"active_turn_frozen":true,
        "native_history_preserved":true,"restart_preserved":true,"missing_default_key_no_fallback":true,"retired_environment_ignored":true}),
    );
}

#[test]
#[ignore = "requires pinned Goose binary"]
fn graph_nodes_resolve_latest_settings_and_explicit_provider_tiers() {
    let gate = Gate::new();
    let steps = |route: &str| {
        vec![
            command("printf model-artifact > evidence.txt"),
            complete(route),
        ]
    };
    let mut first_steps = steps("pro");
    first_steps[0] = command("printf first-model-artifact > evidence.txt").gated(&gate);
    let first = Provider::with_model("model-graph-first", first_steps, "fixture-first");
    let second = Provider::with_model(
        "model-graph-second",
        steps("backup-flash"),
        "fixture-second",
    );
    let mut backup_steps = steps("backup-pro");
    backup_steps.extend(steps("done"));
    let backup = Provider::with_model("model-graph-backup", backup_steps, "fixture-backup");
    let graph = json!({"entry":"flash","agents":{
        "flash":{"model":"models.default.flash"},"pro":{"model":"models.default.pro"},
        "backup-flash":{"model":"models.backup.flash"},"backup-pro":{"model":"models.backup.pro"}},
        "ops":{"done":{"run":"true"}},
        "nodes":[{"id":"flash","agent":"flash"},{"id":"pro","agent":"pro"},
            {"id":"backup-flash","agent":"backup-flash"},{"id":"backup-pro","agent":"backup-pro"},{"id":"done","op":"done"}],
        "edges":[{"from":"flash","to":"pro"},{"from":"pro","to":"backup-flash"},
            {"from":"backup-flash","to":"backup-pro"},{"from":"backup-pro","to":"done"}]});
    let host = Host::new(&graph).default_runtime().with_extra_environment([
        ("ANCHOR_MODEL_URL", "https://ignored-environment.example/v1"),
        ("ANCHOR_MODEL_API_KEY", "ignored-environment-key"),
        ("ANCHOR_MODEL_NAME", "ignored-environment-model"),
        ("ANCHOR_MODEL_WIRE_API", "invalid-environment-protocol"),
        ("ANCHOR_MODEL_ALIASES", "invalid-environment-aliases"),
    ]);
    let server = host.serve(&first);
    save(
        &server,
        "default",
        &first,
        &first.model,
        "unused-before-update",
    );
    save(&server, "backup", &backup, &backup.model, &backup.model);
    let run = server.trigger();
    gate.wait_entered();
    save(&server, "default", &second, &second.model, &second.model);
    gate.open();
    server.wait_status(&run, "completed");
    assert_eq!(first.requests().len(), 2);
    assert_eq!(second.requests().len(), 2);
    assert_eq!(backup.requests().len(), 4);
    evidence(
        &host,
        &[&first, &second, &backup],
        json!({"same_run_new_node_latest":true,"active_node_frozen":true,
        "all_four_references_executed":true,"explicit_backup_only":true,"retired_environment_ignored":true}),
    );
}
