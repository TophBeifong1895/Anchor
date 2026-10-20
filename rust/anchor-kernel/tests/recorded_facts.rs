//! Facts the kernel records but never judges by: time, cause, configuration
//! and creation, plus the read-only history view for observers.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anchor_kernel::*;
use serde_json::{Value, json};

struct AllowAll;
impl Authority for AllowAll {
    fn permits(&self, principal: &Principal, _: Action, _: &str) -> bool {
        principal.id != "outsider"
    }
}

/// Starts at `start` and advances one millisecond per reading.
struct Stepping(Arc<AtomicU64>);
impl Clock for Stepping {
    fn now_ms(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst)
    }
}

fn stepping(start: u64) -> Box<Stepping> {
    Box::new(Stepping(Arc::new(AtomicU64::new(start))))
}

/// Accepts writes as the new state, executes and settles; a write of
/// `{"ask": true}` needs a person.
struct Ledger;
impl Validator for Ledger {
    fn judge(&self, _: &JudgeCtx, submission: &Submission) -> Judgement {
        match submission.action {
            Action::Write if submission.content["ask"] == true => {
                Judgement::pending(json!({"approved": true}))
            }
            Action::Write | Action::Execute | Action::Settle => {
                Judgement::accept(submission.content.clone(), Verification::Asserted)
            }
            Action::Request => Judgement::returned(Reason::ContentInvalid),
        }
    }
}

fn kernel<S: Store>(store: S, start: u64) -> Kernel<S> {
    let mut k = Kernel::new(store, Box::new(AllowAll)).unwrap();
    k.install("validator/ledger", Arc::new(Ledger)).unwrap();
    k.with_clock(stepping(start))
}

fn host() -> Principal {
    Principal::new("host", PrincipalKind::Code)
}
fn agent() -> Principal {
    Principal::new("agent", PrincipalKind::Agent)
}
fn owner() -> Principal {
    Principal::new("owner", PrincipalKind::Human)
}

fn write(k: &Kernel<impl Store>, id: &str, content: Value) -> Submission {
    let seen = k.load(id).unwrap().unwrap().version;
    Submission::new(id, seen, Action::Write).content(content)
}

#[derive(Default, Clone)]
struct Points(Arc<Mutex<Vec<Point>>>);
impl Probe for Points {
    fn observe(&self, point: &Point) {
        self.0.lock().unwrap().push(point.clone());
    }
}

#[test]
fn the_kernel_clock_stamps_every_record_and_seq_keeps_the_order() {
    let k = kernel(MemoryStore::default(), 1_000);
    k.create(Resource::new("run/1", "validator/ledger", json!({})).task(100))
        .unwrap();
    k.submit(&agent(), 1, write(&k, "run/1", json!({"n": 1})))
        .unwrap();
    // Refusals are stamped too.
    k.submit(&agent(), 0, Submission::new("run/1", 0, Action::Write))
        .unwrap();
    let history = k.history("run/1").unwrap();
    let stamps: Vec<_> = history
        .iter()
        .map(|r| (r.stamp.seq(), r.stamp.at()))
        .collect();
    assert_eq!(stamps, vec![(1, Some(1_001)), (2, Some(1_002))]);
    assert_eq!(history[1].verdict.reason, Reason::StaleVersion);
}

#[test]
fn cause_and_configuration_are_recorded_as_given_including_refusals() {
    let points = Points::default();
    let k = kernel(MemoryStore::default(), 0).with_probe(Box::new(points.clone()));
    k.create(Resource::new("run/1", "validator/ledger", json!({})).task(100))
        .unwrap();
    let configured = agent().with_config("cfg-a");
    k.submit(
        &configured,
        1,
        write(&k, "run/1", json!({"n": 1})).cause("tool/run/1", 7),
    )
    .unwrap();
    let outsider = Principal::new("outsider", PrincipalKind::Agent).with_config("cfg-b");
    let refused = k
        .submit(&outsider, 1, write(&k, "run/1", json!({"n": 2})))
        .unwrap();
    assert_eq!(refused.reason, Reason::Unauthorized);

    let history = k.history("run/1").unwrap();
    assert_eq!(
        history[0].submission.cause,
        Some(Cause {
            resource: "tool/run/1".into(),
            seq: 7
        })
    );
    assert_eq!(history[0].stamp.config(), Some("cfg-a"));
    assert_eq!(history[1].submission.cause, None);
    assert_eq!(history[1].stamp.config(), Some("cfg-b"));
    let points = points.0.lock().unwrap();
    let recorded: Vec<_> = points
        .iter()
        .map(|p| (p.config.as_deref(), p.at.is_some()))
        .collect();
    assert_eq!(recorded, vec![(Some("cfg-a"), true), (Some("cfg-b"), true)]);
}

