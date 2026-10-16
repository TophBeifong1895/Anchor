//! The constitution, as tests against the kernel alone.

use std::sync::{Arc, Mutex};

use anchor_kernel::*;
use serde_json::{Value, json};

/// Grants by (principal id, action, target prefix).
struct Grants(Vec<(&'static str, Action, &'static str)>);
impl Authority for Grants {
    fn permits(&self, principal: &Principal, action: Action, target: &str) -> bool {
        self.0
            .iter()
            .any(|(p, a, prefix)| *p == principal.id && *a == action && target.starts_with(prefix))
    }
}

fn all_grants() -> Box<Grants> {
    let mut grants = Vec::new();
    for who in ["agent", "owner", "host", "optimizer"] {
        for action in [
            Action::Write,
            Action::Request,
            Action::Execute,
            Action::Settle,
        ] {
            grants.push((who, action, ""));
        }
    }
    Box::new(Grants(grants))
}

fn agent() -> Principal {
    Principal::new("agent", PrincipalKind::Agent)
}
fn owner() -> Principal {
    Principal::new("owner", PrincipalKind::Human)
}
fn host() -> Principal {
    Principal::new("host", PrincipalKind::Code)
}

/// A tiny Graph-like validator: draft → review → publish, with a feedback
/// route, expressed as which slot is open in which state. It instructs no one.
struct Review;
impl Validator for Review {
    fn open(&self, resource: &Resource) -> Vec<Slot> {
        let slot = match resource.state["phase"].as_str() {
            Some("draft") => "writer",
            Some("review") => "reviewer",
            Some("publish") => "publish",
            _ => return Vec::new(),
        };
        vec![Slot {
            id: slot.into(),
            action: if slot == "publish" {
                Action::Settle
            } else {
                Action::Write
            },
        }]
    }

    fn judge(&self, ctx: &JudgeCtx, s: &Submission) -> Judgement {
        let open = self.open(ctx.resource);
        let Some(slot) = open
            .iter()
            .find(|o| Some(&o.id) == s.slot.as_ref() && o.action == s.action)
        else {
            return Judgement::refused(Reason::SlotClosed);
        };
        if slot.action == Action::Settle {
            return if s.content.get("deliverable").is_some() {
                Judgement::accept(ctx.resource.state.clone(), ctx.verification_of(s))
            } else {
                Judgement::returned(Reason::ContentInvalid)
            };
        }
        let (Some(_summary), Some(route)) = (
            s.content.get("summary").and_then(Value::as_str),
            s.content.get("route").and_then(Value::as_str),
        ) else {
            return Judgement::returned(Reason::ContentInvalid);
        };
        let next = match (slot.id.as_str(), route) {
            ("writer", "review") => "review",
            ("reviewer", "revise") => "draft",
            ("reviewer", "publish") => "publish",
            _ => return Judgement::returned(Reason::RouteInvalid),
        };
        Judgement::accept(json!({"phase": next}), ctx.verification_of(s))
    }
}

/// Installs need a person and bind to what was proposed.
struct Library;
impl Validator for Library {
    fn judge(&self, ctx: &JudgeCtx, s: &Submission) -> Judgement {
        let (Some(plugin), Some(digest)) = (s.content.get("plugin"), s.content.get("digest"))
        else {
            return Judgement::returned(Reason::ContentInvalid);
        };
        let mut state = ctx.resource.state.clone();
        state[plugin.as_str().unwrap()] = digest.clone();
        Judgement::pending(state)
    }
}

struct Endpoint;
impl Validator for Endpoint {
    fn judge(&self, ctx: &JudgeCtx, s: &Submission) -> Judgement {
        if s.content.get("to").is_none() {
            return Judgement::returned(Reason::ContentInvalid);
        }
        Judgement::accept(ctx.resource.state.clone(), Verification::NotApplicable)
    }
}

/// Host control: people stop and resume a task; it is a submission like any
/// other, not a flag a scheduler polls.
struct Control;
impl Validator for Control {
    fn judge(&self, _ctx: &JudgeCtx, _s: &Submission) -> Judgement {
        Judgement::refused(Reason::Unclassified)
    }
}

fn kernel() -> Kernel<MemoryStore> {
    let mut k = Kernel::new(MemoryStore::default(), all_grants()).unwrap();
    k.install("validator/review", Arc::new(Review)).unwrap();
    k.install("validator/library", Arc::new(Library)).unwrap();
    k.install("validator/endpoint", Arc::new(Endpoint)).unwrap();
    k.install("validator/control", Arc::new(Control)).unwrap();
    k.create(Resource::new("run/1", "validator/review", json!({"phase": "draft"})).task(100))
        .unwrap();
    k
}

