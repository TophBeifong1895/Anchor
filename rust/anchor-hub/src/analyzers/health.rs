//! The harness health report: what was recorded, how submissions were
//! judged, how tools and model turns performed, and the integrity of the
//! records themselves. Each irregularity becomes a finding with its records.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use super::{Analyzer, Finding, Need, Report, unmet};
use crate::query::{
    Call, Effect, GapPlace, Invocation, Key, Outcome, Query, Status, covered, group_by,
    node_result, summarize,
};

/// Evidence cited per finding, at most.
const CITED: usize = 50;
/// A check failing in at least this share of a task's graded Runs, over at
/// least [`FAILING_RUNS`] of them, is reported.
const FAILING_SHARE: f64 = 0.5;
const FAILING_RUNS: usize = 3;
/// A configuration whose invocations spend this share of their time before
/// the first model request is reported.
const STARTUP_SHARE: f64 = 0.1;

pub struct Health;

impl Analyzer for Health {
    fn id(&self) -> &'static str {
        "health"
    }
    fn version(&self) -> u32 {
        1
    }
    /// Without these, the sections that use them are left out; the kernel
    /// sections never need them.
    fn needs(&self) -> &'static [Need] {
        &[Need::Time, Need::Observations, Need::Grades]
    }

    fn analyze(&self, query: &Query) -> Report {
        let mut report = Report {
            analyzer: self.id(),
            version: self.version(),
            not_applicable: unmet(query, self.needs()),
            summary: BTreeMap::new(),
            findings: Vec::new(),
        };
        let invocations = query.invocations();
        records(query, &mut report);
        tasks(query, &mut report);
        if !report.not_applicable.contains(&Need::Observations) {
            coverage(query, &invocations, &mut report);
            tools(&invocations, &mut report);
            turns(&invocations, &mut report);
            if !report.not_applicable.contains(&Need::Time) {
                steps(query, &invocations, &mut report);
            }
            configurations(query, &invocations, &mut report);
        }
        if !report.not_applicable.contains(&Need::Grades) {
            quality(query, &invocations, &mut report);
        }
        report
    }
}

fn cite(keys: impl IntoIterator<Item = Key>) -> Vec<Key> {
    keys.into_iter().take(CITED).collect()
}

fn share(part: usize, whole: usize) -> Value {
    if whole == 0 {
        Value::Null
    } else {
        json!((part as f64 / whole as f64 * 1000.0).round() / 1000.0)
    }
}

/// Kernel fields: how many records, how many stamped, and every verdict that
/// was not an acceptance.
fn records(query: &Query, report: &mut Report) {
    let rows = query.rows();
    let roots: BTreeSet<&str> = rows.iter().map(|r| r.key.root.as_str()).collect();
    report.summary.insert(
        "records",
        json!({
            "rows": rows.len(),
            "roots": roots.len(),
            "stamped_time": share(rows.iter().filter(|r| r.at.is_some()).count(), rows.len()),
            "stamped_config": share(rows.iter().filter(|r| r.config.is_some()).count(), rows.len()),
        }),
    );
    let verdicts = group_by(rows, |row| {
        format!(
            "{}/{}/{}",
            json!(row.action).as_str().unwrap_or_default(),
            json!(row.verdict.effect).as_str().unwrap_or_default(),
            json!(row.verdict.reason).as_str().unwrap_or_default()
        )
    });
    report.summary.insert(
        "verdicts",
        json!(
            verdicts
                .iter()
                .map(|(k, v)| (k.clone(), v.len()))
                .collect::<BTreeMap<_, _>>()
        ),
    );
    let refused = group_by(
        rows.iter().filter(|r| r.verdict.effect != Effect::Accepted),
        |row| {
            (
                kind(&row.key.resource).to_owned(),
                json!(row.verdict.effect)
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                json!(row.verdict.reason)
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            )
        },
    );
    for ((kind, effect, reason), rows) in refused {
        report.findings.push(Finding {
            code: "not_accepted",
            subject: format!("{kind}/*"),
            detail: format!("{} submission(s) {effect}: {reason}", rows.len()),
            evidence: cite(rows.iter().map(|r| r.key.clone())),
        });
    }
}

