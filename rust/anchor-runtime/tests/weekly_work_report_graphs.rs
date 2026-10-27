use anchor_runtime::graph::GraphSnapshot;
use serde_json::{Value, json};
use std::collections::BTreeSet;

const CORE: &str = include_str!("../../../examples/graphs/weekly-work-report.json");
const DOCMOST: &str = include_str!("../../../examples/graphs/weekly-work-report-docmost.json");
const NOTIFY: &str = include_str!("../../../examples/graphs/weekly-work-report-notify.json");

fn compile(source: &str) -> GraphSnapshot {
    GraphSnapshot::from_authoring(serde_json::from_str(source).unwrap()).unwrap()
}

#[test]
fn weekly_examples_compile_and_admit() {
    for source in [CORE, DOCMOST, NOTIFY] {
        let snapshot = compile(source);
        let admitted = GraphSnapshot::admit(serde_json::to_value(&snapshot).unwrap()).unwrap();
        assert_eq!(snapshot, admitted);
    }
}

/// The notify wrapper is a thin call-and-send: it adds no agents, no Plugins
/// and no second publication path.
#[test]
fn notify_wrapper_calls_the_core_graph_and_sends_through_the_host_helper() {
    let snapshot = compile(NOTIFY);
    assert_eq!(snapshot.entry, "validate");
    assert!(snapshot.agents.is_empty());
    assert_eq!(
        snapshot.ops["report"]["call"]["graph"],
        Value::from("weekly-work-report")
    );
    assert_eq!(snapshot.ops["report"]["call"]["mode"], Value::from("wait"));
    let notify = snapshot.ops["notify"]["run"].as_str().unwrap();
    assert!(
        notify.contains("anchor-channel-send --bot")
            || notify.contains("\"anchor-channel-send\", \"--bot\""),
        "{notify}"
    );
    assert!(notify.contains("/in/report/result/report.md"), "{notify}");
    // The send target is the operator's input, never a literal in the Graph.
    assert!(notify.contains("notify_bot") && notify.contains("notify_userid"));
}

#[test]
fn core_ends_at_local_publish_without_plugins_or_accounts() {
    let snapshot = compile(CORE);
    assert_eq!(snapshot.entry, "collect");
    assert_eq!(snapshot.input, json!({"end": null}));
    let terminals: Vec<_> = snapshot
        .nodes
        .iter()
        .filter(|node| !snapshot.edges.iter().any(|edge| edge.from_node == node.id))
        .map(|node| node.id.as_str())
        .collect();
    assert_eq!(terminals, ["publish"]);
    assert!(snapshot.nodes.iter().all(|node| node.plugins.is_empty()));
    assert!(snapshot.agents.values().all(|agent| !agent.network));
    assert!(snapshot.ops.values().all(|op| op.get("call").is_none()));
    for forbidden in ["docmost", "MsteinL", "/root/", "/home/", "session_id"] {
        assert!(!CORE.to_lowercase().contains(&forbidden.to_lowercase()));
        assert!(!DOCMOST.contains(forbidden) || forbidden == "docmost");
    }
}

#[test]
fn core_keeps_revision_routes_and_truthful_empty_evidence_policy() {
    let snapshot = compile(CORE);
    let routes: BTreeSet<_> = snapshot
        .edges
        .iter()
        .filter(|edge| edge.from_node == "gate")
        .map(|edge| edge.to_node.as_str())
        .collect();
    assert_eq!(routes, BTreeSet::from(["publish", "understand", "write"]));
    for agent in snapshot.agents.values() {
        for policy in ["不可信", "unavailable", "partial", "没有工作"] {
            assert!(agent.instructions.contains(policy));
        }
    }
    for source in ["codex", "claude", "deepseek"] {
        assert!(snapshot.agents["analyst"].instructions.contains(source));
        assert!(snapshot.agents["reviewer"].instructions.contains(source));
    }
    assert!(
        snapshot.agents["reviewer"]
            .instructions
            .contains("可判 publish")
    );
}

