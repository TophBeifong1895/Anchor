//! Exercise the production scheduler with exactly the legacy fixture ports.
//! The existing failure checker keeps all three S18 invariants unchanged.
use super::*;
use crate::graph::test_support::snapshots::{FrozenOutcome, FrozenStatus, Population, Snapshots};
use crate::graph::{test_support::*, validator::differential::check_failed_run};

async fn compare(
    snapshot: GraphSnapshot,
    seed: u64,
    inject_failures: bool,
    frozen: &FrozenOutcome,
) -> Option<(bool, bool, usize)> {
    let record = match GraphRunRecord::create_with_id(
        snapshot,
        Value::Null,
        format!("scheduler-diff-{seed}"),
    ) {
        Ok(record) => record,
        Err(error) => {
            frozen.assert_rejection(&error);
            return None;
        }
    };
    let (old_trace, old) = frozen.accepted();
    let mut kernel = Kernel::new(MemoryStore::default(), Box::new(AuthorityFixture)).unwrap();
    kernel
        .install(validator::GRAPH_VALIDATOR, Arc::new(GraphValidator))
        .unwrap();
    let resource = validator::run_resource(&record, u64::MAX).unwrap();
    let id = resource.id.clone();
    kernel.create(resource).unwrap();
    let nodes = Scripted {
        seed,
        inject_failures,
        trace: Mutex::default(),
        calls: ScriptedCalls {
            seed,
            ..Default::default()
        },
    };
    let scheduler = KernelScheduler {
        kernel: &kernel,
        observer: None,
        boundary: None,
        artifacts: &Artifacts,
        nodes: &nodes,
        cancellation: Arc::new(AtomicBool::new(false)),
    };
    let resource = scheduler.run(&id).await.unwrap();
    let new = record_of(&resource).unwrap();
    let history = kernel.history(&id).unwrap();
    let mut new_trace = nodes.trace.lock().unwrap().clone();
    let failures = if inject_failures {
        old_trace
            .iter()
            .filter_map(|(node, invocation, _)| {
                failure_for(seed, node, *invocation)
                    .map(|reason| ((node.clone(), *invocation), reason))
            })
            .collect::<BTreeMap<_, _>>()
    } else {
        BTreeMap::new()
    };
    {
        let mut old_trace = old_trace.clone();
        old_trace.sort();
        new_trace.sort();
        assert_eq!(
            old_trace, new_trace,
            "dispatch multiset seed {seed}, failures {inject_failures}"
        );
        if old.status == FrozenStatus::Failed {
            check_failed_run(seed, &old, &new, &failures, &history);
        } else {
            assert_eq!(
                facts(&old.graph_record()),
                facts(&new),
                "Graph facts seed {seed}"
            );
            assert!(
                matches!(
                    (old.status, resource.phase()),
                    (
                        FrozenStatus::Completed,
                        Some(Phase::Settled {
                            result: Outcome::Done
                        })
                    ) | (
                        FrozenStatus::Stopped,
                        Some(Phase::Settled {
                            result: Outcome::Escalated { .. }
                        })
                    )
                ),
                "phase seed {seed}"
            );
        }
        // Every real request is charged once; stale retries carry no extra cost.
        let completed = old_trace.len() - failures.len();
        assert_eq!(
            resource.task.as_ref().unwrap().budget.spent,
            completed as u64,
            "cost seed {seed}"
        );
    }
    Some((
        !failures.is_empty(),
        !new.module_activations.is_empty(),
        history
            .iter()
            .filter(|r| r.verdict.reason == Reason::StaleVersion)
            .count(),
    ))
}

#[tokio::test]
async fn production_scheduler_matches_old_runner_on_two_thousand_serial_seeds() {
    let mut cases = 0;
    let mut failures = 0;
    let mut modules = 0;
    let mut frozen = Snapshots::open(Population::Serial);
    for seed in 0..2_000 {
        if let Some((failed, module, _)) =
            compare(random_graph(seed), seed, true, &frozen.next(seed).scheduler).await
        {
            cases += 1;
            failures += usize::from(failed);
            modules += usize::from(module);
        }
    }
    eprintln!("production scheduler serial: cases={cases}, failures={failures}, modules={modules}");
    assert!(cases > 900);
    assert!(failures > 20);
    assert!(modules > 100);
}

