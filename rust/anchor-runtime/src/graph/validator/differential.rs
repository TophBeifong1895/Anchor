//! Differential test: the old push-model `GraphRunner` against the kernel with
//! `GraphValidator`, on thousands of random Graphs and route scripts.
//!
//! Both sides see the same Graph and the same deterministic route choices.
//! They must dispatch the same node invocations, in the same order, with the
//! same committed inputs, and end with the same Graph facts.

use std::sync::Arc;

use anchor_kernel::{
    Action, Authority, Evidence, Kernel, MemoryStore, Outcome, Phase, Principal, PrincipalKind,
    Reason, Submission, Waiter,
};

use super::super::*;
use super::{
    CallSubmission, DispatchKind, GRAPH_VALIDATOR, GraphValidator, NodeSubmission,
    SettleSubmission, call_identity, record_of, run_resource,
};

use super::super::test_support::*;

// ---------------------------------------------------------------------------
// New side: kernel + GraphValidator + a trivial scheduler
// ---------------------------------------------------------------------------

struct AllowAll;
impl Authority for AllowAll {
    fn permits(&self, _: &Principal, _: Action, _: &str) -> bool {
        true
    }
}

/// Waves: take every slot open now, submit each, then look again. This
/// mirrors the old Runner's barrier so the two can be compared step by step.
/// Parallel submissions to one Run race on its version; a stale submission is
/// returned and simply resubmitted against the current state.
fn run_new(record: GraphRunRecord, seed: u64, inject_failures: bool) -> NewRun {
    let mut kernel = Kernel::new(MemoryStore::default(), Box::new(AllowAll)).unwrap();
    kernel
        .install(GRAPH_VALIDATOR, Arc::new(GraphValidator))
        .unwrap();
    let resource = run_resource(&record, u64::MAX).unwrap();
    let id = resource.id.clone();
    kernel.create(resource).unwrap();
    let agent = Principal::new("executor", PrincipalKind::Agent);
    let host = Principal::new("host", PrincipalKind::Code);
    let mut trace = Trace::new();
    let mut resubmissions = 0;
    let mut late = BTreeSet::new();
    for _ in 0..10_000 {
        let resource = kernel.load(&id).unwrap().unwrap();
        if resource.is_settled() {
            break;
        }
        if let Some(Phase::Waiting {
            waiter: Waiter::Child { .. },
        }) = resource.phase()
        {
            // The child settled: host code releases the parent (kernel law).
            let resume = Submission::new(&id, resource.version, Action::Request)
                .content(serde_json::json!({"control": "resume"}));
            assert!(kernel.submit(&host, 0, resume).unwrap().is_accepted());
            continue;
        }
        let wave = GraphValidator::frontier(&resource).unwrap();
        let open = kernel.open(&id).unwrap();
        assert_eq!(
            open.iter().map(|s| s.id.clone()).collect::<Vec<_>>(),
            wave.iter().map(|d| d.slot()).collect::<Vec<_>>()
        );
        if wave.is_empty() {
            break;
        }
        let state = record_of(&resource).unwrap();
        for dispatch in wave {
            let node = dispatch.node.id.clone();
            let mut principal = if dispatch.kind == DispatchKind::Work {
                trace.push((
                    node.clone(),
                    dispatch.invocation,
                    dispatch.inputs.iter().map(|c| c.id.clone()).collect(),
                ));
                &agent
            } else {
                &host
            };
            let routes = state
                .snapshot
                .edges
                .iter()
                .filter(|e| e.from_node == node)
                .map(|e| e.to_node.clone())
                .collect::<Vec<_>>();
            let commit = commit_of(&InvocationKey {
                run_id: state.run_id.clone(),
                graph_digest: state.graph_digest.clone(),
                node_id: node.clone(),
                invocation: dispatch.invocation,
            });
            let route = match dispatch.kind {
                DispatchKind::Work => route_for(seed, &node, dispatch.invocation, &routes),
                _ => None,
            };
            let content = if dispatch.kind == DispatchKind::Call {
                let key = call_identity(&state, &dispatch).unwrap().durable_key();
                let mode = state.snapshot.ops[dispatch.node.op.as_ref().unwrap()]["call"]["mode"]
                    .as_str()
                    .unwrap()
                    .to_owned();
                let child = child_of(&key);
                let bound = state.graph_calls.contains_key(&key);
                serde_json::to_value(match (call_script(&key, &mode, seed), bound) {
                    (CallScript::WaitThenComplete, false) => CallSubmission::Bound {
                        child_run_id: child,
                    },
                    (CallScript::WaitThenComplete | CallScript::CompleteAtOnce, _) => {
                        CallSubmission::Completed {
                            child_run_id: child,
                            // For a detach this carries the child's business
                            // result; the validator must not let it through.
                            output: child_output(&key, seed),
                            commit: commit.clone(),
                        }
                    }
                    (CallScript::Detached, _) => CallSubmission::Completed {
                        child_run_id: child,
                        output: Value::Null,
                        commit: commit.clone(),
                    },
                    (CallScript::Failed, _) => CallSubmission::Failed {
                        child_run_id: Some(child),
                        reason: "child refused".into(),
                    },
                    (CallScript::Uncertain, _) => CallSubmission::Uncertain {
                        child_run_id: None,
                        reason: "admission unknown".into(),
                    },
                })
                .unwrap()
            } else {
                serde_json::to_value(NodeSubmission {
                    summary: format!("{node} #{}", dispatch.invocation),
                    interruption: None,
                    route,
                    output: serde_json::json!({"node": node, "invocation": dispatch.invocation}),
                    model_requests: 1,
                    commit: commit.clone(),
                    plugin_views: BTreeMap::new(),
                    plugin_digests: BTreeMap::new(),
                })
                .unwrap()
            };
            let failure = (inject_failures
                && dispatch.kind == DispatchKind::Work
                && !state.snapshot.ops.contains_key("callw"))
            .then(|| failure_for(seed, &node, dispatch.invocation))
            .flatten();
            let (action, content) = if let Some(reason) = failure {
                principal = &host;
                (
                    Action::Settle,
                    serde_json::to_value(SettleSubmission { reason }).unwrap(),
                )
            } else {
                (Action::Write, content)
            };
            // An executor submits against the version it was dispatched at;
            // only after a stale return does it re-read.
            let mut version = resource.version;
            loop {
                let verdict = kernel
                    .submit(
                        principal,
                        1,
                        Submission::new(&id, version, action)
                            .slot(dispatch.slot())
                            .content(content.clone())
                            .evidence(Evidence::Resource {
                                id: commit.id.clone(),
                                version: 0,
                            }),
                    )
                    .unwrap();
                if verdict.reason == Reason::StaleVersion {
                    resubmissions += 1;
                    version = kernel.load(&id).unwrap().unwrap().version;
                    continue;
                }
                // S6: a sibling finishing after the Run failed is refused and
                // stays in the Run's history; it is not merged into results.
                if verdict.reason == Reason::TaskSettled {
                    late.insert((node.clone(), dispatch.invocation));
                    break;
                }
                assert!(verdict.is_accepted(), "{node}: {verdict:?}");
                break;
            }
        }
    }
    let resource = kernel.load(&id).unwrap().unwrap();
    // Every late refusal must be in the Run's history with its evidence.
    let history = kernel.history(&id).unwrap();
    let refused_in_history = history
        .iter()
        .filter(|r| r.verdict.reason == Reason::TaskSettled && !r.submission.evidence.is_empty())
        .count();
    assert_eq!(refused_in_history, late.len());
    NewRun {
        trace,
        record: record_of(&resource).unwrap(),
        phase: resource.phase().cloned().unwrap(),
        resubmissions,
        late,
        history,
    }
}