#[test]
fn creation_is_stamped_by_the_kernel_and_survives_every_change() {
    let root = tempfile::tempdir().unwrap();
    let k = kernel(FileStore::new(root.path()).unwrap(), 5_000);
    // A creator cannot supply the time; only the configuration digest.
    let mut proposed = Resource::new("run/1", "validator/ledger", json!({})).task(100);
    proposed.created = Some(Creation {
        at: Some(1),
        config: Some("cfg-run".into()),
    });
    k.create(proposed).unwrap();
    let expected = Some(Creation {
        at: Some(5_000),
        config: Some("cfg-run".into()),
    });
    assert_eq!(k.load("run/1").unwrap().unwrap().created, expected);

    k.submit(&agent(), 1, write(&k, "run/1", json!({"n": 1})))
        .unwrap();
    let seen = k.load("run/1").unwrap().unwrap().version;
    assert!(
        k.submit(&host(), 1, Submission::new("run/1", seen, Action::Settle))
            .unwrap()
            .is_accepted()
    );
    let settled = k.load("run/1").unwrap().unwrap();
    assert!(settled.is_settled());
    assert_eq!(settled.created, expected);
    assert!(
        k.history("run/1")
            .unwrap()
            .iter()
            .filter_map(|r| r.after.as_ref())
            .all(|after| after.created == expected)
    );

    // Reopening the store reads the same creation.
    drop(k);
    let reopened = kernel(FileStore::new(root.path()).unwrap(), 9_000);
    assert_eq!(reopened.load("run/1").unwrap().unwrap().created, expected);
}

#[test]
fn resources_the_kernel_creates_share_the_stamp_of_their_submission() {
    let k = kernel(MemoryStore::default(), 100);
    k.create(Resource::new("run/1", "validator/ledger", json!({})).task(100))
        .unwrap();
    let seen = k.load("run/1").unwrap().unwrap().version;
    k.submit(
        &host().with_config("cfg-exec"),
        0,
        Submission::new("run/1", seen, Action::Execute),
    )
    .unwrap();
    let stamp = k.history("run/1").unwrap()[0].stamp.clone();
    let effect = k.store().list("effect/run/1/").unwrap();
    assert_eq!(effect.len(), 1);
    assert_eq!(
        k.load(&effect[0]).unwrap().unwrap().created,
        Some(Creation {
            at: stamp.at(),
            config: Some("cfg-exec".into()),
        })
    );
}

#[test]
fn an_approval_applied_by_the_kernel_records_its_cause() {
    let k = kernel(MemoryStore::default(), 0);
    k.create(Resource::new("run/1", "validator/ledger", json!({})).task(100))
        .unwrap();
    let pending = k
        .submit(&agent(), 0, write(&k, "run/1", json!({"ask": true})))
        .unwrap();
    assert_eq!(pending.effect, Effect::Pending);
    let id = k.store().list("pending/run/1/").unwrap().remove(0);
    let decision = Submission::new(&id, 0, Action::Write).content(json!({"decision": "approve"}));
    assert!(k.submit(&owner(), 0, decision).unwrap().is_accepted());
    let decided = k.history(&id).unwrap();
    let applied = k.history("run/1").unwrap().pop().unwrap();
    assert_eq!(
        applied.submission.cause,
        Some(Cause {
            resource: id,
            seq: decided[0].stamp.seq()
        })
    );
}

#[test]
fn records_and_resources_written_before_these_facts_read_as_not_recorded() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("run%2Fold");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(
        dir.join("resource.json"),
        json!({"id": "run/old", "version": 0, "validator": "validator/ledger", "state": {}})
            .to_string(),
    )
    .unwrap();
    let old = json!({
        "stamp": {"principal": {"id": "host", "kind": "code"}, "cost": 1, "seq": 1, "knobs": "default"},
        "submission": {"target": "run/old", "seen": 0, "action": "write"},
        "verdict": Verdict::returned(Reason::ContentInvalid)
    });
    std::fs::write(dir.join("history.jsonl"), format!("{old}\n")).unwrap();

    let store = FileStore::new(root.path()).unwrap();
    let record = store.history("run/old").unwrap().remove(0);
    assert_eq!(record.stamp.at(), None);
    assert_eq!(record.stamp.config(), None);
    assert_eq!(record.submission.cause, None);
    assert_eq!(store.load("run/old").unwrap().unwrap().created, None);
}

