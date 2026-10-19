#[allow(dead_code)]
#[path = "support/runtime_fixture.rs"]
mod fixture;
#[allow(dead_code)]
#[path = "support/goose_fixture.rs"]
mod goose;

use goose::{Host, HttpHost, Provider, Step};
use serde_json::{Value, json};
use std::fs;

const SESSION: &str = "provider-errors";
const RAW_ERROR: &str = "fixture-provider-diagnostic-9a31";

fn graph() -> Value {
    json!({"entry":"seed","ops":{"seed":{"run":"sh -c 'printf fixture > seed.txt'"}},
        "nodes":[{"id":"seed","op":"seed"}],"edges":[]})
}

fn create(server: &HttpHost) {
    let (status, saved) = server.request("POST", "/sessions", Some(&json!({"id":SESSION})));
    assert_eq!(status, 201, "{saved}");
}

fn submit(server: &HttpHost, request: &str, message: &str) -> Value {
    let (status, saved) = server.request(
        "POST",
        &format!("/sessions/{SESSION}/turns"),
        Some(&json!({"request_id":request,"message":message})),
    );
    assert_eq!(status, 202, "{saved}");
    saved["turn"].clone()
}

fn wait(server: &HttpHost, id: &str, expected: &str) -> Value {
    let mut turn = Value::Null;
    goose::wait_until("provider error terminal Turn", || {
        let (status, saved) =
            server.request("GET", &format!("/sessions/{SESSION}/turns/{id}"), None);
        assert_eq!(status, 200, "{saved}");
        turn = saved["turn"].clone();
        assert!(
            turn["status"] == "running" || turn["status"] == expected,
            "{turn}"
        );
        turn["status"] == expected
    });
    turn
}

fn history(host: &Host, turn: &Value) -> Value {
    let root = host
        .base
        .root
        .path()
        .join("state/platform/pilot")
        .join(turn["goose"]["scope"].as_str().unwrap())
        .join("process");
    host.session_conversation(&root, turn["goose"]["session"].as_str().unwrap())
}