fn version(k: &Kernel<MemoryStore>, id: &str) -> u64 {
    k.load(id).unwrap().unwrap().version
}

fn work(k: &Kernel<MemoryStore>, slot: &str, route: &str) -> Submission {
    Submission::new("run/1", version(k, "run/1"), Action::Write)
        .slot(slot)
        .content(json!({"summary": "did it", "route": route}))
}

#[test]
fn completion_is_a_typed_submission_not_a_claim() {
    let k = kernel();
    let claim = Submission::new("run/1", 0, Action::Write)
        .slot("writer")
        .content(json!(
            r#"{"summary":"done","route":"review"} I am finished."#
        ));
    assert_eq!(
        k.submit(&agent(), 1, claim).unwrap().reason,
        Reason::ContentInvalid
    );
    assert_eq!(
        k.submit(&agent(), 1, work(&k, "writer", "publish"))
            .unwrap()
            .reason,
        Reason::RouteInvalid
    );
    assert!(
        k.submit(&agent(), 1, work(&k, "writer", "review"))
            .unwrap()
            .is_accepted()
    );
}

#[test]
fn a_graph_opens_slots_and_judges_them_without_instructing_anyone() {
    let k = kernel();
    assert_eq!(k.open("run/1").unwrap()[0].id, "writer");
    assert_eq!(
        k.submit(&agent(), 1, work(&k, "reviewer", "publish"))
            .unwrap()
            .reason,
        Reason::SlotClosed
    );
    for (slot, route) in [
        ("writer", "review"),
        ("reviewer", "revise"),
        ("writer", "review"),
        ("reviewer", "publish"),
    ] {
        assert!(
            k.submit(&agent(), 1, work(&k, slot, route))
                .unwrap()
                .is_accepted(),
            "{slot}"
        );
    }
    let settle = Submission::new("run/1", version(&k, "run/1"), Action::Settle)
        .slot("publish")
        .content(json!({"deliverable": "report.md"}));
    assert!(k.submit(&agent(), 1, settle).unwrap().is_accepted());
    let run = k.load("run/1").unwrap().unwrap();
    assert_eq!(
        run.phase(),
        Some(&Phase::Settled {
            result: Outcome::Done
        })
    );
    assert!(
        k.open("run/1").unwrap().is_empty(),
        "a settled task offers nothing"
    );
}

#[test]
fn accepted_is_not_correct() {
    let k = kernel();
    let asserted = k.submit(&agent(), 1, work(&k, "writer", "review")).unwrap();
    assert_eq!(
        (asserted.effect, asserted.verification),
        (Effect::Accepted, Verification::Asserted)
    );
    let evidenced = k
        .submit(
            &agent(),
            1,
            work(&k, "reviewer", "publish").evidence(Evidence::Observation {
                id: "pytest-42".into(),
            }),
        )
        .unwrap();
    assert_eq!(evidenced.verification, Verification::Evidenced);
}

fn control(k: &Kernel<MemoryStore>, who: &Principal, what: &str) -> Verdict {
    let s = Submission::new("run/1", version(k, "run/1"), Action::Request)
        .content(json!({"control": what}));
    k.submit(who, 0, s).unwrap()
}

/// One rule replaces every stop/pause check: a waiting task offers no slots and
/// refuses agents. Stop is a person's submission, not a flag a loop polls.
#[test]
fn waiting_is_one_rule_not_scattered_checks() {
    let k = kernel();
    assert_eq!(control(&k, &agent(), "stop").reason, Reason::HumanOnly);
    assert!(control(&k, &owner(), "stop").is_accepted());
    assert!(k.open("run/1").unwrap().is_empty());
    assert_eq!(
        k.submit(&agent(), 1, work(&k, "writer", "review"))
            .unwrap()
            .reason,
        Reason::TaskWaiting
    );
    assert!(control(&k, &owner(), "resume").is_accepted());
    assert_eq!(k.open("run/1").unwrap()[0].id, "writer");
    assert!(
        k.submit(&agent(), 1, work(&k, "writer", "review"))
            .unwrap()
            .is_accepted()
    );
}