/// A resource's kind: its id up to the first `/`.
fn kind(id: &str) -> &str {
    id.split('/').next().unwrap_or(id)
}

fn tasks(query: &Query, report: &mut Report) {
    let tasks = query.tasks();
    let statuses = group_by(&tasks, |task| {
        json!(task.entry.status)
            .to_string()
            .trim_matches('"')
            .to_owned()
    });
    let settled: Vec<u64> = tasks
        .iter()
        .filter(|t| t.entry.status.is_some_and(Status::is_settled))
        .filter_map(|t| t.span_ms())
        .collect();
    report.summary.insert(
        "tasks",
        json!({
            "count": tasks.len(),
            "status": statuses.iter().map(|(k, v)| (k.clone(), v.len())).collect::<BTreeMap<_, _>>(),
            "settled_span_ms": summarize(settled),
        }),
    );
    for task in &tasks {
        if let Some(status) = task.entry.status
            && status.is_settled()
            && status != Status::Done
        {
            report.findings.push(Finding {
                code: "task_not_done",
                subject: task.entry.id.clone(),
                detail: format!("settled as {}", json!(status)),
                evidence: cite(task.rows.last().map(|r| r.key.clone())),
            });
        }
    }
}

/// Whether both sides of every invocation were recorded, and whether the
/// observations agree with themselves.
fn coverage(query: &Query, invocations: &[Invocation], report: &mut Report) {
    let observed_runs: BTreeSet<&str> = invocations
        .iter()
        .filter(|i| !i.traces.is_empty())
        .map(|i| i.run_id.as_str())
        .collect();
    let known_runs: BTreeSet<String> = query
        .tasks()
        .iter()
        .filter_map(|t| t.entry.id.strip_prefix("run/").map(str::to_owned))
        .collect();
    let mut unobserved = Vec::new();
    let mut orphans = Vec::new();
    for invocation in invocations {
        match (invocation.result, invocation.traces.is_empty()) {
            // A node that spent model requests in a Run whose other
            // invocations were observed: the recorder missed this one.
            (Some(result), true)
                if result.cost > 0 && observed_runs.contains(invocation.run_id.as_str()) =>
            {
                unobserved.push(invocation)
            }
            (None, false) if known_runs.contains(&invocation.run_id) => orphans.push(invocation),
            _ => {}
        }
        for life in invocation.lifecycles() {
            let turns = invocation.turns().len() as u64;
            let calls = invocation.calls().len() as u64 + life.tool_calls_dropped;
            if invocation.traces.len() == 1
                && (turns != life.model_turns || calls != life.tool_calls)
            {
                report.findings.push(Finding {
                    code: "counts_disagree",
                    subject: invocation.key(),
                    detail: format!(
                        "lifecycle reports {} turns and {} calls; {} turns and {} calls recorded",
                        life.model_turns, life.tool_calls, turns, calls
                    ),
                    evidence: invocation.evidence(),
                });
            }
            if life.tool_calls_dropped > 0 {
                report.findings.push(Finding {
                    code: "tool_calls_dropped",
                    subject: invocation.key(),
                    detail: format!("{} tool call(s) not recorded", life.tool_calls_dropped),
                    evidence: invocation.evidence(),
                });
            }
            if life.outcome != Outcome::Completed {
                report.findings.push(Finding {
                    code: "invocation_not_completed",
                    subject: invocation.key(),
                    detail: format!("ended {}", json!(life.outcome)),
                    evidence: invocation.evidence(),
                });
            }
        }
        let unclosed: Vec<Call> = invocation
            .calls()
            .into_iter()
            .filter(|c| c.ended_at_ms.is_none() || c.started_at_ms.is_none())
            .collect();
        if !unclosed.is_empty() {
            report.findings.push(Finding {
                code: "call_without_time",
                subject: invocation.key(),
                detail: format!("{} tool call(s) lack a start or end time", unclosed.len()),
                evidence: invocation.evidence(),
            });
        }
    }
    report.summary.insert(
        "invocations",
        json!({
            "count": invocations.len(),
            "observed": invocations.iter().filter(|i| !i.traces.is_empty()).count(),
            "with_result": invocations.iter().filter(|i| i.result.is_some()).count(),
            "resumed": invocations.iter().filter(|i| i.traces.len() > 1).count(),
            "unobserved_with_model_requests": unobserved.len(),
            "observed_without_result": orphans.len(),
            "runs_observed": observed_runs.len(),
            "runs_known": known_runs.len(),
        }),
    );
    for invocation in unobserved {
        report.findings.push(Finding {
            code: "unobserved_invocation",
            subject: invocation.key(),
            detail: format!(
                "node result spent {} model request(s) but no observation was recorded",
                invocation.result.map_or(0, |r| r.cost)
            ),
            evidence: invocation.evidence(),
        });
    }
    for invocation in orphans {
        let outcomes: Vec<Value> = invocation
            .lifecycles()
            .iter()
            .map(|l| json!(l.outcome))
            .collect();
        report.findings.push(Finding {
            code: "observation_without_result",
            subject: invocation.key(),
            detail: format!(
                "observed (outcome {}) but its Run recorded no result for it",
                json!(outcomes)
            ),
            evidence: invocation.evidence(),
        });
    }
}