#[tokio::test]
async fn production_scheduler_matches_old_runner_on_one_thousand_parallel_seeds() {
    let mut cases = 0;
    let mut failures = 0;
    let mut stale = 0;
    let mut frozen = Snapshots::open(Population::ParallelFailures);
    let mut frozen_coverage = Snapshots::open(Population::ParallelNoFailures);
    for seed in 0..1_000 {
        let (failed, _, retries) = compare(
            random_parallel_graph(seed),
            seed,
            true,
            &frozen.next(seed).scheduler,
        )
        .await
        .unwrap();
        cases += 1;
        failures += usize::from(failed);
        stale += retries;
        compare(
            random_parallel_graph(seed),
            seed,
            false,
            &frozen_coverage.next(seed).scheduler,
        )
        .await
        .unwrap();
    }
    eprintln!(
        "production scheduler parallel: cases={cases}, failures={failures}, stale={stale}; no-failure cases={cases}"
    );
    assert_eq!(cases, 1_000);
    assert!(failures > 20);
    assert!(stale > 200);
}

#[tokio::test]
async fn production_scheduler_matches_old_runner_on_one_thousand_parallel_module_seeds() {
    let mut modules = 0;
    let mut stale = 0;
    let mut frozen = Snapshots::open(Population::ParallelModules);
    for seed in 0..1_000 {
        let (failed, module, retries) = compare(
            random_parallel_module_graph(seed),
            seed,
            false,
            &frozen.next(seed).scheduler,
        )
        .await
        .unwrap();
        assert!(!failed);
        modules += usize::from(module);
        stale += retries;
    }
    eprintln!(
        "production scheduler parallel modules: cases=1000, modules={modules}, stale={stale}"
    );
    assert!(modules > 200);
    assert!(stale > 200);
}

#[tokio::test]
async fn production_scheduler_matches_old_runner_on_one_thousand_call_seeds() {
    let mut waited = 0;
    let mut detached = 0;
    let mut failed = 0;
    let mut unknown = 0;
    let mut frozen = Snapshots::open(Population::Calls);
    for seed in 0..1_000 {
        let record = GraphRunRecord::create_with_id(
            random_call_graph(seed),
            serde_json::json!({}),
            format!("scheduler-calls-{seed}"),
        )
        .unwrap();
        let frozen_case = frozen.next(seed);
        let (old_trace, old) = frozen_case.scheduler.accepted();
        let mut kernel = Kernel::new(MemoryStore::default(), Box::new(AuthorityFixture)).unwrap();
        kernel
            .install(validator::GRAPH_VALIDATOR, Arc::new(GraphValidator))
            .unwrap();
        let resource = validator::run_resource(&record, u64::MAX).unwrap();
        let id = resource.id.clone();
        kernel.create(resource).unwrap();
        let nodes = Scripted {
            seed,
            inject_failures: false,
            trace: Mutex::default(),
            calls: ScriptedCalls {
                seed,
                ..Default::default()
            },
        };
        let scheduler = KernelScheduler {
            kernel: &kernel,
            observer: None,
            boundary: None,
            artifacts: &Artifacts,
            nodes: &nodes,
            cancellation: Arc::new(AtomicBool::new(false)),
        };
        let resource = scheduler.run(&id).await.unwrap();
        let new = record_of(&resource).unwrap();
        let mut new_trace = nodes.trace.lock().unwrap().clone();
        {
            let mut old_trace = old_trace.clone();
            old_trace.sort();
            new_trace.sort();
            assert_eq!(old_trace, new_trace, "call dispatch multiset seed {seed}");
            assert_eq!(
                facts(&old.graph_record()),
                facts(&new),
                "call Graph facts seed {seed}"
            );
            assert!(
                matches!(
                    (old.status, resource.phase()),
                    (
                        FrozenStatus::Completed,
                        Some(Phase::Settled {
                            result: Outcome::Done
                        })
                    ) | (
                        FrozenStatus::Stopped,
                        Some(Phase::Settled {
                            result: Outcome::Escalated { .. }
                        })
                    ) | (
                        FrozenStatus::Failed,
                        Some(Phase::Settled {
                            result: Outcome::Failed { .. } | Outcome::Unknown { .. }
                        })
                    )
                ),
                "call phase seed {seed}"
            );
            assert_eq!(
                resource.task.as_ref().unwrap().budget.spent,
                old_trace.len() as u64
            );
        }
        waited += usize::from(!nodes.calls.waiting.lock().unwrap().is_empty());
        detached += usize::from(
            new.graph_calls
                .values()
                .any(|c| c.status == GraphCallStatus::Detached),
        );
        failed += usize::from(
            new.graph_calls
                .values()
                .any(|c| c.status == GraphCallStatus::Failed),
        );
        unknown += usize::from(
            new.graph_calls
                .values()
                .any(|c| c.status == GraphCallStatus::Uncertain),
        );
    }
    eprintln!(
        "production scheduler calls: cases=1000, waited={waited}, detached={detached}, failed={failed}, unknown={unknown}"
    );
    assert!(waited > 100 && detached > 100 && failed > 20 && unknown > 20);
}
