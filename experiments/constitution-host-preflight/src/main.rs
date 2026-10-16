//! K0.1 regression of the public API counterexamples, without Host execution.

use std::collections::BTreeMap;
use std::error::Error;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::sync::Arc;

use anchor_kernel::{
    Action, Authority, FileStore, Kernel, MemoryStore, Phase, Principal, PrincipalKind, Reason,
    Submission, Waiter,
};
use anchor_runtime::graph::validator::{
    GRAPH_VALIDATOR, GraphValidator, NodeSubmission, run_resource,
};
use anchor_runtime::graph::{CommitRef, GraphNode, GraphRunRecord, GraphSnapshot};
use serde_json::{Value, json};

type ProbeResult<T> = Result<T, Box<dyn Error>>;

// Authority is granted in this fixture so each observation reaches the kernel
// law or Graph validator under examination. Production authority is not changed.
struct FixtureAuthority;

impl Authority for FixtureAuthority {
    fn permits(&self, _: &Principal, _: Action, _: &str) -> bool {
        true
    }
}

fn graph_record(id: &str) -> ProbeResult<GraphRunRecord> {
    Ok(GraphRunRecord::create_with_id(
        GraphSnapshot {
            objective: "Host integration preflight".into(),
            input: json!({}),
            entry: "work".into(),
            agents: BTreeMap::new(),
            ops: BTreeMap::from([("work".into(), json!({"run": "printf fixture"}))]),
            nodes: vec![GraphNode {
                id: "work".into(),
                agent: None,
                op: Some("work".into()),
                input: None,
                plugins: vec![],
                max_rounds: None,
            }],
            edges: vec![],
            module_rounds: BTreeMap::new(),
        },
        json!({}),
        id,
    )?)
}

fn memory_kernel() -> ProbeResult<Kernel<MemoryStore>> {
    let mut kernel = Kernel::new(MemoryStore::default(), Box::new(FixtureAuthority))?;
    kernel.install(GRAPH_VALIDATOR, Arc::new(GraphValidator))?;
    Ok(kernel)
}

fn file_kernel(root: &std::path::Path) -> ProbeResult<Kernel<FileStore>> {
    let mut kernel = Kernel::new(FileStore::new(root)?, Box::new(FixtureAuthority))?;
    kernel.install(GRAPH_VALIDATOR, Arc::new(GraphValidator))?;
    Ok(kernel)
}

fn unknown_effect_wait() -> ProbeResult<Value> {
    let kernel = memory_kernel()?;
    kernel.create(run_resource(&graph_record("unknown")?, 100)?)?;
    let code = Principal::new("fixture-host", PrincipalKind::Code);
    let verdict = kernel.submit(
        &code,
        0,
        Submission::new("run/unknown", 0, Action::Request).content(json!({
            "control": "stop",
            "reason": "unknown external effect; human observation required"
        })),
    )?;
    assert_eq!(verdict.reason, Reason::EvidenceMissing);
    let observed_stop = kernel.submit(
        &code,
        0,
        Submission::new("run/unknown", 0, Action::Request)
            .content(json!({
                "control": "stop",
                "reason": "unknown external effect; human observation required"
            }))
            .evidence(anchor_kernel::Evidence::Observation {
                id: "effect/unknown".into(),
            }),
    )?;
    assert!(observed_stop.is_accepted());
    let resource = kernel.load("run/unknown")?.ok_or("Run missing")?;
    assert!(matches!(
        resource.phase(),
        Some(Phase::Waiting {
            waiter: Waiter::Human { .. }
        })
    ));
    Ok(json!({
        "id": "code_unknown_effect_wait",
        "blocking": false,
        "plan_requirement": "2.2/2.3: host-observed unknown effects put the Run in Waiter::Human using the kernel lifecycle, with the actual Code identity (C4/C5).",
        "observed": {
            "principal": code,
            "without_observation": verdict,
            "with_observation": observed_stop,
            "phase": resource.phase(),
            "open_slots": kernel.open("run/unknown")?
        },
        "source": [
            "rust/anchor-kernel/src/kernel.rs:task_control",
            "rust/anchor-runtime/src/graph/validator.rs:GraphValidator::judge"
        ]
    }))
}

fn returned_cost() -> ProbeResult<Value> {
    let kernel = memory_kernel()?;
    kernel.create(run_resource(&graph_record("budget")?, 10)?)?;
    let code = Principal::new("fixture-host", PrincipalKind::Code);
    let mut verdicts = Vec::new();
    for _ in 0..3 {
        verdicts.push(
            kernel.submit(
                &code,
                6,
                Submission::new("run/budget", 0, Action::Write)
                    .slot("work#1")
                    .content(json!({})),
            )?,
        );
    }
    let resource = kernel.load("run/budget")?.ok_or("Run missing")?;
    assert_eq!(verdicts[0].reason, Reason::ContentInvalid);
    assert_eq!(verdicts[1].reason, Reason::BudgetExhausted);
    assert_eq!(verdicts[2].reason, Reason::TaskSettled);
    assert_eq!(
        resource.task.as_ref().ok_or("Task missing")?.budget.spent,
        18
    );
    assert!(resource.is_settled());
    Ok(json!({
        "id": "returned_submission_cost",
        "blocking": false,
        "plan_requirement": "Constitution 2.5/6.1 and integration 2.2: refused or returned work still costs resources; exceeding the finite budget settles the task.",
        "observed": {
            "reported_cost": 18,
            "cost_per_submission": 6,
            "verdicts": verdicts,
            "task": resource.task,
            "history_len": kernel.history("run/budget")?.len()
        },
        "source": ["rust/anchor-kernel/src/kernel.rs:Kernel::adjudicate"]
    }))
}