struct NewRun {
    trace: Trace,
    record: GraphRunRecord,
    phase: Phase,
    resubmissions: u64,
    late: BTreeSet<(String, u64)>,
    history: Vec<anchor_kernel::Record>,
}

fn result_set(record: &GraphRunRecord) -> BTreeSet<(String, u64)> {
    record
        .results
        .values()
        .flatten()
        .map(|r| (r.node_id.clone(), r.key.invocation))
        .collect()
}

/// S18: the first terminal fact wins; every executed fact occurs exactly
/// once, either in state or in the TaskSettled history. Compare the remaining
/// facts after accounting for the documented late wave and ceiling effects.
fn check_failed_run(
    seed: u64,
    old: &GraphRunRecord,
    new: &GraphRunRecord,
    failures: &BTreeMap<(String, u64), String>,
    history: &[anchor_kernel::Record],
) -> (u64, bool) {
    let terminal_index = history
        .iter()
        .position(|r| r.after.as_ref().is_some_and(|a| a.is_settled()))
        .expect("a failed Run has an accepted terminal fact");
    let terminal = &history[terminal_index];
    let Phase::Settled {
        result:
            Outcome::Failed {
                reason: Reason::Unclassified,
                detail,
            },
    } = terminal.after.as_ref().unwrap().phase().unwrap()
    else {
        panic!("not a failed settlement, seed {seed}")
    };
    let prior = history[..terminal_index]
        .iter()
        .rev()
        .find_map(|r| r.after.as_ref())
        .map(|r| record_of(r).unwrap());
    let ceiling = new.snapshot.nodes.iter().find(|node| {
        node.max_rounds.is_some_and(|limit| {
            detail
                == &format!(
                    "parallel branch `{}` exceeded node round ceiling `{limit}`",
                    node.id
                )
        })
    });
    if let Some(node) = ceiling {
        let limit = node.max_rounds.unwrap();
        assert!(
            old.passes.get(&node.id).copied().unwrap_or(0) >= u64::from(limit),
            "unjustified ceiling, seed {seed}"
        );
        let known_ceiling = old.ceased.contains(&format!("{}@{limit}", node.id));
        let successor = old.parallel.as_ref().unwrap().branches.iter().any(|b| {
            b.nodes.get(b.next_index) == Some(&node.id)
                && b.next_index > 0
                && b.completed
                    .last()
                    .is_some_and(|c| c.node_id == b.nodes[b.next_index - 1])
        });
        assert!(
            known_ceiling || successor,
            "ceiling must follow a completed branch invocation, seed {seed}"
        );
        assert_eq!(terminal.submission.action, Action::Write);
    } else {
        assert!(
            failures
                .iter()
                .any(|((node, _), reason)| detail == &format!("{node} failed: {reason}")),
            "unjustified failure reason, seed {seed}: {detail}"
        );
        assert_eq!(terminal.submission.action, Action::Settle);
    }
    let new_results = result_set(new);
    let old_work = old
        .results
        .values()
        .flatten()
        .filter(|r| {
            old.snapshot
                .nodes
                .iter()
                .any(|n| n.id == r.node_id && n.agent.is_some())
        })
        .map(|r| (r.node_id.clone(), r.key.invocation))
        .collect::<BTreeSet<_>>();
    let mut seen_results = BTreeSet::new();
    let mut seen_failures = BTreeSet::new();
    for report in history
        .iter()
        .filter(|r| r.verdict.reason != Reason::StaleVersion)
    {
        let slot = report.submission.slot.as_deref().expect("dispatch slot");
        let (node, invocation) = slot.rsplit_once('#').unwrap();
        let key = (node.to_owned(), invocation.parse::<u64>().unwrap());
        assert!(
            report.verdict.is_accepted() || report.verdict.reason == Reason::TaskSettled,
            "unexpected verdict, seed {seed}"
        );
        match report.submission.action {
            Action::Settle => {
                assert!(
                    seen_failures.insert(key.clone()),
                    "duplicate failure, seed {seed}"
                );
                assert_eq!(report.stamp.principal().kind, PrincipalKind::Code);
                let reason = failures.get(&key).expect("invented failure");
                assert_eq!(
                    report.submission.content,
                    serde_json::json!({"reason":reason})
                );
                assert_eq!(report.verdict.is_accepted(), std::ptr::eq(report, terminal));
                if let Some(activation) = &old.parallel {
                    let index = activation
                        .branches
                        .iter()
                        .position(|b| b.nodes.get(b.next_index).map(String::as_str) == Some(node))
                        .unwrap();
                    let old_branch = &activation.branches[index];
                    let new_branch = &new.parallel.as_ref().unwrap().branches[index];
                    assert_eq!(old_branch.status, ParallelBranchStatus::Failed);
                    assert_eq!(old_branch.error.as_deref(), Some(reason.as_str()));
                    assert_eq!(
                        new_branch.status,
                        if report.verdict.is_accepted() {
                            ParallelBranchStatus::Failed
                        } else {
                            ParallelBranchStatus::Ready
                        }
                    );
                    assert_eq!(
                        new_branch.error.as_deref(),
                        report.verdict.is_accepted().then_some(reason.as_str())
                    );
                }
            }
            Action::Write if old_work.contains(&key) => {
                assert!(
                    seen_results.insert(key.clone()),
                    "duplicate Work result, seed {seed}"
                );
                assert_eq!(report.stamp.principal().kind, PrincipalKind::Agent);
                let content: NodeSubmission =
                    serde_json::from_value(report.submission.content.clone()).unwrap();
                assert_eq!(
                    (&content.commit.node_id, content.commit.invocation),
                    (&key.0, key.1)
                );
                assert!(report.submission.evidence.contains(&Evidence::Resource {
                    id: content.commit.id.clone(),
                    version: 0
                }));
                assert_eq!(
                    new_results.contains(&key),
                    report.verdict.is_accepted(),
                    "state/history must be disjoint, seed {seed}"
                );
            }
            Action::Write => {
                assert!(
                    old.snapshot
                        .nodes
                        .iter()
                        .any(|n| n.id == node && n.agent.is_none()),
                    "invented Work result, seed {seed}"
                );
                assert!(report.verdict.is_accepted());
            }
            other => panic!("unexpected action {other:?}, seed {seed}"),
        }
    }
    assert_eq!(seen_results, old_work, "lost Work result, seed {seed}");
    assert_eq!(
        seen_failures,
        failures.keys().cloned().collect(),
        "lost failure, seed {seed}"
    );

    let mut accounted = new.clone();
    let mut expected = old.clone();
    // Remove only the final look-ahead's exact ceased/edge writes. Restore
    // overwritten decisions from the preceding accepted state, not defaults.
    for record in [&mut accounted, &mut expected] {
        let baseline = prior
            .as_ref()
            .map(|r| &r.ceased)
            .cloned()
            .unwrap_or_default();
        let added = record
            .ceased
            .difference(&baseline)
            .cloned()
            .collect::<Vec<_>>();
        assert!(
            added.len() <= 1,
            "more than one terminal ceiling, seed {seed}"
        );
        for ceased in added {
            let (node, limit) = ceased.rsplit_once('@').unwrap();
            let limit = limit.parse::<u32>().unwrap();
            assert!(
                record
                    .snapshot
                    .nodes
                    .iter()
                    .any(|n| n.id == node && n.max_rounds == Some(limit))
            );
            assert!(record.passes[node] >= u64::from(limit));
            for edge in record.snapshot.edges.iter().filter(|e| e.from_node == node) {
                let key = edge_key(node, &edge.to_node);
                let decision = record.decided.remove(&key).unwrap();
                assert!(!decision.selected);
                assert_eq!(decision.sequence, record.sequence);
                assert_eq!(decision.result_sequence, 0);
                assert_eq!(
                    decision.source_invocation,
                    record.invocations.get(node).copied().unwrap_or(0)
                );
                if let Some(previous) = prior.as_ref().and_then(|r| r.decided.get(&key)) {
                    record.decided.insert(key, previous.clone());
                }
            }
            record.ceased.remove(&ceased);
            record.sequence -= 1;
            if let Some(activation) = &mut record.parallel {
                for branch in &mut activation.branches {
                    if branch.nodes.get(branch.next_index).map(String::as_str) == Some(node)
                        && branch.status == ParallelBranchStatus::Failed
                    {
                        assert_eq!(
                            branch.error.as_deref(),
                            Some(
                                format!(
                                    "parallel branch `{node}` exceeded node round ceiling `{limit}`"
                                )
                                .as_str()
                            )
                        );
                        branch.status = ParallelBranchStatus::Ready;
                        branch.error = None;
                    }
                }
            }
        }
    }
    // Replay only rejected facts into the comparison copy, never the Kernel.
    for report in history
        .iter()
        .filter(|r| r.verdict.reason == Reason::TaskSettled)
    {
        let (node, invocation) = report
            .submission
            .slot
            .as_deref()
            .unwrap()
            .rsplit_once('#')
            .unwrap();
        let invocation = invocation.parse::<u64>().unwrap();
        assert_eq!(
            accounted.invocations.get(node).copied().unwrap_or(0) + 1,
            invocation
        );
        accounted.invocations.insert(node.into(), invocation);
        *accounted.passes.entry(node.into()).or_default() += 1;
        let index = accounted
            .parallel
            .as_ref()
            .unwrap()
            .branches
            .iter()
            .position(|b| b.nodes.get(b.next_index).map(String::as_str) == Some(node))
            .unwrap();
        if report.submission.action == Action::Settle {
            let branch = &mut accounted.parallel.as_mut().unwrap().branches[index];
            branch.status = ParallelBranchStatus::Failed;
            branch.error = Some(failures[&(node.to_owned(), invocation)].clone());
        } else {
            let content: NodeSubmission =
                serde_json::from_value(report.submission.content.clone()).unwrap();
            let dispatch = super::Dispatch {
                node: accounted
                    .snapshot
                    .nodes
                    .iter()
                    .find(|n| n.id == node)
                    .unwrap()
                    .clone(),
                invocation,
                kind: DispatchKind::Work,
                branch: Some(index),
                inputs: vec![],
            };
            let routes = super::routes_of(&accounted, node);
            assert_eq!(routes.len(), 1);
            super::record_result(
                &mut accounted,
                &dispatch,
                NodeCompletion {
                    submission: content.summary,
                    route: content.route,
                    output: content.output,
                    model_requests: content.model_requests,
                },
                content.commit.clone(),
                (
                    content.plugin_views,
                    content.plugin_digests,
                    content.interruption,
                ),
                &routes,
                routes.first(),
            );
            let branch = &mut accounted.parallel.as_mut().unwrap().branches[index];
            branch.completed.push(content.commit);
            branch.next_index += 1;
            branch.status = if branch.next_index == branch.nodes.len() {
                ParallelBranchStatus::Completed
            } else {
                ParallelBranchStatus::Ready
            };
        }
    }
    // S6: cursors prepared in an unexecuted next wave are not invocations.
    // A real failed invocation is counted, even though it has no result.
    let mut phantoms = 0;
    for (node, count) in expected.invocations.clone() {
        let actual = expected
            .results
            .get(&node)
            .and_then(|r| r.last())
            .map(|r| r.key.invocation)
            .unwrap_or(0)
            .max(
                failures
                    .keys()
                    .filter(|(n, _)| n == &node)
                    .map(|(_, i)| *i)
                    .max()
                    .unwrap_or(0),
            );
        if count > actual {
            phantoms += count - actual;
            *expected.invocations.get_mut(&node).unwrap() = actual;
            *expected.passes.get_mut(&node).unwrap() -= count - actual;
            let branch = expected
                .parallel
                .as_mut()
                .unwrap()
                .branches
                .iter_mut()
                .find(|b| {
                    b.cursor
                        .as_ref()
                        .is_some_and(|c| c.node_id == node && c.key.invocation == count)
                })
                .expect("phantom must have an unexecuted cursor");
            branch.cursor = None;
            branch.status = ParallelBranchStatus::Ready;
        }
    }
    expected.invocations.retain(|_, v| *v > 0);
    expected.passes.retain(|_, v| *v > 0);
    assert_eq!(
        facts(&expected),
        facts(&accounted),
        "all accounted Graph facts, seed {seed}"
    );
    for node in old.snapshot.nodes.iter().filter(|n| n.agent.is_some()) {
        assert_eq!(
            expected.results.get(&node.id),
            accounted.results.get(&node.id),
            "full Work results, seed {seed}"
        );
    }
    assert_eq!(
        expected.parallel, accounted.parallel,
        "all accounted branch facts, seed {seed}"
    );
    (phantoms, ceiling.is_some() && !failures.is_empty())
}

