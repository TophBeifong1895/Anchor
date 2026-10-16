//! E3: measurement-driven selection over a simulated proposer.
//!
//! The proposer is a stochastic model of an agent working in a sandbox with
//! hidden walls (constraints it does not know about). It is a stand-in: the
//! probes, statistics and gate are real code; the behaviour model is assumed.
//!
//! Knobs:
//! - `informative_errors`: refusal messages that teach (higher recovery odds).
//! - `preload`: walls are disclosed up front, at a fixed context cost.
//! - `give_up_on_wall`: the proposer stops at the first refusal (a Goodhart
//!   change: fewer collisions, fewer accepted results).
//! - `cosmetic`: a change with no behavioural effect (placebo).

use crate::measure::{mean, paired_permutation, spearman};
use crate::rng::{Rng, mix};

/// Ground-truth per-step hit probability of each hidden wall.
pub const WALLS: [f64; 6] = [0.12, 0.08, 0.05, 0.03, 0.015, 0.005];
const STEP_COST: u64 = 3;
const COLLISION_COST: u64 = 1;
const ATTEMPT_COST: u64 = 2;
const PRELOAD_COST: u64 = 6;
const MAX_ATTEMPTS: u32 = 8;
const QUALITY: f64 = 0.92;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Knobs {
    pub informative_errors: bool,
    pub preload: bool,
    pub give_up_on_wall: bool,
    pub cosmetic: u8,
}