/// The lifecycle the documented rules spell out per verb, from one law.
#[test]
fn the_task_lifecycle_needs_no_rule_per_verb() {
    let k = kernel();
    assert_eq!(
        control(&k, &owner(), "resume").reason,
        Reason::NotWaiting,
        "resume a running task"
    );
    assert!(control(&k, &owner(), "stop").is_accepted());
    assert!(
        control(&k, &owner(), "stop").is_accepted(),
        "stop is idempotent in effect"
    );
    assert!(control(&k, &owner(), "abandon").is_accepted());
    let run = k.load("run/1").unwrap().unwrap();
    assert!(matches!(
        run.phase(),
        Some(Phase::Settled {
            result: Outcome::Aborted { .. }
        })
    ));
    // Settled is irreversible: no resume, no second abandon, no stop.
    for verb in ["resume", "abandon", "stop"] {
        assert_eq!(
            control(&k, &owner(), verb).reason,
            Reason::TaskSettled,
            "{verb}"
        );
    }
}

/// A task waiting on a child is released by host code, not by a person's
/// resume and not by an agent.
#[test]
fn whoever_a_task_waits_on_releases_it() {
    struct CallGraph;
    impl Validator for CallGraph {
        fn judge(&self, ctx: &JudgeCtx, s: &Submission) -> Judgement {
            let child = s.content["call"].as_str().unwrap_or_default().to_owned();
            Judgement::accept(ctx.resource.state.clone(), Verification::Asserted).with_phase(
                Phase::Waiting {
                    waiter: Waiter::Child { task: child },
                },
            )
        }
    }
    let mut k = kernel();
    k.install("validator/call", Arc::new(CallGraph)).unwrap();
    k.create(Resource::new("run/p", "validator/call", json!({})).task(10))
        .unwrap();
    let call = Submission::new("run/p", 0, Action::Write).content(json!({"call": "run/child"}));
    assert!(k.submit(&agent(), 0, call).unwrap().is_accepted());
    let resume = |who: &Principal| {
        let s = Submission::new("run/p", version(&k, "run/p"), Action::Request)
            .content(json!({"control": "resume"}));
        k.submit(who, 0, s).unwrap()
    };
    assert_eq!(resume(&agent()).reason, Reason::TaskWaiting);
    assert_eq!(resume(&owner()).reason, Reason::Unauthorized);
    assert!(resume(&host()).is_accepted());
}

#[test]
fn budget_settles_the_task_exactly_once() {
    let k = kernel();
    let mut last = None;
    for _ in 0..10 {
        let v = k
            .submit(&agent(), 40, work(&k, "writer", "review"))
            .unwrap();
        last = Some(v);
        if !v.is_accepted() {
            break;
        }
        // go back to draft so the writer slot reopens
        k.submit(&agent(), 0, work(&k, "reviewer", "revise"))
            .unwrap();
    }
    assert_eq!(last.unwrap().reason, Reason::BudgetExhausted);
    let run = k.load("run/1").unwrap().unwrap();
    assert!(matches!(
        run.phase(),
        Some(Phase::Settled {
            result: Outcome::Failed {
                reason: Reason::BudgetExhausted,
                ..
            }
        })
    ));
    assert_eq!(
        k.submit(&owner(), 0, work(&k, "writer", "review"))
            .unwrap()
            .reason,
        Reason::TaskSettled
    );
}

#[test]
fn people_decide_pending_changes_bound_to_what_they_saw() {
    let k = kernel();
    k.create(Resource::new(
        "library/main",
        "validator/library",
        json!({}),
    ))
    .unwrap();
    let install = |plugin: &str| {
        Submission::new("library/main", 0, Action::Write).content(
            json!({"plugin": plugin, "digest": format!("sha256:{plugin}"), "approved": true}),
        )
    };
    assert_eq!(
        k.submit(&agent(), 1, install("a")).unwrap().effect,
        Effect::Pending
    );
    assert_eq!(
        k.submit(&agent(), 1, install("b")).unwrap().effect,
        Effect::Pending
    );
    assert_eq!(
        k.load("library/main").unwrap().unwrap().state,
        json!({}),
        "approval is not a field"
    );

    let pendings = k.store().list("pending/library/main/").unwrap();
    assert_eq!(pendings.len(), 2);
    let decide =
        |id: &str| Submission::new(id, 0, Action::Write).content(json!({"decision": "approve"}));
    assert_eq!(
        k.submit(&agent(), 0, decide(&pendings[0])).unwrap().reason,
        Reason::HumanOnly
    );
    assert!(
        k.submit(&owner(), 0, decide(&pendings[1]))
            .unwrap()
            .is_accepted()
    );
    assert_eq!(
        k.load("library/main").unwrap().unwrap().state["b"],
        "sha256:b"
    );
    assert_eq!(
        k.submit(&owner(), 0, decide(&pendings[0])).unwrap().reason,
        Reason::PendingTargetChanged
    );
    // A decided pending cannot be decided again.
    assert_eq!(
        k.submit(&owner(), 0, decide(&pendings[1])).unwrap().effect,
        Effect::Returned
    );
}

