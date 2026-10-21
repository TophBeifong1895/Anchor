mod support;

use std::collections::BTreeSet;

use anchor_hub::adapters::{Hub, discover};
use anchor_hub::analyzers::{
    Analyzer, Finding, Need, Report, contrast::Contrast, health::Health, stall::Stall,
};
use anchor_hub::query::{
    Activity, GapPlace, Query, Status, covered, mann_whitney, node_result, normal_cdf, repeats,
    summarize, two_proportions,
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
    assert_eq!(report.not_applicable, vec![Need::Grades]);
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
    assert_eq!(
        health.not_applicable,
        vec![Need::Time, Need::Observations, Need::Grades]
    );
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
        vec![
            world.root("kernel-facts"),
            world.root("kernel-grades"),
            world.root("kernel-runs")
        ]
    );
    let mut hub = Hub::new(vec![world.path().to_path_buf()]);
    let first = hub.refresh().unwrap();
    assert_eq!((hub.roots(), first), (3, 7));
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
    assert!((s.sd - 3.0277).abs() < 1e-3, "{}", s.sd);
    assert_eq!(summarize([7]).unwrap().sd, 0.0);
    assert_eq!(summarize(Vec::<u64>::new()), None);
    assert_eq!(covered([0..10, 5..15, 20..25, 21..22]), 20);
}

#[test]
fn grades_join_their_runs_and_failing_checks_are_reported() {
    let world = World::new();
    support::r1(&world);
    support::r2(&world);
    world.run("r3", 300_000);
    world.result("r3", "worker#1", 300_100, 1, true);
    world.grade(
        "r1",
        "ledger",
        &[("unpaid", true), ("total", false)],
        400_000,
    );
    world.grade(
        "r2",
        "ledger",
        &[("unpaid", false), ("total", false)],
        400_001,
    );
    // Regraded: only the latest grade of a Run counts.
    world.grade(
        "r3",
        "ledger",
        &[("unpaid", false), ("total", false)],
        400_002,
    );
    world.grade(
        "r3",
        "ledger",
        &[("unpaid", true), ("total", true)],
        400_003,
    );
    let (mut table, mut projector) = project(&world);
    let grades = DataRoot::open(&world.root("kernel-grades"), "kernel-grades").unwrap();
    projector.pull(&grades, &mut table).unwrap();

    let query = Query::new(table.facts(), None);
    let grades = query.grades();
    assert_eq!(grades.len(), 3);
    assert!(grades["r3"].grade.passed());
    assert_eq!(grades["r3"].row.key.place, 2);

    let report = Health.analyze(&query);
    assert!(report.not_applicable.is_empty());
    let quality = &report.summary["quality"]["ledger v1"];
    assert_eq!(quality["runs"], json!(3));
    assert_eq!(quality["pass_rate"], json!(0.333));
    assert_eq!(quality["mean_score"], json!(0.5));
    assert_eq!(quality["checks"]["total"], json!(0.333));
    assert_eq!(quality["checks"]["unpaid"], json!(0.667));
    assert_eq!(quality["tokens"]["n"], json!(3));
    // r1 also observed its ghost invocation, under its own configuration.
    assert_eq!(quality["configurations"], json!(2));
    // Only `total` fails in at least half the Runs, once r3's regrade counts.
    let failing: Vec<_> = report
        .findings
        .iter()
        .filter(|f| f.code == "check_failing")
        .collect();
    assert_eq!(failing.len(), 1, "{failing:#?}");
    assert_eq!(failing[0].subject, "task ledger v1");
    assert!(
        failing[0].detail.contains("check total failed in 2 of 3"),
        "{}",
        failing[0].detail
    );
    assert_eq!(failing[0].evidence.len(), 2);
}

#[test]
fn rank_and_proportion_tests_match_known_values() {
    assert!((normal_cdf(1.96) - 0.975).abs() < 1e-4);
    assert!((normal_cdf(-1.0) - 0.158_655).abs() < 1e-5);
    let separated = mann_whitney(&[1.0, 2.0, 3.0], &[4.0, 5.0, 6.0]).unwrap();
    assert_eq!(separated.u, 0.0);
    assert!((separated.p - 0.0495).abs() < 1e-3, "{separated:?}");
    let same = mann_whitney(&[1.0, 2.0, 3.0], &[1.0, 2.0, 3.0]).unwrap();
    assert!((same.p - 1.0).abs() < 1e-6);
    let tied = mann_whitney(&[5.0, 5.0], &[5.0, 5.0]).unwrap();
    assert_eq!(tied.p, 1.0);
    assert_eq!(mann_whitney(&[], &[1.0]), None);
    let p = two_proportions(10, 100, 30, 100).unwrap();
    assert!((p - 0.0004).abs() < 1e-4, "{p}");
    assert_eq!(two_proportions(0, 10, 0, 10), Some(1.0));
}