impl Knobs {
    pub fn baseline() -> Self {
        Self {
            informative_errors: false,
            preload: false,
            give_up_on_wall: false,
            cosmetic: 0,
        }
    }
    fn recovery_odds(&self) -> f64 {
        if self.informative_errors { 0.8 } else { 0.35 }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Case {
    pub id: u64,
    pub steps: u32,
}

pub fn cases(seed: u64, n: usize, steps: (u32, u32)) -> Vec<Case> {
    let mut rng = Rng::new(seed);
    (0..n)
        .map(|i| Case {
            id: mix(&[seed, i as u64]),
            steps: steps.0 + rng.below((steps.1 - steps.0 + 1) as u64) as u32,
        })
        .collect()
}

#[derive(Clone, Debug, Default)]
pub struct Session {
    pub cost: u64,
    pub accepted: bool,
    /// (wall, attempts to recover)
    pub collisions: Vec<(usize, u32)>,
    pub gave_up: bool,
}

pub fn run_session(case: Case, knobs: Knobs, seed: u64) -> Session {
    let mut rng = Rng::new(seed);
    let mut session = Session::default();
    if knobs.preload {
        session.cost += PRELOAD_COST;
    }
    let mut learned = [false; WALLS.len()];
    for _ in 0..case.steps {
        session.cost += STEP_COST;
        for (wall, &p) in WALLS.iter().enumerate() {
            let p = if knobs.preload { p * 0.15 } else { p };
            if learned[wall] || !rng.chance(p) {
                continue;
            }
            session.cost += COLLISION_COST;
            if knobs.give_up_on_wall {
                session.collisions.push((wall, 0));
                session.gave_up = true;
                return session;
            }
            let mut attempts = 0;
            loop {
                attempts += 1;
                session.cost += ATTEMPT_COST;
                if rng.chance(knobs.recovery_odds()) {
                    break;
                }
                if attempts == MAX_ATTEMPTS {
                    session.collisions.push((wall, attempts));
                    return session;
                }
            }
            session.collisions.push((wall, attempts));
            learned[wall] = rng.chance(0.9);
        }
    }
    session.accepted = rng.chance(QUALITY);
    session
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CaseResult {
    pub cost: f64,
    pub accepted: f64,
    pub sessions: f64,
}

/// `stream` makes the two variants' randomness independent, as two model runs
/// would be.
pub fn evaluate(cases: &[Case], knobs: Knobs, repeats: u32, stream: u64) -> Vec<CaseResult> {
    cases
        .iter()
        .map(|case| {
            let mut result = CaseResult::default();
            for rep in 0..repeats {
                let s = run_session(*case, knobs, mix(&[case.id, rep as u64, stream]));
                result.cost += s.cost as f64;
                result.accepted += s.accepted as u8 as f64;
                result.sessions += 1.0;
            }
            result
        })
        .collect()
}

/// The main objective: cost per accepted result. Failed sessions count in the
/// numerator and not in the denominator.
pub fn cost_per_accepted(results: &[CaseResult]) -> f64 {
    let cost: f64 = results.iter().map(|r| r.cost).sum();
    let accepted: f64 = results.iter().map(|r| r.accepted).sum();
    if accepted == 0.0 {
        f64::INFINITY
    } else {
        cost / accepted
    }
}

pub fn acceptance(results: &[CaseResult]) -> f64 {
    let accepted: f64 = results.iter().map(|r| r.accepted).sum();
    let sessions: f64 = results.iter().map(|r| r.sessions).sum();
    accepted / sessions
}

#[derive(Clone, Copy, Debug)]
pub struct GateConfig {
    pub repeats: u32,
    pub alpha: f64,
    /// Largest acceptable drop in acceptance rate.
    pub margin: f64,
    pub resamples: u32,
}

impl Default for GateConfig {
    fn default() -> Self {
        Self {
            repeats: 3,
            alpha: 0.05,
            margin: 0.02,
            resamples: 999,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct GateDecision {
    pub improvement: f64,
    pub p_cost: f64,
    pub acceptance_drop: f64,
    pub p_drop: f64,
    pub accepted: bool,
}

/// Deterministic selection: quality is a constraint, efficiency the objective.
/// The rule is fixed code; the optimiser proposing `candidate` cannot change it.
pub fn gate(
    holdout: &[Case],
    reference: Knobs,
    candidate: Knobs,
    config: GateConfig,
    seed: u64,
) -> GateDecision {
    let a = evaluate(holdout, reference, config.repeats, mix(&[seed, 1]));
    let b = evaluate(holdout, candidate, config.repeats, mix(&[seed, 2]));
    let (improvement, p_cost) = paired_permutation(
        &a,
        &b,
        |x, y| cost_per_accepted(x) - cost_per_accepted(y),
        config.resamples,
        mix(&[seed, 3]),
    );
    let (acceptance_drop, p_drop) = paired_permutation(
        &a,
        &b,
        |x, y| acceptance(x) - acceptance(y),
        config.resamples,
        mix(&[seed, 4]),
    );
    let quality_ok = acceptance_drop <= config.margin && p_drop > config.alpha;
    GateDecision {
        improvement,
        p_cost,
        acceptance_drop,
        p_drop,
        accepted: quality_ok && improvement > 0.0 && p_cost < config.alpha,
    }
}

/// The common LLM-driven rule: one run per case, keep the candidate if its
/// number is better.
pub fn naive_gate(holdout: &[Case], reference: Knobs, candidate: Knobs, seed: u64) -> bool {
    let a = evaluate(holdout, reference, 1, mix(&[seed, 1]));
    let b = evaluate(holdout, candidate, 1, mix(&[seed, 2]));
    cost_per_accepted(&b) < cost_per_accepted(&a)
}

// ---------------------------------------------------------------------------
// Diagnostics straight from probes (no model reads any record)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Diagnostics {
    pub sessions: usize,
    pub collisions_per_wall: Vec<u64>,
    /// Share of sessions that hit the wall at least once: each new session
    /// paying again for the same knowledge.
    pub sessions_hitting_wall: Vec<f64>,
    pub mean_recovery_distance: f64,
    pub rank_correlation_with_truth: f64,
}

pub fn diagnose(cases: &[Case], knobs: Knobs, seed: u64) -> Diagnostics {
    let mut per_wall = vec![0u64; WALLS.len()];
    let mut sessions_hitting = vec![0u64; WALLS.len()];
    let mut distances = Vec::new();
    for case in cases {
        let s = run_session(*case, knobs, mix(&[case.id, seed]));
        let mut hit = [false; WALLS.len()];
        for (wall, attempts) in &s.collisions {
            per_wall[*wall] += 1;
            hit[*wall] = true;
            if *attempts > 0 {
                distances.push(*attempts as f64);
            }
        }
        for (wall, h) in hit.iter().enumerate() {
            sessions_hitting[wall] += *h as u64;
        }
    }
    let observed: Vec<f64> = per_wall.iter().map(|c| *c as f64).collect();
    Diagnostics {
        sessions: cases.len(),
        rank_correlation_with_truth: spearman(&observed, &WALLS),
        collisions_per_wall: per_wall,
        sessions_hitting_wall: sessions_hitting
            .iter()
            .map(|h| *h as f64 / cases.len() as f64)
            .collect(),
        mean_recovery_distance: mean(&distances),
    }
}

// ---------------------------------------------------------------------------
// Knob analysis and a learned policy
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub struct Sensitivity {
    /// Positive: turning the knob on lowers cost per accepted result.
    pub effect: f64,
    pub p: f64,
}

pub fn sensitivity(cases: &[Case], base: Knobs, on: Knobs, seed: u64) -> Sensitivity {
    let a = evaluate(cases, base, 3, mix(&[seed, 11]));
    let b = evaluate(cases, on, 3, mix(&[seed, 12]));
    let (effect, p) = paired_permutation(
        &a,
        &b,
        |x, y| cost_per_accepted(x) - cost_per_accepted(y),
        999,
        mix(&[seed, 13]),
    );
    // Two-sided: also test the opposite direction.
    let (_, p_rev) = paired_permutation(
        &a,
        &b,
        |x, y| cost_per_accepted(y) - cost_per_accepted(x),
        999,
        mix(&[seed, 14]),
    );
    Sensitivity {
        effect,
        p: (2.0 * p.min(p_rev)).min(1.0),
    }
}

/// What the data says a knob is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KnobVerdict {
    /// No measurable effect anywhere: delete it.
    Delete,
    /// Same best value in every task class: fix it as a constant.
    Constant,
    /// Best value depends on the task: a real knob for the policy.
    Knob,
}

/// One test per task class means several chances to be fooled by noise, so
/// the significance level is divided by the number of classes (Bonferroni).
/// Without it, a placebo knob is kept as "real" noticeably more often.
pub fn classify(per_class: &[Sensitivity], alpha: f64) -> KnobVerdict {
    let corrected = alpha / per_class.len().max(1) as f64;
    let significant: Vec<&Sensitivity> = per_class.iter().filter(|s| s.p < corrected).collect();
    if significant.is_empty() {
        return KnobVerdict::Delete;
    }
    let positive = significant.iter().any(|s| s.effect > 0.0);
    let negative = significant.iter().any(|s| s.effect < 0.0);
    if positive && negative {
        KnobVerdict::Knob
    } else {
        KnobVerdict::Constant
    }
}

/// A policy from a task description (here: its length) to a knob value,
/// learned from training cases: the smallest version of "a model that turns
/// the knobs". Returns the threshold above which `preload` is enabled.
pub fn learn_preload_threshold(training: &[Case], base: Knobs, seed: u64) -> u32 {
    let mut best = (f64::INFINITY, 0);
    for threshold in (0..=60).step_by(2) {
        let results: Vec<CaseResult> = training
            .iter()
            .flat_map(|case| {
                let knobs = Knobs {
                    preload: case.steps > threshold,
                    ..base
                };
                evaluate(&[*case], knobs, 3, mix(&[seed, case.id]))
            })
            .collect();
        let objective = cost_per_accepted(&results);
        if objective < best.0 {
            best = (objective, threshold);
        }
    }
    best.1
}

pub fn evaluate_policy(
    cases: &[Case],
    base: Knobs,
    policy: impl Fn(&Case) -> bool,
    seed: u64,
) -> f64 {
    let results: Vec<CaseResult> = cases
        .iter()
        .flat_map(|case| {
            let knobs = Knobs {
                preload: policy(case),
                ..base
            };
            evaluate(&[*case], knobs, 3, mix(&[seed, case.id]))
        })
        .collect();
    cost_per_accepted(&results)
}
