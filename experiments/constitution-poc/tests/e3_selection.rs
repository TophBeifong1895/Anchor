//! E3: measurement and selection without a model reading any record.
//!
//! The proposer is simulated (see src/sim.rs); the probes, statistics and
//! gate are the code under test. Seeds are fixed, so results are exact.

use constitution_poc::sim::*;

fn base() -> Knobs {
    Knobs::baseline()
}

/// Counting alone recovers which walls matter and how good the error
/// messages are; no model reads a trace.
#[test]
fn probes_alone_rank_the_walls_and_measure_error_messages() {
    let all = cases(7, 2000, (4, 50));
    let terse = diagnose(&all, base(), 1);
    assert!(
        (terse.rank_correlation_with_truth - 1.0).abs() < 1e-9,
        "{terse:?}"
    );
    // Most sessions pay again for the most common wall: knowledge that should
    // be part of the stage.
    assert!(terse.sessions_hitting_wall[0] > 0.8);
    let informative = diagnose(
        &all,
        Knobs {
            informative_errors: true,
            ..base()
        },
        1,
    );
    assert!(terse.mean_recovery_distance > 2.0 * informative.mean_recovery_distance);
}

#[test]
fn the_gate_accepts_a_real_improvement() {
    let holdout = cases(99, 200, (4, 50));
    let candidate = Knobs {
        informative_errors: true,
        ..base()
    };
    for seed in 0..5 {
        let decision = gate(
            &holdout,
            base(),
            candidate,
            GateConfig::default(),
            1000 + seed,
        );
        assert!(decision.accepted, "{decision:?}");
    }
}

/// A change with no effect on behaviour. The deterministic gate accepts it at
/// about its significance level; "one run, keep it if the number is better"
/// accepts it about half of the time.
#[test]
fn the_gate_rejects_placebos_and_the_naive_rule_does_not() {
    let trials = 60;
    let placebo = Knobs {
        cosmetic: 1,
        ..base()
    };
    let (mut gated, mut naive) = (0, 0);
    for t in 0..trials {
        let holdout = cases(500 + t, 200, (4, 50));
        gated += gate(&holdout, base(), placebo, GateConfig::default(), t).accepted as u32;
        naive += naive_gate(&holdout, base(), placebo, t) as u32;
    }
    let gated_rate = gated as f64 / trials as f64;
    let naive_rate = naive as f64 / trials as f64;
    assert!(gated_rate <= 0.10, "gate false positives {gated_rate}");
    assert!(naive_rate >= 0.35, "naive false positives {naive_rate}");
}

/// Giving up at the first refusal makes every efficiency number look better:
/// cost per session and collisions fall. Quality as a constraint rejects it.
#[test]
fn quality_as_a_constraint_rejects_goodhart_changes() {
    let holdout = cases(99, 300, (4, 50));
    let give_up = Knobs {
        give_up_on_wall: true,
        ..base()
    };
    let a = evaluate(&holdout, base(), 1, 7);
    let b = evaluate(&holdout, give_up, 1, 7);
    let per_session = |r: &[CaseResult]| r.iter().map(|x| x.cost).sum::<f64>() / r.len() as f64;
    assert!(
        per_session(&b) < per_session(&a) / 3.0,
        "the naive efficiency number improves"
    );
    let decision = gate(&holdout, base(), give_up, GateConfig::default(), 9);
    assert!(
        !decision.accepted && decision.acceptance_drop > 0.5,
        "{decision:?}"
    );
}

/// Recursion on the knob set: the data says what each candidate knob is.
/// With 1,000 cases per task class the verdicts match the ground truth of the
/// simulation; with 300 the task-dependent knob is not yet detectable.
#[test]
fn data_decides_whether_a_knob_exists() {
    let informative = Knobs {
        informative_errors: true,
        ..base()
    };
    let verdict = |b: Knobs, on: Knobs, n: usize| {
        let short = sensitivity(&cases(31, n, (4, 8)), b, on, 1);
        let long = sensitivity(&cases(32, n, (30, 50)), b, on, 2);
        classify(&[short, long], 0.05)
    };
    // Better everywhere: stop treating it as a knob, fix it on.
    assert_eq!(verdict(base(), informative, 1000), KnobVerdict::Constant);
    // No effect anywhere: delete it.
    assert_eq!(
        verdict(
            informative,
            Knobs {
                cosmetic: 1,
                ..informative
            },
            1000
        ),
        KnobVerdict::Delete
    );
    // Hurts short tasks, helps long ones: a real knob for the policy.
    let preload = Knobs {
        preload: true,
        ..informative
    };
    assert_eq!(verdict(informative, preload, 1000), KnobVerdict::Knob);
    // Not enough data: the classification errs toward the simpler answer.
    assert_eq!(verdict(informative, preload, 300), KnobVerdict::Constant);
}

/// The smallest "policy that turns a knob": a learned threshold on task
/// length. With enough training data it beats both constant settings on
/// unseen cases; the margin is small in this environment.
#[test]
fn a_learned_policy_beats_constants_given_enough_data() {
    let informative = Knobs {
        informative_errors: true,
        ..base()
    };
    let threshold = learn_preload_threshold(&cases(41, 2000, (4, 50)), informative, 1);
    let test = cases(42, 3000, (4, 50));
    let learned = evaluate_policy(&test, informative, |c| c.steps > threshold, 3);
    let always_on = evaluate_policy(&test, informative, |_| true, 3);
    let always_off = evaluate_policy(&test, informative, |_| false, 3);
    assert!(
        learned < always_on && learned < always_off,
        "{learned} {always_on} {always_off}"
    );
}