/// Tool efficiency: frequency, success, failure classes, duration, and calls
/// the model had to correct.
fn tools(invocations: &[Invocation], report: &mut Report) {
    let calls: Vec<(&Invocation, Call)> = invocations
        .iter()
        .flat_map(|i| i.calls().into_iter().map(move |c| (i, c)))
        .collect();
    let by_tool = group_by(&calls, |(_, call)| call.tool.to_owned());
    let mut summary = BTreeMap::new();
    for (tool, calls) in &by_tool {
        let failed: Vec<_> = calls.iter().filter(|(_, c)| !c.ok).collect();
        let failures = group_by(&failed, |(_, c)| {
            json!(c.failure)
                .as_str()
                .unwrap_or("unclassified")
                .to_owned()
        });
        summary.insert(
            tool.clone(),
            json!({
                "calls": calls.len(),
                "ok": calls.len() - failed.len(),
                "success_rate": share(calls.len() - failed.len(), calls.len()),
                "failures": failures.iter().map(|(k, v)| (k.clone(), v.len())).collect::<BTreeMap<_, _>>(),
                "duration_ms": summarize(calls.iter().filter_map(|(_, c)| c.duration_ms())),
                "shapes": calls.iter().map(|(_, c)| c.arguments_shape).collect::<BTreeSet<_>>().len(),
                // Where the `path` arguments of this tool pointed, and what
                // happened there: the only handle on the value of a rejected
                // path, whose value itself is never recorded.
                "paths": group_by(
                    calls.iter().filter_map(|(_, c)| Some((c.path?, c))),
                    |(class, _)| *class,
                )
                    .into_iter()
                    .map(|(class, of)| {
                        let failed = of.iter().filter(|(_, call)| !call.ok).count();
                        (format!("{class:?}").to_lowercase(), json!({"calls": of.len(), "failed": failed}))
                    })
                    .collect::<BTreeMap<_, _>>(),
            }),
        );
        for (class, failed) in failures {
            let mut keys: Vec<Key> = failed.iter().flat_map(|(i, _)| i.evidence()).collect();
            keys.dedup();
            // The path families the failing calls named; the path itself is
            // not recorded, so this is as close to the value as the records
            // can come.
            let paths: Vec<String> = {
                let mut seen: Vec<String> = failed
                    .iter()
                    .filter_map(|(_, c)| c.path)
                    .map(|class| format!("{class:?}").to_lowercase())
                    .collect();
                seen.sort();
                seen.dedup();
                seen
            };
            report.findings.push(Finding {
                code: "tool_failures",
                subject: format!("tool {tool}"),
                detail: format!(
                    "{} of {} call(s) failed: {class}{}",
                    failed.len(),
                    calls.len(),
                    if paths.is_empty() {
                        String::new()
                    } else {
                        format!(", on {} path(s)", paths.join(", "))
                    }
                ),
                evidence: cite(keys),
            });
        }
    }
    report.summary.insert("tools", json!(summary));

    // A failed call followed, in the same invocation, by a successful call of
    // the same tool: the model recovered from the refusal. Arguments of another
    // shape mean it changed how it asked; the same shape means it changed only
    // the value, which is what a rejected path looks like from here. Both are
    // recoveries, so both count, and the count is of failed calls rather than
    // of the invocations they happened in.
    let mut corrected: BTreeMap<String, (usize, usize, Vec<&Invocation>)> = BTreeMap::new();
    for invocation in invocations {
        let calls = invocation.calls();
        for (index, call) in calls.iter().enumerate() {
            if call.ok {
                continue;
            }
            let Some(later) = calls[index + 1..]
                .iter()
                .find(|later| later.tool == call.tool && later.ok)
            else {
                continue;
            };
            let entry = corrected.entry(call.tool.to_owned()).or_default();
            entry.2.push(invocation);
            if later.arguments_shape == call.arguments_shape {
                entry.1 += 1;
            } else {
                entry.0 += 1;
            }
        }
    }
    for (tool, (reshaped, revalued, invocations)) in corrected {
        let mut keys: Vec<Key> = invocations.iter().flat_map(|i| i.evidence()).collect();
        keys.dedup();
        report.findings.push(Finding {
            code: "call_corrected",
            subject: format!("tool {tool}"),
            detail: format!(
                "{} failed call(s) were followed by a successful call of the same tool: \
                 {reshaped} with arguments of another shape, {revalued} with arguments of the same shape",
                reshaped + revalued
            ),
            evidence: cite(keys),
        });
    }
}

