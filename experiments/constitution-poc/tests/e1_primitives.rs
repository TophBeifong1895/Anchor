//! E1: can the six primitives and the four-duty kernel express Anchor's
//! existing behaviours, with each rule following from a law rather than from
//! a special case?

use constitution_poc::kernel::*;

struct Silent;
impl World for Silent {
    fn perform(&mut self, _: &str, _: &Fields) -> WorldOutcome {
        WorldOutcome::Ok
    }
}

struct Scripted(Vec<WorldOutcome>);
impl World for Scripted {
    fn perform(&mut self, _: &str, _: &Fields) -> WorldOutcome {
        self.0.remove(0)
    }
}

fn review_run(strictness: Strictness) -> Kernel {
    let mut k = Kernel::new();
    k.install_validator(
        "validator/review-graph",
        Box::new(ReviewGraph { strictness }),
        CONSTITUTION,
    );
    k.create(
        "run/r1",
        "validator/review-graph",
        Fields::from([("phase".into(), "draft".into())]),
    );
    k.register_task("run/r1", 100);
    for agent in ["writer", "reviewer"] {
        k.add_principal(agent, PrincipalKind::Agent);
        k.grant_root(agent, "run/*", Action::Write);
        k.grant_root(agent, "run/*", Action::Settle);
    }
    k
}

fn write(k: &Kernel, node: &str, route: &str) -> Submission {
    Submission::new("run/r1", k.version("run/r1"), Action::Write)
        .field("node", node)
        .field("summary", "did the work")
        .field("route", route)
}