#[test]
fn core_uses_native_publication_and_declares_assets_archive() {
    let snapshot = compile(CORE);
    assert_eq!(
        snapshot.ops["publish"]["run"],
        "/local-inputs/rsi weekly-publish"
    );
    for operation in ["collect", "gate", "publish"] {
        assert!(
            snapshot.ops[operation]["run"]
                .as_str()
                .unwrap()
                .starts_with("/local-inputs/rsi ")
        );
    }
    let writes = snapshot.ops["publish"]["writes"].as_array().unwrap();
    for artifact in [
        "report.md",
        "sources.md",
        "assets",
        "assets.tar",
        "review.json",
        "review.md",
    ] {
        assert!(writes.contains(&json!(artifact)));
    }
}

#[test]
fn wrapper_requires_explicit_destination_before_calling_core() {
    let snapshot = compile(DOCMOST);
    assert_eq!(snapshot.entry, "validate");
    for field in ["docmost_space", "docmost_parent_title"] {
        assert_eq!(snapshot.input[field], Value::Null);
        assert!(
            snapshot.ops["validate"]["run"]
                .as_str()
                .unwrap()
                .contains(field)
        );
    }
    assert!(snapshot.input.get("timezone").is_none());
    assert_eq!(snapshot.nodes.len(), 3);
    assert_eq!(snapshot.agents.len(), 1);
    assert_eq!(snapshot.ops.len(), 2);
    let edges: Vec<_> = snapshot
        .edges
        .iter()
        .map(|edge| (edge.from_node.as_str(), edge.to_node.as_str()))
        .collect();
    assert_eq!(edges, [("validate", "report"), ("report", "docmost")]);
}

#[test]
fn wrapper_waits_for_core_and_maps_window_without_destination() {
    let core = compile(CORE);
    let wrapper = compile(DOCMOST);
    let call = &wrapper.ops["report"]["call"];
    assert_eq!(call["graph"], "weekly-work-report");
    assert_eq!(call["mode"], "wait");
    assert_eq!(call["input_map"], json!({"end": "/end"}));
    for (field, pointer) in call["input_map"].as_object().unwrap() {
        assert!(core.input.get(field).is_some());
        assert_eq!(
            wrapper.input.pointer(pointer.as_str().unwrap()),
            core.input.get(field)
        );
    }
    assert!(call.get("files").is_none());
    assert_eq!(call["result"]["node"], "publish");
}

#[test]
fn wrapper_selects_declared_regular_files_and_reads_call_result_path() {
    let core = compile(CORE);
    let wrapper = compile(DOCMOST);
    let files = wrapper.ops["report"]["call"]["result"]["files"]
        .as_array()
        .unwrap();
    assert_eq!(
        wrapper.ops["report"]["call"]["result"]["files"],
        json!([
            "report.md",
            "sources.md",
            "assets.tar",
            "review.json",
            "review.md"
        ])
    );
    let publisher = &wrapper.agents["docmost_publisher"];
    for file in files {
        assert!(
            core.ops["publish"]["writes"]
                .as_array()
                .unwrap()
                .contains(file)
        );
        assert!(
            publisher
                .reads
                .contains(&format!("result/{}", file.as_str().unwrap()))
        );
    }
    assert!(!files.contains(&json!("assets")));
    assert!(publisher.network);
    let node = wrapper
        .nodes
        .iter()
        .find(|node| node.id == "docmost")
        .unwrap();
    assert_eq!(node.plugins.len(), 1);
    assert_eq!(node.plugins[0].id, "docmost");
    assert!(node.plugins[0].required);
    for contract in [
        "/in/report/result/report.md",
        "tar -xf /in/report/result/assets.tar -C /workspace",
        "attachments_upload_page_image",
        "attachmentId",
        "原位替换",
        "不发布 sources.md",
    ] {
        assert!(publisher.instructions.contains(contract));
    }
    assert!(!publisher.instructions.contains("/in/publish/"));
}
