mod support;

use std::collections::BTreeSet;

use anchor_hub::adapters::{Hub, discover};
use anchor_hub::analyzers::{Analyzer, Finding, Need, Report, health::Health, stall::Stall};
use anchor_hub::query::{
    Activity, GapPlace, Query, Status, covered, node_result, repeats, summarize,
};
use anchor_hub::source::{DataRoot, Stored};
use anchor_hub::table::{Body, FactTable, MemoryTable, Projector};
use anchor_kernel::{MemoryStore, Record, Resource, Store};
use serde_json::json;
use support::{SECRET, World};

fn project(world: &World) -> (MemoryTable, Projector) {
    let mut table = MemoryTable::default();
    let mut projector = Projector::default();
    for name in ["kernel-runs", "kernel-facts"] {
        let root = DataRoot::open(&world.root(name), name).unwrap();
        projector.pull(&root, &mut table).unwrap();
    }
    (table, projector)
}

fn codes(report: &Report) -> BTreeSet<(&'static str, String)> {
    report
        .findings
        .iter()
        .map(|f| (f.code, f.subject.clone()))
        .collect()
}

fn finding<'r>(report: &'r Report, code: &str) -> &'r Finding {
    report
        .findings
        .iter()
        .find(|f| f.code == code)
        .unwrap_or_else(|| panic!("no {code} in {:#?}", report.findings))
}

#[test]
fn rows_are_keyed_by_root_resource_and_place_and_carry_no_business_content() {
    let world = World::new();
    support::r1(&world);
    let (table, _) = project(&world);
    let facts = table.facts();
    let run: Vec<_> = facts
        .rows
        .iter()
        .filter(|r| r.key.root == "kernel-runs" && r.key.resource == "run/r1")
        .collect();
    assert_eq!(
        run.iter()
            .map(|r| (r.key.place, r.slot.as_deref().unwrap(), r.at))
            .collect::<Vec<_>>(),
        vec![
            (1, "prepare#1", Some(1_200)),
            (2, "worker#1", Some(4_800)),
            (3, "critic#1", Some(6_000)),
            (4, "verify#1", Some(6_300)),
            (5, "late#1", Some(6_400)),
        ]
    );
    assert_eq!(run[3].status, Some(Status::Done));
    assert!(!run[4].verdict.is_accepted());
    assert!(run.iter().all(|r| r.body == Body::Kernel));

    let traces: Vec<_> = facts
        .rows
        .iter()
        .filter(|r| r.key.root == "kernel-facts")
        .collect();
    assert_eq!(traces.len(), 2);
    assert!(
        traces
            .iter()
            .all(|r| matches!(r.body, Body::Observation(_)))
    );
    let worker = support::configuration("worker").digest();
    assert_eq!(
        facts.configurations.keys().collect::<BTreeSet<_>>(),
        BTreeSet::from([&worker, &support::configuration("ghost").digest()])
    );

    let projected = serde_json::to_string(facts.rows).unwrap();
    assert!(!projected.contains(SECRET));
}

#[test]
fn a_later_pull_projects_only_what_was_appended() {
    let world = World::new();
    support::r1(&world);
    let (mut table, mut projector) = project(&world);
    let before = table.facts().rows.len();
    support::r2(&world);
    let mut added = 0;
    for name in ["kernel-runs", "kernel-facts"] {
        let root = DataRoot::open(&world.root(name), name).unwrap();
        added += projector.pull(&root, &mut table).unwrap();
    }
    assert_eq!(added, 2);
    assert_eq!(table.facts().rows.len(), before + 2);
    let keys: BTreeSet<_> = table.facts().rows.iter().map(|r| r.key.clone()).collect();
    assert_eq!(keys.len(), table.facts().rows.len(), "no row twice");
}

#[test]
fn a_manifest_that_does_not_hash_to_its_name_is_not_that_configuration() {
    let world = World::new();
    world.run("r", 1);
    world.observe_as(
        "r",
        "worker",
        10,
        "not-its-digest",
        Some(&support::configuration("worker")),
        vec![support::lifecycle(
            (2, 9),
            anchor_facts::Outcome::Completed,
            0,
            0,
        )],
    );
    world.result("r", "worker#1", 11, 1, true);
    let (table, _) = project(&world);
    assert!(table.facts().configurations.is_empty());
    let report = Health.analyze(&Query::new(table.facts(), None));
    assert_eq!(
        finding(&report, "configuration_unknown").subject,
        "config not-its-digest"
    );
}