/// Model turns: how many per invocation, their size and their latency, and
/// where an invocation's time went.
fn turns(invocations: &[Invocation], report: &mut Report) {
    let observed: Vec<&Invocation> = invocations
        .iter()
        .filter(|i| !i.traces.is_empty())
        .collect();
    let turns: Vec<_> = observed.iter().flat_map(|i| i.turns()).collect();
    let (mut model, mut tool, mut total) = (0_u64, 0_u64, 0_u64);
    let mut gaps: BTreeMap<GapPlace, Vec<u64>> = BTreeMap::new();
    for invocation in &observed {
        total += invocation
            .lifecycles()
            .iter()
            .map(|l| l.ended_at_ms.saturating_sub(l.started_at_ms))
            .sum::<u64>();
        model += covered(invocation.turn_spans().into_iter().map(|(_, span)| span));
        tool += covered(invocation.calls().iter().filter_map(Call::span));
        for gap in invocation.gaps() {
            gaps.entry(gap.place()).or_default().push(gap.ms());
        }
    }
    let part = |ms: u64| {
        if total == 0 {
            Value::Null
        } else {
            json!((ms as f64 / total as f64 * 1000.0).round() / 1000.0)
        }
    };
    report.summary.insert(
        "turns",
        json!({
            "per_invocation": summarize(observed.iter().map(|i| i.turns().len() as u64)),
            "total_tokens": summarize(turns.iter().map(|t| t.total_tokens)),
            "input_tokens": summarize(turns.iter().map(|t| t.input_tokens)),
            "output_tokens": summarize(turns.iter().map(|t| t.output_tokens)),
            "elapsed_ms": summarize(turns.iter().filter_map(|t| t.elapsed_ms)),
            "time_to_first_token_ms": summarize(turns.iter().filter_map(|t| t.time_to_first_token_ms)),
            "cache_read_share": share(
                turns.iter().filter_map(|t| t.cache_read_tokens).sum::<u64>() as usize,
                turns.iter().map(|t| t.input_tokens).sum::<u64>() as usize,
            ),
            "compactions": turns.iter().filter(|t| t.compaction).count(),
        }),
    );
    // Shares of invocation time: the model working, tools running, and the
    // harness between them, by where the gap lies.
    report.summary.insert(
        "time",
        json!({
            "invocation_ms": total,
            "model_share": part(model),
            "tool_share": part(tool),
            "unobserved_share": part(gaps.values().flatten().sum()),
            "unobserved": gaps.iter().map(|(place, ms)| (
                json!(place).as_str().unwrap_or_default().to_owned(),
                json!({"share": part(ms.iter().sum()), "ms": summarize(ms.iter().copied())}),
            )).collect::<BTreeMap<_, _>>(),
        }),
    );
}