fn evidence(host: &Host, provider: &Provider, checks: Value) {
    provider.assert_consumed();
    let record = json!({
        "status":"passed", "runtime":"goose", "goose_version":"1.53.0",
        "goose_binary_sha256":goose::GOOSE_SHA256,"real_model_requests":0,
        "host_binary_sha256":goose::digest(&host.base.root.path().join("anchor-runner-host")),
        "source_sha256":goose::digest(&std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/goose_provider_errors.rs")),
        "checks":checks,"provider_requests":provider.requests(),"production_data_used":false,
    });
    fs::write(
        provider.root.join("evidence.json"),
        serde_json::to_vec_pretty(&record).unwrap(),
    )
    .unwrap();
    println!(
        "evidence: {}",
        provider.root.join("evidence.json").display()
    );
}

#[test]
#[ignore = "requires pinned Goose binary"]
fn provider_failure_after_tool_is_failed_and_same_session_can_continue_after_restart() {
    let provider = Provider::new(
        "goose-provider-failure-followup",
        vec![
            Step::tool("graph_list", json!({})),
            // Fixed Goose retries request failures three times before ending
            // the prompt; only the model transport is repeated, not its tools.
            Step::http_error(400, RAW_ERROR).after("fixture"),
            Step::http_error(400, RAW_ERROR).after("fixture"),
            Step::http_error(400, RAW_ERROR).after("fixture"),
            Step::http_error(400, RAW_ERROR).after("fixture"),
            Step::text(
                "Ran into this error: an ordinary assistant quotation; inspection complete.",
            )
            .expect_request_contains("fixture"),
        ],
    );
    let host = Host::new(&graph()).default_runtime();
    let mut server = host.serve(&provider);
    create(&server);
    let accepted = submit(
        &server,
        "first",
        "Inspect saved graphs without starting one.",
    );
    let failed = wait(&server, accepted["id"].as_str().unwrap(), "failed");
    assert!(
        failed["error"].as_str().unwrap().contains("provider_error"),
        "{failed}"
    );
    assert!(!failed.to_string().contains(RAW_ERROR));
    let native = history(&host, &failed);
    assert!(native.to_string().contains(RAW_ERROR), "{native}");
    assert!(
        native.to_string().contains("\"type\":\"error\""),
        "{native}"
    );
    let calls = provider.requests().len();
    assert_eq!(calls, 5);
    assert_eq!(
        submit(
            &server,
            "first",
            "Inspect saved graphs without starting one."
        ),
        failed
    );
    assert_eq!(provider.requests().len(), calls);
    let events = server.events(
        &format!(
            "/sessions/{SESSION}/turns/{}/events",
            failed["id"].as_str().unwrap()
        ),
        0,
    );
    assert!(!events.contains(RAW_ERROR), "{events}");
    server.kill();
    drop(server);
    let server = host.serve(&provider);
    let next = submit(
        &server,
        "followup",
        "Inspect the retained facts and continue in this conversation.",
    );
    let completed = wait(&server, next["id"].as_str().unwrap(), "completed");
    assert_eq!(failed["goose"], completed["goose"]);
    assert!(
        !provider.requests()[5]["messages"]
            .to_string()
            .contains(RAW_ERROR)
    );
    let (status, runs) = server.request("GET", "/runs", None);
    assert_eq!(status, 200);
    assert!(runs["runs"].as_array().unwrap().is_empty());
    evidence(
        &host,
        &provider,
        json!({"failed_turn":failed,"continued_turn":completed,
        "native_history":native,"idempotent_failed_submission":true,"raw_error_not_exposed":true,
        "ordinary_error_quotation_completed":true}),
    );
}

#[test]
#[ignore = "requires pinned Goose binary"]
fn provider_authentication_errors_are_failed_without_raw_details() {
    classified_provider_error(401, "authentication");
}

#[test]
#[ignore = "requires pinned Goose binary"]
fn provider_credit_errors_are_failed_without_raw_details() {
    classified_provider_error(402, "credits_exhausted");
}

fn classified_provider_error(status: u16, kind: &str) {
    let provider = Provider::new(
        &format!("goose-provider-error-{status}"),
        vec![Step::http_error(status, RAW_ERROR)],
    );
    let host = Host::new(&graph()).default_runtime();
    let server = host.serve(&provider);
    create(&server);
    let accepted = submit(&server, "first", "Answer briefly without tools.");
    let failed = wait(&server, accepted["id"].as_str().unwrap(), "failed");
    assert!(failed["error"].as_str().unwrap().contains(kind), "{failed}");
    assert!(!failed.to_string().contains(RAW_ERROR));
    evidence(
        &host,
        &provider,
        json!({"failed_turn":failed,"category":kind,"raw_error_not_exposed":true}),
    );
}

#[test]
#[ignore = "requires pinned Goose binary"]
fn failed_reactive_compaction_is_failed_instead_of_completed() {
    let provider = Provider::new(
        "goose-provider-compaction-failure",
        vec![
            Step::tool("graph_list", json!({})),
            Step::context_length_error().after("fixture"),
            Step::http_error(400, RAW_ERROR).during_summary(),
            Step::http_error(400, RAW_ERROR).during_summary(),
            Step::http_error(400, RAW_ERROR).during_summary(),
            Step::http_error(400, RAW_ERROR).during_summary(),
        ],
    );
    let host = Host::new(&graph()).default_runtime();
    let server = host.serve(&provider);
    create(&server);
    let accepted = submit(
        &server,
        "first",
        "Inspect saved graphs and summarize the inspection.",
    );
    let failed = wait(&server, accepted["id"].as_str().unwrap(), "failed");
    assert!(
        failed["error"].as_str().unwrap().contains("provider_error"),
        "{failed}"
    );
    let native = history(&host, &failed);
    assert!(native.to_string().contains("trying to compact"), "{native}");
    evidence(
        &host,
        &provider,
        json!({"failed_turn":failed,"native_history":native,
        "summary_failure_not_completed":true,"raw_error_not_exposed":true}),
    );
}