#[test]
fn invocations_join_run_results_and_observations_and_keep_dangling_sides() {
    let world = World::new();
    support::r1(&world);
    let (table, _) = project(&world);
    let query = Query::new(table.facts(), None);
    let invocations = query.invocations();
    let sides: Vec<_> = invocations
        .iter()
        .map(|i| (i.key(), i.result.is_some(), i.traces.len()))
        .collect();
    assert_eq!(
        sides,
        vec![
            ("r1/critic#1".into(), true, 0),
            ("r1/ghost#1".into(), false, 1),
            ("r1/prepare#1".into(), true, 0),
            ("r1/verify#1".into(), true, 0),
            ("r1/worker#1".into(), true, 1),
        ]
    );
    let worker = &invocations[4];
    assert_eq!(worker.evidence().len(), 2);
    assert_eq!(
        worker.config(),
        Some(support::configuration("worker").digest().as_str())
    );
    assert_eq!(
        node_result(worker.result.unwrap()),
        Some(("r1", "worker", 1))
    );
}

#[test]
fn a_turn_ends_when_its_first_call_starts_even_if_its_usage_is_observed_later() {
    let world = World::new();
    support::r1(&world);
    let (table, _) = project(&world);
    let query = Query::new(table.facts(), None);
    let worker = query
        .invocations()
        .into_iter()
        .find(|i| i.key() == "r1/worker#1")
        .unwrap();
    assert_eq!(
        worker.turn_spans(),
        vec![(1, 1_400..2_400), (2, 2_480..3_480), (3, 3_630..4_630)]
    );
    let gaps: Vec<_> = worker.gaps().iter().map(|g| (g.place(), g.ms())).collect();
    assert_eq!(
        gaps,
        vec![
            (GapPlace::Startup, 100),
            (GapPlace::ToolToTurn, 30),
            (GapPlace::ToolToTurn, 30),
            (GapPlace::Teardown, 69),
        ]
    );
    // Model, tools and gaps partition the invocation.
    let model = covered(worker.turn_spans().into_iter().map(|(_, s)| s));
    let tools = covered(worker.calls().iter().filter_map(|c| c.span()));
    let idle: u64 = worker.gaps().iter().map(|g| g.ms()).sum();
    assert_eq!(model + tools + idle, 4_700 - 1_300);
    assert_eq!(
        worker.timeline().first().map(|s| s.activity),
        Some(Activity::Turn { index: 1 })
    );
}

#[test]
fn task_steps_measure_time_since_the_previous_record() {
    let world = World::new();
    support::r1(&world);
    let (table, _) = project(&world);
    let query = Query::new(table.facts(), None);
    let tasks = query.tasks();
    assert_eq!(tasks.len(), 1);
    let steps: Vec<_> = tasks[0]
        .steps()
        .iter()
        .map(|(row, since)| (row.slot.clone().unwrap(), *since))
        .collect();
    assert_eq!(
        steps,
        vec![
            ("prepare#1".into(), Some(200)),
            ("worker#1".into(), Some(3_600)),
            ("critic#1".into(), Some(1_200)),
            ("verify#1".into(), Some(300)),
            ("late#1".into(), Some(100)),
        ]
    );
    assert_eq!(tasks[0].span_ms(), Some(5_400));
}

#[test]
fn health_reports_each_irregularity_with_its_records() {
    let world = World::new();
    support::r1(&world);
    support::r2(&world);
    let (table, _) = project(&world);
    let report = Health.analyze(&Query::new(table.facts(), Some(0)));
    assert!(report.not_applicable.is_empty());
    assert_eq!(
        codes(&report),
        BTreeSet::from([
            ("not_accepted", "run/*".into()),
            ("unobserved_invocation", "r1/critic#1".into()),
            ("observation_without_result", "r1/ghost#1".into()),
            ("invocation_not_completed", "r1/ghost#1".into()),
            ("tool_failures", "tool anchor_run".into()),
            ("tool_failures", "tool anchor_read".into()),
            ("call_corrected", "tool anchor_run".into()),
        ])
    );
    let refused = finding(&report, "not_accepted");
    assert!(refused.detail.contains("task_settled"));
    assert_eq!(refused.evidence[0].place, 5);
    assert_eq!(finding(&report, "unobserved_invocation").evidence.len(), 1);
    assert_eq!(finding(&report, "call_corrected").evidence.len(), 2);

    let time = &report.summary["time"];
    let shares = ["model_share", "tool_share", "unobserved_share"]
        .map(|k| time[k].as_f64().unwrap())
        .iter()
        .sum::<f64>();
    assert!((shares - 1.0).abs() < 0.01, "{time}");
    let steps = &report.summary["steps"];
    assert_eq!(steps["observed_overhead_ms"]["min"], json!(150));
    assert_eq!(steps["observed_overhead_ms"]["max"], json!(200));
    assert_eq!(
        steps["unobserved_step_ms"]["critic"]["median"],
        json!(1_200)
    );
    assert_eq!(
        report.summary["tools"]["anchor_read"]["failures"]["not_authorized"],
        json!(3)
    );
}