#[derive(Default, Debug)]
struct Tally {
    cases: u64,
    rejected_graphs: u64,
    completed: u64,
    round_limited: u64,
    dispatches: u64,
    longest: usize,
    module_activated: u64,
    module_limited: u64,
    skipped_paths: u64,
    failed: u64,
    node_failed: u64,
    multi_failure_waves: u64,
    ceiling_preempted_failures: u64,
    parallel_activations: u64,
    resubmissions: u64,
    phantom_invocations: u64,
    late_refusals: u64,
    calls_waited: u64,
    calls_detached: u64,
    call_failed: u64,
    call_unknown: u64,
}

async fn check(snapshot: GraphSnapshot, seed: u64, tally: &mut Tally) {
    check_with_failures(snapshot, seed, tally, true).await;
}

async fn check_with_failures(
    snapshot: GraphSnapshot,
    seed: u64,
    tally: &mut Tally,
    inject_failures: bool,
) {
    let Ok(record) =
        GraphRunRecord::create_with_id(snapshot.clone(), serde_json::json!({}), format!("r{seed}"))
    else {
        tally.rejected_graphs += 1;
        return;
    };
    let (old_trace, old) = run_old(record.clone(), seed, inject_failures).await;
    let NewRun {
        trace: new_trace,
        record: new,
        phase,
        resubmissions,
        late,
        history,
    } = run_new(record, seed, inject_failures);
    assert_eq!(
        old_trace, new_trace,
        "dispatch trace differs, seed {seed}, graph {snapshot:#?}"
    );
    let call_ended_it = old.graph_calls.values().any(|c| {
        matches!(
            c.status,
            GraphCallStatus::Failed | GraphCallStatus::Uncertain
        )
    });
    // Recompute every actual failed invocation, including failures whose
    // error text the old wave subsequently overwrote (S17).
    let failures = if !inject_failures || snapshot.ops.contains_key("callw") {
        BTreeMap::new()
    } else {
        old_trace
            .iter()
            .filter_map(|(node, invocation, _)| {
                failure_for(seed, node, *invocation)
                    .map(|reason| ((node.clone(), *invocation), reason))
            })
            .collect::<BTreeMap<_, _>>()
    };
    let node_failed = !failures.is_empty();
    if node_failed {
        tally.node_failed += 1;
        tally.multi_failure_waves += u64::from(failures.len() > 1);
    }
    if old.status == RunStatus::Failed && !call_ended_it {
        let (phantoms, ceiling_preempted) = check_failed_run(seed, &old, &new, &failures, &history);
        tally.phantom_invocations += phantoms;
        tally.ceiling_preempted_failures += u64::from(ceiling_preempted);
        tally.failed += u64::from(!node_failed);
        tally.late_refusals += late.len() as u64;
    } else {
        assert!(late.is_empty(), "seed {seed}");
        assert_eq!(facts(&old), facts(&new), "Graph facts differ, seed {seed}");
        match (old.status, &phase) {
            (
                RunStatus::Completed,
                Phase::Settled {
                    result: Outcome::Done,
                },
            ) => tally.completed += 1,
            (
                RunStatus::Stopped,
                Phase::Settled {
                    result: Outcome::Escalated { .. },
                },
            ) if !old.ceased.is_empty() => tally.round_limited += 1,
            (
                RunStatus::Failed,
                Phase::Settled {
                    result: Outcome::Failed { .. },
                },
            ) if node_failed => {}
            (
                RunStatus::Failed,
                Phase::Settled {
                    result: Outcome::Failed { .. },
                },
            ) if call_ended_it => tally.call_failed += 1,
            // S7: an unknown admission settles as unknown, not as failed.
            (
                RunStatus::Failed,
                Phase::Settled {
                    result: Outcome::Unknown { .. },
                },
            ) if call_ended_it => tally.call_unknown += 1,
            other => panic!("terminal state differs, seed {seed}: {other:?}"),
        }
    }
    tally.cases += 1;
    tally.resubmissions += resubmissions;
    for call in new.graph_calls.values() {
        match call.status {
            GraphCallStatus::Completed => tally.calls_waited += 1,
            GraphCallStatus::Detached => {
                tally.calls_detached += 1;
                assert!(
                    call.output
                        .as_ref()
                        .is_some_and(|o| o.get("secret").is_none()),
                    "a detach leaked the child's result, seed {seed}"
                );
            }
            _ => {}
        }
    }
    tally.parallel_activations += new.results.get("f").map(|r| r.len() as u64).unwrap_or(0);
    tally.module_activated += u64::from(!new.module_activations.is_empty());
    tally.module_limited += u64::from(new.ceased.iter().any(|c| c.starts_with("m@")));
    tally.skipped_paths += u64::from(
        new.snapshot
            .nodes
            .iter()
            .any(|n| !new.invocations.contains_key(&n.id)),
    );
    tally.dispatches += new_trace.len() as u64;
    tally.longest = tally.longest.max(new_trace.len());
}

