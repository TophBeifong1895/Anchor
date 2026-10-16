//! Runs every experiment and prints the results as Markdown. All numbers in
//! README.md come from this program: `cargo run --release --bin report`.

use std::time::Instant;

use constitution_poc::kernel::*;
use constitution_poc::lifecycle::{Derived, Documented, explore, group};
use constitution_poc::sim::*;

struct Quiet;
impl World for Quiet {
    fn perform(&mut self, _: &str, _: &Fields) -> WorldOutcome {
        WorldOutcome::Ok
    }
}

fn main() {
    println!("# constitution-poc report\n");
    lifecycle();
    selection();
    scale();
}

fn lifecycle() {
    println!("## E2 Run lifecycle\n");
    let e = explore(Documented::initial(), Derived::initial());
    println!(
        "- reachable product states: {}, transitions compared: {}, divergences: {}",
        e.states,
        e.transitions,
        e.divergences.len()
    );
    for d in group(&e.divergences) {
        println!(
            "- `{:?}`: documented accepts = {}, derived accepts = {}",
            d.path, d.left_accepts, d.right_accepts
        );
    }
    println!();
}

fn selection() {
    println!("## E3 Measurement and selection\n");
    let base = Knobs::baseline();
    let informative = Knobs {
        informative_errors: true,
        ..base
    };

    let all = cases(7, 2000, (4, 50));
    let terse = diagnose(&all, base, 1);
    let taught = diagnose(&all, informative, 1);
    println!(
        "Diagnostics from counters only ({} sessions):\n",
        terse.sessions
    );
    println!("| wall | true hit probability | collisions | sessions hitting it |");
    println!("| --- | --- | --- | --- |");
    for (i, p) in WALLS.iter().enumerate() {
        println!(
            "| {i} | {p} | {} | {:.1}% |",
            terse.collisions_per_wall[i],
            100.0 * terse.sessions_hitting_wall[i]
        );
    }
    println!(
        "\n- rank correlation with ground truth: {:.2}",
        terse.rank_correlation_with_truth
    );
    println!(
        "- mean recovery distance: terse errors {:.2}, informative errors {:.2}\n",
        terse.mean_recovery_distance, taught.mean_recovery_distance
    );

    let cfg = GateConfig::default();
    let holdout = cases(99, 200, (4, 50));
    let real = (0..20)
        .filter(|t| gate(&holdout, base, informative, cfg, 1000 + t).accepted)
        .count();
    let placebo = Knobs {
        cosmetic: 1,
        ..base
    };
    let trials = 200;
    let (mut gated, mut naive) = (0, 0);
    for t in 0..trials {
        let h = cases(500 + t, 200, (4, 50));
        gated += gate(&h, base, placebo, cfg, t).accepted as u32;
        naive += naive_gate(&h, base, placebo, t) as u32;
    }
    println!("Selection gate (200 hold-out cases × 3 repeats, α = 0.05):\n");
    println!("| candidate | deterministic gate | one run, keep if better |");
    println!("| --- | --- | --- |");
    println!("| informative errors (real) | accepted {real}/20 | — |");
    println!("| cosmetic (placebo) | accepted {gated}/{trials} | accepted {naive}/{trials} |");

    let h = cases(99, 2000, (4, 50));
    let give_up = Knobs {
        give_up_on_wall: true,
        ..base
    };
    let a = evaluate(&h, base, 1, 7);
    let b = evaluate(&h, give_up, 1, 7);
    let per_session = |r: &[CaseResult]| r.iter().map(|x| x.cost).sum::<f64>() / r.len() as f64;
    let decision = gate(&cases(99, 300, (4, 50)), base, give_up, cfg, 9);
    println!(
        "\nGoodhart change (give up at first refusal): cost/session {:.1} → {:.1}, acceptance {:.3} → {:.3}, cost per accepted {:.1} → {:.1}; gate accepted = {}\n",
        per_session(&a),
        per_session(&b),
        acceptance(&a),
        acceptance(&b),
        cost_per_accepted(&a),
        cost_per_accepted(&b),
        decision.accepted
    );

    println!("Knob analysis (effect on cost per accepted result; positive = knob helps):\n");
    println!("| candidate | n per class | short tasks | long tasks | verdict |");
    println!("| --- | --- | --- | --- | --- |");
    let preload = Knobs {
        preload: true,
        ..informative
    };
    let rows = [
        ("informative errors", base, informative),
        (
            "cosmetic",
            informative,
            Knobs {
                cosmetic: 1,
                ..informative
            },
        ),
        ("preload", informative, preload),
    ];
    for n in [300usize, 1000] {
        for (name, b, on) in rows {
            let s = sensitivity(&cases(31, n, (4, 8)), b, on, 1);
            let l = sensitivity(&cases(32, n, (30, 50)), b, on, 2);
            println!(
                "| {name} | {n} | {:+.2} (p={:.3}) | {:+.2} (p={:.3}) | {:?} |",
                s.effect,
                s.p,
                l.effect,
                l.p,
                classify(&[s, l], 0.05)
            );
        }
    }

    println!("\nLearned policy (preload when task length > threshold):\n");
    println!("| training cases | threshold | learned | always on | always off |");
    println!("| --- | --- | --- | --- | --- |");
    for (train, test) in [(300, 3000), (2000, 3000)] {
        let th = learn_preload_threshold(&cases(41, train, (4, 50)), informative, 1);
        let t = cases(42, test, (4, 50));
        println!(
            "| {train} | {th} | {:.2} | {:.2} | {:.2} |",
            evaluate_policy(&t, informative, |c| c.steps > th, 3),
            evaluate_policy(&t, informative, |_| true, 3),
            evaluate_policy(&t, informative, |_| false, 3)
        );
    }
    println!();
}

fn scale() {
    println!("## E4 Scale\n");
    println!("| resources | submissions | history kept | elapsed | per submission | probe keys |");
    println!("| --- | --- | --- | --- | --- | --- |");
    for (resources, submissions, history) in [
        (1_000usize, 100_000usize, false),
        (10_000, 1_000_000, false),
        (100_000, 1_000_000, false),
        (10_000, 1_000_000, true),
    ] {
        let mut k = if history {
            Kernel::new()
        } else {
            Kernel::new().without_history()
        };
        k.install_validator(
            "validator/loose",
            Box::new(ReviewGraph {
                strictness: Strictness::Loose,
            }),
            CONSTITUTION,
        );
        k.add_principal("agent", PrincipalKind::Agent);
        k.grant_root("agent", "run/*", Action::Write);
        let ids: Vec<String> = (0..resources).map(|i| format!("run/{i:05}")).collect();
        for id in &ids {
            k.create(id, "validator/loose", Fields::new());
        }
        let mut world = Quiet;
        let start = Instant::now();
        for i in 0..submissions {
            let id = &ids[i % resources];
            let s = Submission::new(id, k.version(id), Action::Write).field("summary", "step");
            k.submit("agent", None, 1, s, &mut world);
        }
        let elapsed = start.elapsed();
        println!(
            "| {resources} | {submissions} | {history} | {:.2?} | {:.2?} | {} |",
            elapsed,
            elapsed / submissions as u32,
            k.probe.keys()
        );
    }
    println!();
}