/// One scripted sequence of proposals; returns every verdict and the final
/// resource without its creation stamp.
fn script(k: &Kernel<MemoryStore>, config: Option<&str>) -> (Vec<Verdict>, Resource) {
    let as_configured = |p: Principal| match config {
        Some(c) => p.with_config(c),
        None => p,
    };
    k.create(Resource::new("run/1", "validator/ledger", json!({})).task(3))
        .unwrap();
    let mut verdicts = Vec::new();
    for n in 0..3 {
        verdicts.push(
            k.submit(
                &as_configured(agent()),
                1,
                write(k, "run/1", json!({"n": n})).cause("x", n),
            )
            .unwrap(),
        );
    }
    verdicts.push(
        k.submit(
            &as_configured(agent()),
            0,
            Submission::new("run/1", 0, Action::Write),
        )
        .unwrap(),
    );
    // The budget of 3 is crossed here: the kernel settles the task.
    verdicts.push(
        k.submit(
            &as_configured(agent()),
            1,
            write(k, "run/1", json!({"n": 9})),
        )
        .unwrap(),
    );
    let mut last = k.load("run/1").unwrap().unwrap();
    last.created = None;
    (verdicts, last)
}

#[test]
fn verdicts_do_not_depend_on_time_cause_or_configuration() {
    let early = script(&kernel(MemoryStore::default(), 0), None);
    let late = script(&kernel(MemoryStore::default(), u64::MAX / 2), Some("cfg-z"));
    assert_eq!(early, late);
    assert_eq!(early.0.last().unwrap().reason, Reason::BudgetExhausted);
}

// ---------------------------------------------------------------------------
// HistoryReader
// ---------------------------------------------------------------------------

fn tree(root: &std::path::Path) -> Vec<(String, u64)> {
    let mut entries = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let meta = entry.metadata().unwrap();
            if meta.is_dir() {
                stack.push(entry.path());
            }
            entries.push((
                entry
                    .path()
                    .strip_prefix(root)
                    .unwrap()
                    .display()
                    .to_string(),
                if meta.is_dir() { 0 } else { meta.len() },
            ));
        }
    }
    entries.sort();
    entries
}

#[test]
fn the_reader_matches_the_store_and_creates_or_changes_nothing() {
    let root = tempfile::tempdir().unwrap();
    let k = kernel(FileStore::new(root.path()).unwrap(), 0);
    for id in ["run/a", "run/b"] {
        k.create(Resource::new(id, "validator/ledger", json!({})).task(10))
            .unwrap();
        k.submit(&agent(), 1, write(&k, id, json!({"n": 1})))
            .unwrap();
    }
    // A writer is part-way through appending a record to run/a.
    let history = root.path().join("run%2Fa").join("history.jsonl");
    let mut bytes = std::fs::read(&history).unwrap();
    bytes.extend_from_slice(br#"{"stamp":{"principal""#);
    std::fs::write(&history, &bytes).unwrap();
    let before = tree(root.path());

    let reader = HistoryReader::open(root.path()).unwrap();
    assert_eq!(reader.list("run/").unwrap(), vec!["run/a", "run/b"]);
    let records = reader.history("run/a").unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].stamp.seq(), 1);
    assert_eq!(reader.load("run/b").unwrap(), k.load("run/b").unwrap());
    assert_eq!(
        reader.history("run/b").unwrap(),
        k.history("run/b").unwrap()
    );
    assert_eq!(reader.load("run/missing").unwrap(), None);
    assert!(reader.history("run/missing").unwrap().is_empty());
    assert_eq!(tree(root.path()), before);

    // A resource removed by its owner reads as absent.
    drop(k);
    FileStore::new(root.path())
        .unwrap()
        .remove("run/b")
        .unwrap();
    assert_eq!(reader.load("run/b").unwrap(), None);
    assert!(reader.history("run/b").unwrap().is_empty());
    assert_eq!(reader.list("run/").unwrap(), vec!["run/a"]);
}

#[test]
fn the_reader_never_creates_its_root() {
    let root = tempfile::tempdir().unwrap();
    let missing = root.path().join("absent");
    assert!(HistoryReader::open(&missing).is_err());
    assert!(!missing.exists());
}