/// Run steps across roots: how long each node's step took in its Run, and
/// for observed invocations how much of that the invocation itself was.
fn steps(query: &Query, invocations: &[Invocation], report: &mut Report) {
    let lifetimes: BTreeMap<&Key, u64> = invocations
        .iter()
        .filter_map(|i| {
            let ms = i
                .lifecycles()
                .iter()
                .map(|l| l.ended_at_ms.saturating_sub(l.started_at_ms))
                .sum();
            (!i.traces.is_empty()).then_some((&i.result?.key, ms))
        })
        .collect();
    let (mut unobserved, mut outside) = (BTreeMap::<String, Vec<u64>>::new(), Vec::new());
    for task in query.tasks() {
        for (row, since) in task.steps() {
            if row.verdict.effect != Effect::Accepted {
                continue;
            }
            let (Some(since), Some((_, node, _))) = (since, node_result(row)) else {
                continue;
            };
            match lifetimes.get(&row.key) {
                Some(&ms) => outside.push(since.saturating_sub(ms)),
                None => unobserved.entry(node.to_owned()).or_default().push(since),
            }
        }
    }
    report.summary.insert(
        "steps",
        json!({
            "observed_overhead_ms": summarize(outside),
            "unobserved_step_ms": unobserved.iter().map(|(node, ms)| (node.clone(), summarize(ms.iter().copied()))).collect::<BTreeMap<_, _>>(),
        }),
    );
}

/// Per configuration: the same node under different configurations is what
/// a later comparison would contrast.
fn configurations(query: &Query, invocations: &[Invocation], report: &mut Report) {
    let observed = invocations.iter().filter(|i| !i.traces.is_empty());
    let by_config = group_by(observed, |i| i.config().unwrap_or("").to_owned());
    let mut summary = BTreeMap::new();
    for (digest, invocations) in &by_config {
        let manifest = query.configuration(digest);
        // Startup: from the invocation's start to its first observed
        // activity, the harness's fixed cost before the model is asked.
        let startup: Vec<(u64, u64)> = invocations
            .iter()
            .flat_map(|i| {
                let gaps = i.gaps();
                i.lifecycles().into_iter().filter_map(move |life| {
                    let gap = gaps.iter().find(|g| {
                        g.place() == GapPlace::Startup && g.start == life.started_at_ms
                    })?;
                    Some((
                        gap.ms(),
                        life.ended_at_ms.saturating_sub(life.started_at_ms),
                    ))
                })
            })
            .collect();
        let startup_share = share(
            startup.iter().map(|(gap, _)| *gap).sum::<u64>() as usize,
            startup.iter().map(|(_, total)| *total).sum::<u64>() as usize,
        );
        summary.insert(
            digest.clone(),
            json!({
                "node": manifest.map(|m| m.node_id.as_str()),
                "model": manifest.map(|m| m.model.as_str()),
                "plugins": manifest.map(|m| m.plugins.len()),
                "known": manifest.is_some(),
                "invocations": invocations.len(),
                "turns": summarize(invocations.iter().map(|i| i.turns().len() as u64)),
                "tool_calls": summarize(invocations.iter().map(|i| i.calls().len() as u64)),
                "tokens": summarize(invocations.iter().map(|i| i.turns().iter().map(|t| t.total_tokens).sum())),
                "duration_ms": summarize(invocations.iter().flat_map(|i| i.lifecycles()).map(|l| l.ended_at_ms.saturating_sub(l.started_at_ms))),
                "startup_ms": summarize(startup.iter().map(|(gap, _)| *gap)),
                "startup_share": startup_share,
            }),
        );
        if let Some(part) = startup_share.as_f64()
            && part >= STARTUP_SHARE
        {
            report.findings.push(Finding {
                code: "startup_heavy",
                subject: format!(
                    "config {digest} (node {})",
                    manifest.map_or("?", |m| m.node_id.as_str())
                ),
                detail: format!(
                    "{:.0}% of invocation time passes before the first model request (median {} ms)",
                    part * 100.0,
                    summarize(startup.iter().map(|(gap, _)| *gap)).map_or(0, |s| s.median)
                ),
                evidence: cite(invocations.iter().flat_map(|i| i.evidence())),
            });
        }
        if manifest.is_none() {
            report.findings.push(Finding {
                code: "configuration_unknown",
                subject: format!("config {digest}"),
                detail: format!(
                    "{} invocation(s) recorded under a configuration with no readable manifest",
                    invocations.len()
                ),
                evidence: cite(invocations.iter().flat_map(|i| i.evidence())),
            });
        }
    }
    report.summary.insert("configurations", json!(summary));
}