#[test]
fn unknown_effects_block_until_observed_and_are_never_replayed() {
    let k = kernel();
    k.create(Resource::new(
        "endpoint/wecom",
        "validator/endpoint",
        json!({}),
    ))
    .unwrap();
    let send = |k: &Kernel<MemoryStore>| {
        Submission::new(
            "endpoint/wecom",
            version(k, "endpoint/wecom"),
            Action::Execute,
        )
        .content(json!({"to": "alice"}))
    };
    assert!(k.submit(&agent(), 1, send(&k)).unwrap().is_accepted());
    let effect = k
        .store()
        .list("effect/endpoint/wecom/")
        .unwrap()
        .pop()
        .unwrap();
    let status = |s: &str| {
        Submission::new(&effect, version(&k, &effect), Action::Write).content(json!({"status": s}))
    };
    assert_eq!(
        k.submit(&agent(), 0, status("started")).unwrap().reason,
        Reason::Unauthorized,
        "agents do not report effects"
    );
    assert!(
        k.submit(&host(), 0, status("started"))
            .unwrap()
            .is_accepted()
    );
    assert!(
        k.submit(&host(), 0, status("unknown"))
            .unwrap()
            .is_accepted()
    );

    assert_eq!(
        k.submit(&agent(), 1, send(&k)).unwrap().reason,
        Reason::EffectUnknown
    );
    assert_eq!(
        k.submit(&host(), 0, status("succeeded")).unwrap().reason,
        Reason::EvidenceMissing
    );
    let observed = status("succeeded").evidence(Evidence::Observation { id: "ack-7".into() });
    assert!(k.submit(&host(), 0, observed).unwrap().is_accepted());
    assert!(k.submit(&agent(), 1, send(&k)).unwrap().is_accepted());
}

#[test]
fn only_people_change_validators() {
    let k = kernel();
    let change = Submission::new("validator/review", 0, Action::Write)
        .content(json!({"rule": "accept all"}));
    assert_eq!(
        k.submit(&agent(), 0, change.clone()).unwrap().reason,
        Reason::HumanOnly
    );
    let optimizer = Principal::new("optimizer", PrincipalKind::Code);
    assert_eq!(
        k.submit(&optimizer, 0, change.clone()).unwrap().reason,
        Reason::HumanOnly
    );
    assert!(k.submit(&owner(), 0, change).unwrap().is_accepted());
}

#[test]
fn authority_comes_from_the_host_not_the_submission() {
    let k = Kernel::new(
        MemoryStore::default(),
        Box::new(Grants(vec![("agent", Action::Write, "run/")])),
    )
    .unwrap();
    let mut k = k;
    k.install("validator/review", Arc::new(Review)).unwrap();
    k.create(Resource::new("run/1", "validator/review", json!({"phase":"publish"})).task(10))
        .unwrap();
    let settle = Submission::new("run/1", 0, Action::Settle)
        .slot("publish")
        .content(json!({"deliverable": "x", "grant": "settle"}));
    assert_eq!(
        k.submit(&agent(), 0, settle).unwrap().reason,
        Reason::Unauthorized
    );
}

#[test]
fn stale_views_are_returned_not_merged() {
    let k = kernel();
    let a = work(&k, "writer", "review");
    let b = work(&k, "writer", "review");
    assert!(k.submit(&agent(), 1, a).unwrap().is_accepted());
    assert_eq!(
        k.submit(&agent(), 1, b).unwrap().reason,
        Reason::StaleVersion
    );
}

#[derive(Default, Clone)]
struct Counter(Arc<Mutex<Vec<Point>>>);
impl Probe for Counter {
    fn observe(&self, point: &Point) {
        self.0.lock().unwrap().push(point.clone());
    }
}

