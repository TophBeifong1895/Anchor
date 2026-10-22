//! Gate: may an arm of a controlled experiment replace the reference arm?
//! Quality is the constraint: an arm is adopted only on evidence that it is
//! not worse by more than `delta`, whatever it saves. The decision may be read
//! after every Run and the experiment stopped as soon as it is made, so the
//! budget is spent only until the evidence is there.
//!
//! Per task, the arm's Runs are paired with the reference's in the order they
//! started (an experiment interleaves them). Each pair's score difference
//! feeds bets (testing by betting, see [`Wealth`]):
//!
//! - one pooled `not_worse` bet per arm, over its pairs on all tasks: it
//!   grows when the arm is no worse than `-delta` on the task set;
//! - one `worse` guard per arm and task: it grows when the arm is worse than
//!   that on this task, so a loss on one task is not averaged away.
//!
//! All are e-processes, valid at any stopping time. Admissions and guards
//! are two families, each read under e-BH, which keeps the expected share of
//! wrong decisions in it at most α whatever the dependence. An arm is
//! admitted when its pooled bet is chosen and no guard vetoes it; neither
//! admitted nor rejected, it stays open, and when its budget ends it is not
//! adopted.

use std::collections::BTreeMap;

use serde_json::json;

use super::{
    Analyzer, Finding, Need, Report,
    contrast::{Contrast, Measured},
    unmet,
};
use crate::query::{Query, Wealth, e_bh};

/// At most this expected share of gate decisions may be wrong.
pub const ALPHA: f64 = 0.05;
/// A quality loss smaller than this is tolerated.
pub const QUALITY_DELTA: f64 = 0.05;

pub struct Gate {
    pub alpha: f64,
    pub delta: f64,
    /// The arm the others are compared against, by a prefix of its value;
    /// the arm first seen when not given.
    pub reference: Option<String>,
    /// The configuration manifest field that names a Run's arm.
    pub by: String,
}

impl Default for Gate {
    fn default() -> Self {
        Self {
            alpha: ALPHA,
            delta: QUALITY_DELTA,
            reference: None,
            by: Contrast::default().by,
        }
    }
}