#[test]
fn health_flags_a_configuration_that_waits_long_before_the_model() {
    let world = World::new();
    world.run("r", 0);
    world.observe(
        "r",
        "answer",
        2_100,
        &support::configuration("answer"),
        vec![
            support::turn(1, 2_000, 1_000),
            support::call("final_result", 1, json!({}), (2_000, 2_001), None),
            support::lifecycle((500, 2_050), anchor_facts::Outcome::Completed, 1, 1),
        ],
    );
    world.result("r", "answer#1", 2_200, 1, true);
    let (table, _) = project(&world);
    let report = Health.analyze(&Query::new(table.facts(), None));
    let startup = finding(&report, "startup_heavy");
    assert!(startup.detail.contains("32%"), "{}", startup.detail);
    assert!(
        startup.detail.contains("median 500 ms"),
        "{}",
        startup.detail
    );
}

#[test]
fn stall_finds_stalled_runs_silences_slow_calls_and_loops() {
    let world = World::new();
    support::r1(&world);
    support::r2(&world);
    let (table, _) = project(&world);
    let now = 136_200 + 16 * 60_000;
    let report = Stall::default().analyze(&Query::new(table.facts(), Some(now)));
    assert_eq!(
        codes(&report),
        BTreeSet::from([
            ("stalled_task", "run/r2".into()),
            ("idle_gap", "r2/worker#1".into()),
            ("slow_tool_call", "tool anchor_run".into()),
            ("repeated_failure", "r2/worker#1".into()),
        ])
    );
    let gap = finding(&report, "idle_gap");
    assert!(
        gap.detail
            .contains("60 s with no model turn or tool call observed, after tool call anchor_run and before model turn 5"),
        "{}",
        gap.detail
    );
    assert!(
        finding(&report, "repeated_failure")
            .detail
            .contains("3 consecutive failed anchor_read")
    );

    // Not yet idle long enough: no task finding.
    let early = Stall::default().analyze(&Query::new(table.facts(), Some(136_200 + 60_000)));
    assert!(!early.findings.iter().any(|f| f.code == "stalled_task"));
}

#[test]
fn records_without_time_or_observations_are_reported_not_applicable() {
    // A history from before records carried time.
    let store = MemoryStore::default();
    let mut resource = Resource::new("run/old", "validator/graph", json!(null)).task(10);
    store.create(&resource).unwrap();
    resource.version = 1;
    let record: Record = serde_json::from_value(json!({
        "stamp": {"principal": {"id": "anchor/executor", "kind": "agent"}, "cost": 1},
        "submission": {"target": "run/old", "seen": 0, "action": "write", "slot": "a#1"},
        "verdict": {"effect": "accepted", "verification": "evidenced", "reason": "ok"},
        "after": resource,
    }))
    .unwrap();
    store.commit("run/old", &record).unwrap();

    let mut table = MemoryTable::default();
    Projector::default()
        .pull(&Stored::new(&store, "memory"), &mut table)
        .unwrap();
    let query = Query::new(table.facts(), Some(u64::MAX));
    let health = Health.analyze(&query);
    assert_eq!(health.not_applicable, vec![Need::Time, Need::Observations]);
    assert_eq!(health.summary["records"]["rows"], json!(1));
    assert!(!health.summary.contains_key("tools"));
    let stall = Stall::default().analyze(&query);
    assert!(stall.findings.is_empty());
}

#[test]
fn the_cli_hub_discovers_roots_and_follows_appends() {
    let world = World::new();
    support::r1(&world);
    // A directory that is no data root is not read.
    std::fs::create_dir_all(world.path().join("host/state/goose-acp")).unwrap();
    assert_eq!(
        discover(world.path()),
        vec![world.root("kernel-facts"), world.root("kernel-runs")]
    );
    let mut hub = Hub::new(vec![world.path().to_path_buf()]);
    let first = hub.refresh().unwrap();
    assert_eq!((hub.roots(), first), (2, 7));
    assert_eq!(hub.refresh().unwrap(), 0);
    support::r2(&world);
    assert_eq!(hub.refresh().unwrap(), 2);
}

#[test]
fn operators() {
    assert_eq!(
        repeats(&[1, 1, 1, 2, 2, 3, 3, 3, 3], 3, |x| *x),
        vec![0..3, 5..9]
    );
    assert_eq!(repeats::<u8, u8>(&[], 1, |x| *x), vec![]);
    let s = summarize([5, 1, 3, 2, 4, 10, 6, 7, 9, 8]).unwrap();
    assert_eq!((s.n, s.min, s.median, s.p90, s.max), (10, 1, 5, 9, 10));
    assert_eq!(summarize(Vec::<u64>::new()), None);
    assert_eq!(covered([0..10, 5..15, 20..25, 21..22]), 20);
}