/// Two harnesses on one task: the variant halves agent time and tool
/// failures, and loses quality on one Run.
fn experiment(world: &World) {
    let mut variant = support::configuration("worker");
    variant.host_sha256 = "host-b".into();
    for n in 0..6u64 {
        let (arm, run) = if n % 2 == 0 {
            (support::configuration("worker"), format!("a{n}"))
        } else {
            (variant.clone(), format!("b{n}"))
        };
        let start = 1_000_000 + n * 100_000;
        let length = if n % 2 == 0 {
            10_000 + n * 100
        } else {
            5_000 + n * 100
        };
        world.run(&run, start - 10);
        let failed = (n % 2 == 0).then_some(anchor_facts::Failure::InvalidArguments);
        world.observe(
            &run,
            "worker",
            start + length + 5,
            &arm,
            vec![
                support::turn(1, start + 1_000, 900),
                support::call(
                    "anchor_edit",
                    1,
                    json!({"path": "x"}),
                    (start + 1_000, start + 1_010),
                    failed,
                ),
                support::turn(2, start + 2_000, 900),
                support::call(
                    "final_result",
                    2,
                    json!({}),
                    (start + 2_000, start + 2_001),
                    None,
                ),
                support::lifecycle(
                    (start, start + length),
                    anchor_facts::Outcome::Completed,
                    2,
                    2,
                ),
            ],
        );
        world.result(&run, "worker#1", start + length + 10, 2, true);
        let passed = n != 5;
        world.grade(&run, "ledger", &[("total", passed)], start + length + 20);
    }
}

#[test]
fn contrast_compares_arms_on_quality_first_then_cost() {
    let world = World::new();
    experiment(&world);
    let mut table = MemoryTable::default();
    let mut projector = Projector::default();
    for name in ["kernel-runs", "kernel-facts", "kernel-grades"] {
        projector
            .pull(
                &DataRoot::open(&world.root(name), name).unwrap(),
                &mut table,
            )
            .unwrap();
    }
    let report = Contrast::default().analyze(&Query::new(table.facts(), None));
    assert!(report.not_applicable.is_empty());
    assert_eq!(report.summary["arms"], json!(["host", "host-b"]));
    let task = &report.summary["tasks"]["ledger v1"];
    assert_eq!(task["arms"]["host"]["runs"], json!(3));
    assert_eq!(task["arms"]["host-b"]["mean_score"], json!(0.667));
    let against = &task["against_reference"]["host-b"];
    assert_eq!(against["score_change"], json!(-0.333));
    let ratio = against["agent_ms_ratio"].as_f64().unwrap();
    assert!((ratio - 5_300.0 / 10_200.0).abs() < 0.002, "{ratio}");
    assert!(against["agent_ms_p"].as_f64().unwrap() < 0.1);
    let overall = &report.summary["overall"];
    assert_eq!(overall["host"]["tool_failure_rate"], json!(0.5));
    assert_eq!(overall["host-b"]["tool_failure_rate"], json!(0.0));
    assert!(overall["host-b"]["agent_ms_relative"].as_f64().unwrap() < 0.55);
    let loss = finding(&report, "contrast_quality_loss");
    assert_eq!(loss.subject, "ledger v1: host-b");
    assert_eq!(loss.evidence.len(), 3);

    // The reference may be named; the other arm is then compared to it.
    let named = Contrast {
        reference: Some("host-b".into()),
        ..Contrast::default()
    }
    .analyze(&Query::new(table.facts(), None));
    assert_eq!(named.summary["arms"], json!(["host-b", "host"]));
    let back = &named.summary["tasks"]["ledger v1"]["against_reference"]["host"];
    assert_eq!(back["score_change"], json!(0.333));
    assert!(
        named
            .findings
            .iter()
            .all(|f| f.code != "contrast_quality_loss")
    );

    // Another field names other arms: here every Run shares one.
    let by_model = Contrast {
        by: "model".into(),
        reference: None,
    }
    .analyze(&Query::new(table.facts(), None));
    assert_eq!(by_model.summary["arms"], json!(["flash"]));
}
