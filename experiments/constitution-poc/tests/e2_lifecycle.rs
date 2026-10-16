//! E2: the documented Run lifecycle (stop / resume / retire / abandon, each
//! with its own preconditions) against the same behaviour derived from three
//! laws and one Session-owned pointer.

use constitution_poc::lifecycle::*;

#[test]
fn derived_lifecycle_matches_documented_rules_except_after_retirement() {
    let exploration = explore(Documented::initial(), Derived::initial());
    assert_eq!(exploration.states, 19, "reachable product states");
    let grouped = group(&exploration.divergences);
    let paths: Vec<Vec<Event>> = grouped.iter().map(|d| d.path.clone()).collect();
    assert_eq!(
        paths,
        vec![
            vec![Event::Stop, Event::Retire, Event::Stop],
            vec![Event::Stop, Event::Retire, Event::Abandon],
        ],
        "every divergence is an operation on a retired, stopped Run"
    );
    // In both, the documented rules accept and the derived model refuses.
    assert!(grouped.iter().all(|d| d.left_accepts && !d.right_accepts));
}

/// The one rule the documented model states separately ("a retired Run cannot
/// resume") is not a rule in the derived model: retirement settles the task,
/// and a settled task is irreversible.
#[test]
fn retired_runs_never_resume_without_a_dedicated_rule() {
    let derived = Derived::initial()
        .step(Event::Stop)
        .and_then(|d| d.step(Event::Retire))
        .unwrap();
    assert_eq!(derived.step(Event::Resume), None);
    assert_eq!(derived.phase, Phase::Settled(Outcome::Retired));
}

/// Retirement waits for unobserved effects: the same law that blocks a second
/// send also blocks retirement, with no rule specific to retirement.
#[test]
fn unknown_delivery_blocks_retirement_by_the_effect_law() {
    let lost = Derived::initial()
        .step(Event::DeliverStart)
        .and_then(|d| d.step(Event::DeliverLost))
        .and_then(|d| d.step(Event::Stop))
        .unwrap();
    assert_eq!(lost.step(Event::Retire), None);
    let inspected = lost.step(Event::Inspect).unwrap();
    assert!(inspected.step(Event::Retire).is_some());
}