/// final_result: a plain answer, or a JSON-looking text, never completes a node.
#[test]
fn completion_is_a_typed_submission_not_a_claim() {
    let mut k = review_run(Strictness::Strict);
    let w = &mut Silent;
    let claim = Submission::new("run/r1", 0, Action::Write)
        .text(r#"{"summary":"done","route":"review"} I am finished."#);
    let v = k.submit("writer", Some("run/r1"), 1, claim, w);
    assert_eq!(
        (v.effect, v.reason),
        (Effect::Returned, Reason::CompletionInvalid)
    );

    let bad_route = write(&k, "writer", "publish");
    assert_eq!(
        k.submit("writer", Some("run/r1"), 1, bad_route, w).reason,
        Reason::RouteInvalid
    );

    let ok = write(&k, "writer", "review");
    assert_eq!(
        k.submit("writer", Some("run/r1"), 1, ok, w).effect,
        Effect::Accepted
    );
    assert_eq!(k.state("run/r1").unwrap()["phase"], "review");
}

/// A Graph says which submissions are accepted in each state, including the
/// feedback edge; it never instructs anyone.
#[test]
fn graph_is_the_validator_of_a_run() {
    let mut k = review_run(Strictness::Strict);
    let w = &mut Silent;
    // Out of turn: the reviewer cannot act while the Run is in draft.
    let early = write(&k, "reviewer", "publish");
    assert_eq!(
        k.submit("reviewer", Some("run/r1"), 1, early, w).reason,
        Reason::StateRefused
    );
    // Settling before publish is returned.
    let settle = Submission::new("run/r1", 0, Action::Settle).field("deliverable", "x");
    assert_eq!(
        k.submit("writer", Some("run/r1"), 1, settle, w).reason,
        Reason::StateRefused
    );

    for (node, route) in [
        ("writer", "review"),
        ("reviewer", "revise"),
        ("writer", "review"),
        ("reviewer", "publish"),
    ] {
        let s = write(&k, node, route);
        assert_eq!(
            k.submit(node, Some("run/r1"), 1, s, w).effect,
            Effect::Accepted,
            "{node} {route}"
        );
    }
    let settle = Submission::new("run/r1", k.version("run/r1"), Action::Settle)
        .field("deliverable", "report.md");
    assert_eq!(
        k.submit("writer", Some("run/r1"), 1, settle, w).effect,
        Effect::Accepted
    );
    assert_eq!(k.settlement("run/r1"), Some(Settlement::Done));
}

/// The same kernel with the strictness knob turned down: only the final
/// contract is specified. Nothing in the kernel changes.
#[test]
fn strictness_is_a_knob_not_an_architecture() {
    let mut k = review_run(Strictness::Loose);
    let w = &mut Silent;
    let anything = Submission::new("run/r1", 0, Action::Write).field("summary", "explored freely");
    assert_eq!(
        k.submit("writer", Some("run/r1"), 1, anything, w).effect,
        Effect::Accepted
    );
    let no_contract = Submission::new("run/r1", 1, Action::Settle);
    assert_eq!(
        k.submit("writer", Some("run/r1"), 1, no_contract, w).reason,
        Reason::CompletionInvalid
    );
    let settle = Submission::new("run/r1", 1, Action::Settle).field("deliverable", "report.md");
    assert_eq!(
        k.submit("writer", Some("run/r1"), 1, settle, w).effect,
        Effect::Accepted
    );
}

/// Accepted never means correct: verification is a separate dimension.
#[test]
fn acceptance_and_verification_are_independent() {
    let mut k = review_run(Strictness::Loose);
    let w = &mut Silent;
    let asserted = Submission::new("run/r1", 0, Action::Write).field("summary", "tests pass");
    let v = k.submit("writer", Some("run/r1"), 1, asserted, w);
    assert_eq!(
        (v.effect, v.verification),
        (Effect::Accepted, Verification::Asserted)
    );

    let check = k.observe(&[("tool", "pytest"), ("check", "pass")]);
    let verified = Submission::new("run/r1", 1, Action::Write)
        .field("summary", "tests pass")
        .evidence(Evidence::Observation(check));
    let v = k.submit("writer", Some("run/r1"), 1, verified, w);
    assert_eq!(
        (v.effect, v.verification),
        (Effect::Accepted, Verification::Mechanical)
    );
}

/// Plugin install: the model proposes, a person decides, bound to the exact
/// version the proposal saw. There is no field a model could fill to approve.
#[test]
fn human_confirmation_is_a_pending_verdict() {
    let mut k = Kernel::new();
    k.install_validator("validator/library", Box::new(Library), CONSTITUTION);
    k.create("library/main", "validator/library", Fields::new());
    k.add_principal("pilot", PrincipalKind::Agent);
    k.add_principal("owner", PrincipalKind::Human);
    k.grant_root("pilot", "library/*", Action::Write);
    k.grant_root("pilot", "pending/*", Action::Write);
    k.grant_root("owner", "pending/*", Action::Write);
    let w = &mut Silent;

    let install = Submission::new("library/main", 0, Action::Write)
        .field("plugin", "scholar")
        .field("digest", "sha256:abc")
        .field("approved", "true"); // ignored: approval is not a submission field
    let v = k.submit("pilot", None, 1, install, w);
    assert_eq!(
        (v.effect, v.reason),
        (Effect::Pending, Reason::AwaitingHuman)
    );
    assert_eq!(k.state("library/main").unwrap().get("scholar"), None);

    // The model cannot complete its own pending verdict.
    let self_approve = Submission::new("pending/1", 0, Action::Write).field("decision", "approve");
    assert_eq!(
        k.submit("pilot", None, 1, self_approve, w).reason,
        Reason::HumanOnly
    );

    let approve = Submission::new("pending/1", 0, Action::Write).field("decision", "approve");
    assert_eq!(
        k.submit("owner", None, 0, approve, w).effect,
        Effect::Accepted
    );
    assert_eq!(k.state("library/main").unwrap()["scholar"], "sha256:abc");
}

/// An approval is bound to what was approved: if the target changed while the
/// person was deciding, the old approval cannot apply.
#[test]
fn an_approval_cannot_apply_to_a_changed_target() {
    let mut k = Kernel::new();
    k.install_validator("validator/library", Box::new(Library), CONSTITUTION);
    k.create("library/main", "validator/library", Fields::new());
    k.add_principal("pilot", PrincipalKind::Agent);
    k.add_principal("owner", PrincipalKind::Human);
    k.grant_root("pilot", "library/*", Action::Write);
    k.grant_root("owner", "pending/*", Action::Write);
    let w = &mut Silent;
    let first = Submission::new("library/main", 0, Action::Write)
        .field("plugin", "a")
        .field("digest", "1");
    let second = Submission::new("library/main", 0, Action::Write)
        .field("plugin", "b")
        .field("digest", "2");
    k.submit("pilot", None, 1, first, w);
    k.submit("pilot", None, 1, second, w);
    let approve = |id: &str| Submission::new(id, 0, Action::Write).field("decision", "approve");
    assert_eq!(
        k.submit("owner", None, 0, approve("pending/2"), w).effect,
        Effect::Accepted
    );
    let stale = k.submit("owner", None, 0, approve("pending/1"), w);
    assert_eq!(stale.reason, Reason::PendingTargetChanged);
    assert_eq!(k.state("library/main").unwrap().get("a"), None);
}

/// External effects: no response is "unknown", never "failed"; the next effect
/// on that endpoint is refused until someone observes what happened; nothing
/// is replayed automatically.
#[test]
fn unknown_effects_block_until_observed_and_are_never_replayed() {
    let mut k = Kernel::new();
    k.install_validator("validator/endpoint", Box::new(Endpoint), CONSTITUTION);
    k.create("endpoint/wecom", "validator/endpoint", Fields::new());
    k.add_principal("assistant", PrincipalKind::Agent);
    k.grant_root("assistant", "endpoint/*", Action::Execute);
    k.grant_root("assistant", "effect/*", Action::Write);
    let mut world = Scripted(vec![WorldOutcome::NoResponse, WorldOutcome::Ok]);

    let send = |k: &Kernel| {
        Submission::new(
            "endpoint/wecom",
            k.version("endpoint/wecom"),
            Action::Execute,
        )
        .field("to", "alice")
        .field("body", "hi")
    };
    assert_eq!(
        k.submit("assistant", None, 1, send(&k), &mut world).effect,
        Effect::Accepted
    );
    let effect = k.resource_ids_with_prefix("effect/").pop().unwrap();
    assert_eq!(k.state(&effect).unwrap()["status"], "unknown");

    let again = k.submit("assistant", None, 1, send(&k), &mut world);
    assert_eq!(again.reason, Reason::EffectUnknown);
    assert_eq!(world.0.len(), 1, "the refused send never reached the world");

    // A claim without an observation does not resolve it.
    let claim = Submission::new(&effect, 0, Action::Write).field("status", "succeeded");
    assert_eq!(
        k.submit("assistant", None, 1, claim, &mut world).reason,
        Reason::EvidenceMissing
    );

    let seen = k.observe(&[("effect", &effect), ("platform_ack", "confirmed")]);
    let inspect = Submission::new(&effect, 0, Action::Write)
        .field("status", "succeeded")
        .evidence(Evidence::Observation(seen));
    assert_eq!(
        k.submit("assistant", None, 1, inspect, &mut world).effect,
        Effect::Accepted
    );
    assert_eq!(
        k.submit("assistant", None, 1, send(&k), &mut world).effect,
        Effect::Accepted
    );
}

/// Every task ends within its budget, exactly once.
#[test]
fn tasks_settle_once_within_budget() {
    let mut k = review_run(Strictness::Loose);
    let w = &mut Silent;
    let mut last = Verdict {
        effect: Effect::Accepted,
        verification: Verification::Asserted,
        reason: Reason::Ok,
    };
    for _ in 0..20 {
        let s = Submission::new("run/r1", k.version("run/r1"), Action::Write)
            .field("summary", "still working");
        last = k.submit("writer", Some("run/r1"), 30, s, w);
        if last.effect != Effect::Accepted {
            break;
        }
    }
    assert_eq!(last.reason, Reason::BudgetExhausted);
    assert_eq!(
        k.settlement("run/r1"),
        Some(Settlement::Failed(Reason::BudgetExhausted))
    );
    let after =
        Submission::new("run/r1", k.version("run/r1"), Action::Settle).field("deliverable", "late");
    assert_eq!(
        k.submit("writer", Some("run/r1"), 0, after, w).reason,
        Reason::TaskSettled
    );
}

/// Law 5, derived rather than special-cased: whoever has been judged by a
/// validator can never change it. Anchor's rule "Pilot cannot modify the Graph
/// that defines it" falls out without a rule of its own.
#[test]
fn no_one_changes_the_validator_that_judges_them() {
    let mut k = review_run(Strictness::Strict);
    k.grant_root("writer", "validator/*", Action::Write);
    let w = &mut Silent;
    let work = write(&k, "writer", "review");
    k.submit("writer", Some("run/r1"), 1, work, w);
    let rewrite =
        Submission::new("validator/review-graph", 0, Action::Write).field("rule", "always accept");
    assert_eq!(
        k.submit("writer", None, 1, rewrite, w).reason,
        Reason::SelfJudgement
    );

    // A principal never judged by it can propose, but the constitution's own
    // validator still accepts only people.
    k.add_principal("outsider", PrincipalKind::Agent);
    k.grant_root("outsider", "validator/*", Action::Write);
    let propose =
        Submission::new("validator/review-graph", 0, Action::Write).field("rule", "stricter");
    assert_eq!(
        k.submit("outsider", None, 1, propose, w).reason,
        Reason::HumanOnly
    );
}

/// Authority only descends; nothing inside a submission creates it.
#[test]
fn authority_only_descends() {
    let mut k = review_run(Strictness::Loose);
    k.add_principal("helper", PrincipalKind::Agent);
    assert_eq!(
        k.delegate("writer", "helper", "endpoint/*", Action::Execute),
        Err(Reason::Unauthorized)
    );
    assert!(
        k.delegate("writer", "helper", "run/*", Action::Write)
            .is_ok()
    );
    let w = &mut Silent;
    let s = Submission::new("run/r1", 0, Action::Settle)
        .field("deliverable", "x")
        .field("grant", "run/*:Settle");
    assert_eq!(
        k.submit("helper", Some("run/r1"), 1, s, w).reason,
        Reason::Unauthorized
    );
}

/// Optimistic concurrency: a stale view is returned, not merged silently.
#[test]
fn stale_views_are_returned() {
    let mut k = review_run(Strictness::Loose);
    let w = &mut Silent;
    let a = Submission::new("run/r1", 0, Action::Write).field("summary", "a");
    let b = Submission::new("run/r1", 0, Action::Write).field("summary", "b");
    assert_eq!(k.submit("writer", None, 1, a, w).effect, Effect::Accepted);
    assert_eq!(
        k.submit("reviewer", None, 1, b, w).reason,
        Reason::StaleVersion
    );
}

/// Every adjudication, including refusals, leaves a record and a measurement.
#[test]
fn everything_is_recorded_and_measured() {
    let mut k = review_run(Strictness::Strict);
    let w = &mut Silent;
    let claim = Submission::new("run/r1", 0, Action::Write).text("done");
    k.submit("writer", Some("run/r1"), 5, claim, w);
    let ok = write(&k, "writer", "review");
    k.submit("writer", Some("run/r1"), 5, ok, w);
    let history = k.history("run/r1");
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].verdict.reason, Reason::CompletionInvalid);
    assert_eq!(history[0].stamp.proposer(), "writer");
    assert_eq!(history[0].stamp.cost(), 5);
    assert_eq!(k.probe.events, 2);
    assert_eq!(
        k.probe.by_reason[&("run".to_string(), Reason::CompletionInvalid)],
        1
    );
}