#[tokio::test]
async fn kernel_with_graph_validator_matches_the_runner_on_random_graphs() {
    let mut tally = Tally::default();
    for seed in 0..10_000 {
        check(random_graph(seed), seed, &mut tally).await;
    }
    eprintln!("differential: {tally:?}");
    assert!(tally.node_failed > 100, "{tally:?}");
    assert!(tally.cases > 9_000, "{tally:?}");
    assert!(
        tally.module_activated > 500 && tally.module_limited > 50 && tally.skipped_paths > 1000,
        "{tally:?}"
    );
    assert!(
        tally.completed > 100 && tally.round_limited > 100,
        "both endings exercised: {tally:?}"
    );
}

#[tokio::test]
async fn parallel_regions_match_the_runner_on_random_graphs() {
    let mut tally = Tally::default();
    let mut coverage = Tally::default();
    for seed in 0..5_000 {
        check(random_parallel_graph(seed), seed, &mut tally).await;
        // Keep the original no-failure coverage population and its thresholds.
        // Injected failures intentionally terminate some later activations.
        check_with_failures(random_parallel_graph(seed), seed, &mut coverage, false).await;
    }
    eprintln!("differential parallel: {tally:?}");
    eprintln!("differential parallel original coverage: {coverage:?}");
    assert_eq!(coverage.node_failed, 0);
    assert_eq!(coverage.cases, 5_000);
    assert!(tally.node_failed > 100, "{tally:?}");
    assert!(tally.multi_failure_waves > 0, "{tally:?}");
    assert!(tally.ceiling_preempted_failures > 0, "{tally:?}");
    assert!(tally.cases > 4_500, "{tally:?}");
    assert!(
        coverage.parallel_activations > 5_000
            && coverage.resubmissions > 1_000
            && coverage.phantom_invocations > 0
            && coverage.late_refusals > 0,
        "{coverage:?}"
    );
    assert!(
        tally.resubmissions > 1_000 && tally.phantom_invocations > 0 && tally.late_refusals > 0,
        "{tally:?}"
    );
    assert!(
        tally.completed > 100 && tally.failed > 100,
        "completed and branch-ceiling failures exercised: {tally:?}"
    );
}

