use super::*;
use crate::application::run_access::kernel_runs::KernelRuns;
use anchor_kernel::{Action, Effect, Outcome, Phase, Principal, PrincipalKind, Reason, Submission};
use anchor_runtime::graph::{
    GraphRunRecord,
    scheduler::{EXECUTOR_ID, OPERATOR_ID},
    test_support::{Artifacts, Scripted, ScriptedCalls, parallel_fixture_graph},
    validator::run_resource_id,
};
use serde_json::json;
use std::sync::{Arc, atomic::AtomicBool};

fn points(root: &Path) -> Vec<Point> {
    std::fs::read_to_string(root.join("kernel-probe/points.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn serial_record(id: &str) -> GraphRunRecord {
    GraphRunRecord::create_with_id(
        serde_json::from_value(json!({
            "objective":"measurement fixture", "entry":"work",
            "agents":{"worker":{"model":"fixture", "instructions":"work"}},
            "nodes":[{"id":"work", "agent":"worker"}], "edges":[]
        }))
        .unwrap(),
        json!({}),
        id,
    )
    .unwrap()
}

#[tokio::test]
async fn host_probe_records_create_submit_and_scheduler_verdicts() {
    let root = tempfile::tempdir().unwrap();
    let runs = KernelRuns::open(&root.path().join("kernel-runs")).unwrap();
    let record =
        GraphRunRecord::create_with_id(parallel_fixture_graph(), json!({}), "m2-probe-sample")
            .unwrap();
    runs.create(&record, 100).unwrap();
    // Creation installs the owner; measurement starts with adjudication.
    assert!(points(root.path()).is_empty());
    let id = run_resource_id(&record.run_id);
    let unauthorized = runs
        .kernel
        .submit(
            &Principal::new("ungranted", PrincipalKind::Human),
            0,
            Submission::new(&id, 0, Action::Request).content(json!({"control":"stop"})),
        )
        .unwrap();
    assert_eq!(unauthorized.reason, Reason::Unauthorized);
    let stale = runs
        .kernel
        .submit(
            &Principal::new(OPERATOR_ID, PrincipalKind::Human),
            0,
            Submission::new(&id, 99, Action::Request).content(json!({"control":"stop"})),
        )
        .unwrap();
    assert_eq!(stale.reason, Reason::StaleVersion);
    runs.control(&record.run_id, "stop", Some("measurement pause"))
        .unwrap();
    let waiting = runs
        .kernel
        .submit(
            &Principal::new(EXECUTOR_ID, PrincipalKind::Agent),
            1,
            Submission::new(&id, 0, Action::Write).slot("closed#1"),
        )
        .unwrap();
    assert_eq!(waiting.reason, Reason::TaskWaiting);
    runs.control(&record.run_id, "resume", None).unwrap();
    let nodes = Scripted {
        seed: 1,
        inject_failures: false,
        trace: Mutex::default(),
        calls: ScriptedCalls::default(),
    };
    let resource = runs
        .drive(
            &record.run_id,
            &Artifacts,
            &nodes,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .unwrap();
    assert_eq!(
        resource.phase(),
        Some(&Phase::Settled {
            result: Outcome::Done
        })
    );
    assert!(runs.control(&record.run_id, "stop", None).is_err());
    let samples = points(root.path());
    let history = runs.kernel.history(&id).unwrap();
    assert_eq!(samples.len(), history.len());
    for (sample, report) in samples.iter().zip(&history) {
        assert_eq!(sample.principal, report.stamp.principal().kind);
        assert_eq!(sample.action, report.submission.action);
        assert_eq!(sample.cost, report.stamp.cost());
        assert_eq!(sample.effect, report.verdict.effect);
        assert_eq!(sample.verification, report.verdict.verification);
        assert_eq!(sample.reason, report.verdict.reason);
        assert_eq!(sample.component, "run");
        assert_eq!(sample.validator, GRAPH_VALIDATOR);
    }
    for principal in [
        PrincipalKind::Agent,
        PrincipalKind::Code,
        PrincipalKind::Human,
    ] {
        assert!(samples.iter().any(|point| point.principal == principal));
    }
    for effect in [Effect::Accepted, Effect::Returned, Effect::Refused] {
        assert!(samples.iter().any(|point| point.effect == effect));
    }
    for reason in [
        Reason::Ok,
        Reason::Unauthorized,
        Reason::StaleVersion,
        Reason::TaskWaiting,
        Reason::TaskSettled,
    ] {
        assert!(samples.iter().any(|point| point.reason == reason));
    }
    if let Some(destination) = std::env::var_os("ANCHOR_KERNEL_PROBE_SAMPLE") {
        std::fs::copy(root.path().join("kernel-probe/points.jsonl"), destination).unwrap();
    }
}

#[test]
fn host_probe_rebuilds_missing_or_partial_projection_from_retained_history() {
    let root = tempfile::tempdir().unwrap();
    let store_root = root.path().join("kernel-runs");
    let runs = KernelRuns::open(&store_root).unwrap();
    let record = serial_record("m2-probe-rebuild");
    runs.create(&record, 10).unwrap();
    runs.control(&record.run_id, "stop", None).unwrap();
    runs.control(&record.run_id, "resume", None).unwrap();
    let expected = points(root.path());
    let resource = runs.load(&record.run_id).unwrap().unwrap();
    let history = runs.kernel.history(&resource.id).unwrap();
    drop(runs);
    let path = root.path().join("kernel-probe/points.jsonl");
    std::fs::write(&path, b"{incomplete\n").unwrap();
    let runs = KernelRuns::open(&store_root).unwrap();
    assert_eq!(points(root.path()), expected);
    assert_eq!(runs.load(&record.run_id).unwrap(), Some(resource.clone()));
    assert_eq!(runs.kernel.history(&resource.id).unwrap(), history);
    drop(runs);
    std::fs::remove_file(path).unwrap();
    let runs = KernelRuns::open(&store_root).unwrap();
    assert_eq!(points(root.path()), expected);
    assert_eq!(runs.load(&record.run_id).unwrap(), Some(resource));
}

#[test]
fn host_probe_io_failure_preserves_accepted_and_refused_authoritative_facts() {
    let root = tempfile::tempdir().unwrap();
    // Both initial rebuild and later append fail while Run storage stays usable.
    std::fs::write(root.path().join("kernel-probe"), b"projection unavailable").unwrap();
    let runs = KernelRuns::open(&root.path().join("kernel-runs")).unwrap();
    let record = serial_record("m2-probe-io-failure");
    runs.create(&record, 10).unwrap();
    runs.control(&record.run_id, "stop", None).unwrap();
    let verdict = runs
        .kernel
        .submit(
            &Principal::new(EXECUTOR_ID, PrincipalKind::Agent),
            1,
            Submission::new(run_resource_id(&record.run_id), 0, Action::Write).slot("work#1"),
        )
        .unwrap();
    assert_eq!(verdict.reason, Reason::TaskWaiting);
    let resource = runs.load(&record.run_id).unwrap().unwrap();
    assert!(matches!(resource.phase(), Some(Phase::Waiting { .. })));
    let history = runs.kernel.history(&resource.id).unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].verdict.effect, Effect::Accepted);
    assert_eq!(history[1].verdict.effect, Effect::Refused);
    assert_eq!(history[1].verdict.reason, Reason::TaskWaiting);
    drop(runs);
    std::fs::remove_file(root.path().join("kernel-probe")).unwrap();
    let runs = KernelRuns::open(&root.path().join("kernel-runs")).unwrap();
    assert_eq!(points(root.path()).len(), 2);
    assert_eq!(runs.load(&record.run_id).unwrap(), Some(resource));
}