impl Analyzer for Gate {
    fn id(&self) -> &'static str {
        "gate"
    }
    fn version(&self) -> u32 {
        1
    }
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
        if !report.not_applicable.is_empty() {
            return report;
        }
        let contrast = Contrast {
            by: self.by.clone(),
            reference: None,
        };
        let (tasks, _) = contrast.runs(query);
        let Some(reference) = self.reference_arm(&tasks) else {
            report
                .summary
                .insert("reference_missing", json!(self.reference));
            return report;
        };
        report.summary.insert("reference", json!(reference));
        report.summary.insert("alpha", json!(self.alpha));
        report.summary.insert("delta", json!(self.delta));

        // Per arm: its pairs on every task, in the order the later Run of
        // each pair started, which no outcome can change.
        let mut arms: BTreeMap<&str, Vec<(String, u64, f64)>> = BTreeMap::new();
        for (task, by_arm) in &tasks {
            let Some(base) = by_arm.get(&reference) else {
                continue;
            };
            let base = started(base);
            for (arm, runs) in by_arm {
                if *arm == reference {
                    continue;
                }
                for (ours, theirs) in started(runs).iter().zip(&base) {
                    arms.entry(arm.as_str()).or_default().push((
                        task.clone(),
                        ours.first_at.max(theirs.first_at),
                        ours.score - theirs.score,
                    ));
                }
            }
        }

        // A difference lies in [-1, 1]; each bet's values are shifted so that
        // its hypothesis says their mean is at most zero.
        let mut admissions = Vec::new();
        let mut guards = Vec::new();
        for (arm, mut pairs) in arms {
            pairs.sort_by(|a, b| (a.1, &a.0).cmp(&(b.1, &b.0)));
            let mut pooled = Wealth::new(-1.0 + self.delta);
            let mut worse: BTreeMap<&str, Wealth> = BTreeMap::new();
            for (task, _, difference) in &pairs {
                pooled.push(difference + self.delta);
                worse
                    .entry(task.as_str())
                    .or_insert_with(|| Wealth::new(-1.0 - self.delta))
                    .push(-difference - self.delta);
            }
            admissions.push((arm, pooled));
            guards.push((
                arm,
                worse
                    .into_iter()
                    .map(|(t, w)| (t.to_owned(), w))
                    .collect::<Vec<_>>(),
            ));
        }

        // Two families, each under its own e-BH: admitting arms, and the
        // per-task guards that can veto them.
        let admitted = e_bh(
            &admissions
                .iter()
                .map(|(_, w)| w.value())
                .collect::<Vec<_>>(),
            self.alpha,
        );
        let flat: Vec<(&str, &str, Wealth)> = guards
            .iter()
            .flat_map(|(arm, tasks)| tasks.iter().map(move |(t, w)| (*arm, t.as_str(), *w)))
            .collect();
        let vetoed = e_bh(
            &flat.iter().map(|(_, _, w)| w.value()).collect::<Vec<_>>(),
            self.alpha,
        );

        report.summary.insert(
            "guards",
            json!(
                flat.iter()
                    .enumerate()
                    .map(|(i, (arm, task, w))| json!({
                        "arm": arm, "task": task, "pairs": w.n(),
                        "e_worse": round(w.value()), "worse": vetoed.contains(&i),
                    }))
                    .collect::<Vec<_>>()
            ),
        );
        for &i in &vetoed {
            let (arm, task, w) = flat[i];
            report.findings.push(Finding {
                code: "gate_worse",
                subject: format!("{task}: {}", short(arm)),
                detail: format!(
                    "quality worse than {} by more than {} over {} paired Runs (e={:.1})",
                    short(&reference),
                    self.delta,
                    w.n(),
                    w.value()
                ),
                evidence: tasks[task][arm]
                    .iter()
                    .map(|m| m.grade.clone())
                    .take(50)
                    .collect(),
            });
        }

        let mut decisions = BTreeMap::new();
        for (index, (arm, pooled)) in admissions.iter().enumerate() {
            let veto = flat
                .iter()
                .enumerate()
                .any(|(i, (a, _, _))| a == arm && vetoed.contains(&i));
            let decision = match (veto, admitted.contains(&index)) {
                (true, _) => "rejected",
                (false, true) => "admitted",
                (false, false) => "open",
            };
            decisions.insert(
                arm.to_string(),
                json!({"decision": decision, "pairs": pooled.n(), "e_not_worse": round(pooled.value())}),
            );
            if decision == "admitted" {
                report.findings.push(Finding {
                    code: "gate_admits",
                    subject: short(arm).to_owned(),
                    detail: format!(
                        "not worse than {} by more than {} over {} paired Runs (e={:.1}), and no task worse",
                        short(&reference),
                        self.delta,
                        pooled.n(),
                        pooled.value()
                    ),
                    evidence: Vec::new(),
                });
            }
        }
        report.summary.insert("arms", json!(decisions));
        report
    }
}

impl Gate {
    /// The arm named, or else the one whose first Run started first.
    fn reference_arm(
        &self,
        tasks: &BTreeMap<String, BTreeMap<String, Vec<Measured>>>,
    ) -> Option<String> {
        let mut first: BTreeMap<&str, u64> = BTreeMap::new();
        for arms in tasks.values() {
            for (arm, runs) in arms {
                let at = runs.iter().map(|r| r.first_at).min().unwrap_or(u64::MAX);
                let entry = first.entry(arm.as_str()).or_insert(at);
                *entry = (*entry).min(at);
            }
        }
        match &self.reference {
            Some(prefix) => first
                .keys()
                .find(|arm| arm.starts_with(prefix.as_str()))
                .map(|arm| (*arm).to_owned()),
            None => first
                .iter()
                .min_by_key(|(arm, at)| (**at, **arm))
                .map(|(arm, _)| (*arm).to_owned()),
        }
    }
}

/// Runs in the order they started.
fn started(runs: &[Measured]) -> Vec<&Measured> {
    let mut runs: Vec<&Measured> = runs.iter().collect();
    runs.sort_by_key(|r| (r.first_at, r.grade.clone()));
    runs
}

fn round(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

fn short(arm: &str) -> &str {
    arm.get(..12).unwrap_or(arm)
}