#[tokio::test]
async fn graph_calls_match_the_runner_on_random_graphs() {
    let mut tally = Tally::default();
    for seed in 0..5_000 {
        check(random_call_graph(seed), seed, &mut tally).await;
    }
    eprintln!("differential calls: {tally:?}");
    assert!(tally.cases > 4_500, "{tally:?}");
    assert!(
        tally.calls_waited > 500
            && tally.calls_detached > 500
            && tally.call_failed > 100
            && tally.call_unknown > 100,
        "{tally:?}"
    );
}

#[tokio::test]
async fn same_wave_node_failures_keep_first_reason_and_record_late_failure() {
    let seed = 495;
    let mut tally = Tally::default();
    check(random_parallel_graph(seed), seed, &mut tally).await;
    assert_eq!(tally.node_failed, 1);
    assert_eq!(tally.multi_failure_waves, 1);
    eprintln!("S17 seed 495: {tally:?}");
}

#[tokio::test]
async fn ceiling_can_settle_before_a_late_node_failure_without_losing_facts() {
    let seed = 2122;
    let mut tally = Tally::default();
    check(random_parallel_graph(seed), seed, &mut tally).await;
    assert_eq!(tally.node_failed, 1);
    assert_eq!(tally.ceiling_preempted_failures, 1);
    eprintln!("S18 seed 2122: {tally:?}");
}