/// Task quality from grades, beside what the graded Runs cost: the signal a
/// comparison of configurations weighs efficiency against.
fn quality(query: &Query, invocations: &[Invocation], report: &mut Report) {
    let grades = query.grades();
    let by_run = group_by(invocations, |i| i.run_id.as_str());
    let by_task = group_by(grades.values(), |g| {
        (g.grade.task.clone(), g.grade.task_version)
    });
    let mut summary = BTreeMap::new();
    for ((task, version), graded) in by_task {
        let runs: BTreeSet<&str> = graded.iter().map(|g| g.grade.run_id.as_str()).collect();
        let of_run = |run: &str| {
            by_run
                .get(run)
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .copied()
        };
        let tokens = runs.iter().map(|run| {
            of_run(run)
                .flat_map(|i| i.turns())
                .map(|t| t.total_tokens)
                .sum::<u64>()
        });
        let agent_ms = runs.iter().map(|run| {
            of_run(run)
                .flat_map(|i| i.lifecycles())
                .map(|l| l.ended_at_ms.saturating_sub(l.started_at_ms))
                .sum::<u64>()
        });
        let tool_calls = runs
            .iter()
            .map(|run| of_run(run).map(|i| i.calls().len() as u64).sum::<u64>());
        let configs: BTreeSet<&str> = runs
            .iter()
            .flat_map(|run| of_run(run).filter_map(|i| i.config()))
            .collect();
        let mut checks: BTreeMap<&str, (usize, Vec<Key>)> = BTreeMap::new();
        for g in &graded {
            for check in &g.grade.checks {
                let entry = checks.entry(check.name.as_str()).or_default();
                if check.passed {
                    entry.0 += 1;
                } else {
                    entry.1.push(g.row.key.clone());
                }
            }
        }
        let scores: Vec<f64> = graded.iter().map(|g| g.grade.score()).collect();
        summary.insert(
            format!("{task} v{version}"),
            json!({
                "runs": graded.len(),
                "mean_score": (scores.iter().sum::<f64>() / scores.len() as f64 * 1000.0).round() / 1000.0,
                "pass_rate": share(graded.iter().filter(|g| g.grade.passed()).count(), graded.len()),
                "checks": checks.iter().map(|(name, (passed, _))| (*name, share(*passed, graded.len()))).collect::<BTreeMap<_, _>>(),
                "configurations": configs.len(),
                "tokens": summarize(tokens),
                "agent_ms": summarize(agent_ms),
                "tool_calls": summarize(tool_calls),
            }),
        );
        for (name, (passed, failed)) in checks {
            let failing = 1.0 - passed as f64 / graded.len() as f64;
            if graded.len() >= FAILING_RUNS && failing >= FAILING_SHARE {
                report.findings.push(Finding {
                    code: "check_failing",
                    subject: format!("task {task} v{version}"),
                    detail: format!(
                        "check {name} failed in {} of {} graded Run(s)",
                        failed.len(),
                        graded.len()
                    ),
                    evidence: cite(failed),
                });
            }
        }
    }
    report.summary.insert("quality", json!(summary));
}