#[test]
fn every_verdict_is_recorded_and_measured_including_refusals() {
    let counter = Counter::default();
    let k = kernel().with_probe(Box::new(counter.clone()));
    k.set_knobs("knobs-v2");
    k.submit(
        &agent(),
        3,
        Submission::new("run/1", 0, Action::Write).slot("writer"),
    )
    .unwrap();
    k.submit(&agent(), 4, work(&k, "writer", "review")).unwrap();
    let history = k.history("run/1").unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].verdict.reason, Reason::ContentInvalid);
    assert!(history[0].after.is_none());
    assert_eq!(history[1].stamp.cost(), 4);
    let points = counter.0.lock().unwrap();
    assert_eq!(points.len(), 2);
    assert_eq!(points[0].reason, Reason::ContentInvalid);
    assert_eq!(points[1].knobs, "knobs-v2");
    assert_eq!(points[1].component, "run");
}

// ---------------------------------------------------------------------------
// Persistence: beyond the in-memory PoC
// ---------------------------------------------------------------------------

fn file_kernel(root: &std::path::Path) -> Kernel<FileStore> {
    let mut k = Kernel::new(FileStore::new(root).unwrap(), all_grants()).unwrap();
    k.install("validator/review", Arc::new(Review)).unwrap();
    k
}

#[test]
fn file_store_survives_restart_with_full_history() {
    let dir = tempfile::tempdir().unwrap();
    {
        let k = file_kernel(dir.path());
        k.create(Resource::new("run/a/b", "validator/review", json!({"phase": "draft"})).task(100))
            .unwrap();
        let s = Submission::new("run/a/b", 0, Action::Write)
            .slot("writer")
            .content(json!({"summary": "x", "route": "review"}));
        assert!(k.submit(&agent(), 2, s).unwrap().is_accepted());
    }
    let k = file_kernel(dir.path());
    let run = k.load("run/a/b").unwrap().unwrap();
    assert_eq!(
        (run.version, run.state["phase"].clone()),
        (1, json!("review"))
    );
    assert_eq!(run.task.unwrap().budget.spent, 2);
    assert_eq!(k.history("run/a/b").unwrap().len(), 1);
    assert_eq!(k.open("run/a/b").unwrap()[0].id, "reviewer");
}

/// Crash after the record is durable but before the state file is replaced:
/// the next load serves the recorded result. Nothing is re-judged.
#[test]
fn crash_between_record_and_state_recovers_by_reading_the_record() {
    let dir = tempfile::tempdir().unwrap();
    let k = file_kernel(dir.path());
    k.create(Resource::new("run/c", "validator/review", json!({"phase": "draft"})).task(100))
        .unwrap();
    let state_path = dir.path().join("run%2Fc/resource.json");
    let before = std::fs::read(&state_path).unwrap();
    let s = Submission::new("run/c", 0, Action::Write)
        .slot("writer")
        .content(json!({"summary": "x", "route": "review"}));
    assert!(k.submit(&agent(), 1, s).unwrap().is_accepted());
    std::fs::write(&state_path, before).unwrap(); // simulate the lost state write
    let reloaded = file_kernel(dir.path()).load("run/c").unwrap().unwrap();
    assert_eq!(
        (reloaded.version, reloaded.state["phase"].clone()),
        (1, json!("review"))
    );
}

/// A torn final history line (crash mid-append) is not a record.
#[test]
fn a_torn_history_line_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let k = file_kernel(dir.path());
    k.create(Resource::new("run/t", "validator/review", json!({"phase": "draft"})).task(100))
        .unwrap();
    let s = Submission::new("run/t", 0, Action::Write)
        .slot("writer")
        .content(json!({"summary": "x", "route": "review"}));
    k.submit(&agent(), 1, s).unwrap();
    let history = dir.path().join("run%2Ft/history.jsonl");
    let mut bytes = std::fs::read(&history).unwrap();
    bytes.extend_from_slice(br#"{"stamp":{"princ"#);
    std::fs::write(&history, bytes).unwrap();
    assert_eq!(file_kernel(dir.path()).history("run/t").unwrap().len(), 1);
}

#[test]
fn ids_cannot_escape_the_store_root() {
    let dir = tempfile::tempdir().unwrap();
    let k = file_kernel(dir.path());
    k.create(Resource::new(
        "../../etc/x",
        "validator/review",
        Value::Null,
    ))
    .unwrap();
    assert!(dir.path().join("%2E%2E%2F%2E%2E%2Fetc%2Fx").exists());
    assert_eq!(
        k.store().list("../").unwrap(),
        vec!["../../etc/x".to_string()]
    );
}