fn history_after_torn_tail() -> ProbeResult<Value> {
    // All corruption and crash simulation are restricted to this owned fixture.
    let fixture = tempfile::tempdir()?;
    let kernel = file_kernel(fixture.path())?;
    kernel.create(run_resource(&graph_record("torn")?, 100)?)?;
    let resource_path = fixture.path().join("run%2Ftorn/resource.json");
    let history_path = fixture.path().join("run%2Ftorn/history.jsonl");
    let before_state_write = fs::read(&resource_path)?;
    let mut history = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&history_path)?;
    history.write_all(b"{\"torn\":")?;
    history.sync_all()?;
    drop(history);
    let history_before_stop = kernel.history("run/torn")?.len();
    let human = Principal::new("fixture-owner", PrincipalKind::Human);
    let verdict = kernel.submit(
        &human,
        0,
        Submission::new("run/torn", 0, Action::Request).content(json!({"control": "stop"})),
    )?;
    let before_restart = kernel.load("run/torn")?.ok_or("Run missing")?;
    let history_after_stop = kernel.history("run/torn")?.len();
    drop(kernel);

    // Recreate the documented crash window: the accepted record is appended,
    // but resource.json remains at its previous version. No real process crash
    // or production file is involved.
    fs::write(&resource_path, before_state_write)?;
    let restarted = file_kernel(fixture.path())?;
    let after_restart = restarted.load("run/torn")?.ok_or("Run missing")?;
    assert_eq!(history_after_stop, 1);
    assert_eq!(after_restart.version, before_restart.version);
    assert_eq!(after_restart.phase(), before_restart.phase());
    Ok(json!({
        "id": "torn_tail_recovers_future_history",
        "blocking": false,
        "plan_requirement": "Constitution 2 and H1/2.1: accepted facts remain readable; a durable record restores a lost state write after restart without replay.",
        "observed": {
            "stop_verdict": verdict,
            "history_before_stop": history_before_stop,
            "history_after_accepted_stop": history_after_stop,
            "before_restart": {
                "version": before_restart.version,
                "phase": before_restart.phase()
            },
            "after_simulated_lost_state_write_and_restart": {
                "version": after_restart.version,
                "phase": after_restart.phase(),
                "history_len": restarted.history("run/torn")?.len()
            },
            "simulation": "Restore the pre-submit resource.json only inside an owned tempfile fixture; retain the appended history exactly as written."
        },
        "source": [
            "rust/anchor-kernel/src/store.rs:FileStore::commit",
            "rust/anchor-kernel/src/store.rs:FileStore::history",
            "rust/anchor-kernel/src/store.rs:FileStore::load"
        ]
    }))
}

fn late_code_completion() -> ProbeResult<Value> {
    let kernel = memory_kernel()?;
    kernel.create(run_resource(&graph_record("late")?, 100)?)?;
    let human = Principal::new("fixture-owner", PrincipalKind::Human);
    let code = Principal::new("fixture-host", PrincipalKind::Code);
    let stop = kernel.submit(
        &human,
        0,
        Submission::new("run/late", 0, Action::Request).content(json!({"control": "stop"})),
    )?;
    let open_after_stop = kernel.open("run/late")?;
    let completion = NodeSubmission {
        summary: "fixture result".into(),
        interruption: None,
        route: None,
        output: json!({}),
        model_requests: 0,
        commit: CommitRef {
            id: "fixture-commit".into(),
            node_id: "work".into(),
            invocation: 1,
        },
        plugin_views: BTreeMap::new(),
        plugin_digests: BTreeMap::new(),
    };
    let verdict = kernel.submit(
        &code,
        0,
        Submission::new("run/late", 1, Action::Write)
            .slot("work#1")
            .content(serde_json::to_value(completion)?),
    )?;
    let resource = kernel.load("run/late")?.ok_or("Run missing")?;
    assert_eq!(verdict.reason, Reason::TaskWaiting);
    assert!(matches!(resource.phase(), Some(Phase::Waiting { .. })));
    assert_eq!(resource.version, 1);
    Ok(json!({
        "id": "late_code_completion_after_stop",
        "blocking": false,
        "plan_requirement": "C4: an accepted stop cancels executors; the scheduler must account for late Op results.",
        "observed": {
            "stop_verdict": stop,
            "open_slots_after_stop": open_after_stop,
            "late_result_verdict": verdict,
            "phase_after_late_result": resource.phase()
        },
        "kernel_rule": "D5: waiting tasks refuse non-lifecycle submissions for every principal; no scheduler convention is required."
    }))
}

fn main() -> ProbeResult<()> {
    let evidence = json!({
        "status": "k01_regression_passed",
        "kind": "public_api_regression",
        "feature_acceptance": "not_run",
        "real_provider": "not_used",
        "scope": "Existing Kernel + GraphValidator; one-node Op fixture; no executor or external side effect runs.",
        "observations": [
            unknown_effect_wait()?,
            returned_cost()?,
            history_after_torn_tail()?,
            late_code_completion()?
        ]
    });
    println!("{}", serde_json::to_string_pretty(&evidence)?);
    Ok(())
}
