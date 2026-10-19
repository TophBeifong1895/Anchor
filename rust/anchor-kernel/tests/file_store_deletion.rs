//! Physical deletion is atomic at directory rename and recoverable by reads.
use anchor_kernel::{FileStore, Reason, Record, Resource, Store, StoreError, Verdict};
use serde_json::json;
use std::{
    fs,
    sync::{Arc, Barrier},
    thread,
};

const ID: &str = "run/delete";
const ENCODED: &str = "run%2Fdelete";
fn resource() -> Resource {
    Resource::new(ID, "validator/test", json!({"retained":true}))
}
fn history() -> Record {
    serde_json::from_value(json!({
        "stamp":{"principal":{"id":"host","kind":"code"},"cost":1,"seq":1,"knobs":"deletion-test"},
        "submission":{"target":ID,"seen":0,"action":"write"},
        "verdict":Verdict::returned(Reason::ContentInvalid),"after":null
    }))
    .unwrap()
}

#[test]
fn remove_deletes_resource_history_and_directory_but_retains_locks_and_other_resources() {
    let root = tempfile::tempdir().unwrap();
    let store = FileStore::new(root.path()).unwrap();
    store.create(&resource()).unwrap();
    store.commit(ID, &history()).unwrap();
    let other = Resource::new("run/other", "validator/test", json!({}));
    store.create(&other).unwrap();
    assert!(root.path().join(ENCODED).join("resource.json").is_file());
    assert!(root.path().join(ENCODED).join("history.jsonl").is_file());
    store.remove(ID).unwrap();
    assert_eq!(store.load(ID).unwrap(), None);
    assert_eq!(store.list("").unwrap(), vec!["run/other"]);
    assert!(store.history(ID).unwrap().is_empty());
    assert!(!root.path().join(ENCODED).exists());
    assert!(!root.path().join(format!(".deleting-{ENCODED}")).exists());
    assert_eq!(fs::read_dir(root.path().join(".locks")).unwrap().count(), 2);
    assert_eq!(store.load(&other.id).unwrap(), Some(other));
}

#[test]
fn remove_is_idempotent_and_rejects_invalid_ids_before_creating_any_files() {
    let root = tempfile::tempdir().unwrap();
    let store = FileStore::new(root.path()).unwrap();
    for id in [String::new(), "a".repeat(513)] {
        assert!(matches!(store.remove(&id),Err(StoreError::InvalidId(actual)) if actual==id));
    }
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    store.remove(ID).unwrap();
    store.remove(ID).unwrap();
    store.create(&resource()).unwrap();
    store.remove(ID).unwrap();
    store.remove(ID).unwrap();
    assert_eq!(store.load(ID).unwrap(), None);
    assert!(store.list("").unwrap().is_empty());
}

#[test]
fn load_finishes_interrupted_deletion_with_only_a_tombstone() {
    let root = tempfile::tempdir().unwrap();
    let store = FileStore::new(root.path()).unwrap();
    store.create(&resource()).unwrap();
    store.commit(ID, &history()).unwrap();
    let tombstone = root.path().join(format!(".deleting-{ENCODED}"));
    fs::rename(root.path().join(ENCODED), &tombstone).unwrap();
    drop(store);
    let restarted = FileStore::new(root.path()).unwrap();
    assert!(
        restarted.list("").unwrap().is_empty(),
        "tombstone is never a resource"
    );
    assert_eq!(restarted.load(ID).unwrap(), None);
    assert!(!tombstone.exists());
    assert_eq!(restarted.load(ID).unwrap(), None);
    restarted.remove(ID).unwrap();
    // An incompletely removed directory is recoverable, too.
    fs::create_dir(&tombstone).unwrap();
    fs::write(tombstone.join("history.jsonl"), b"partly removed").unwrap();
    assert_eq!(restarted.load(ID).unwrap(), None);
    assert!(!tombstone.exists());
}

#[test]
fn load_cleans_old_tombstone_and_preserves_existing_live_resource() {
    let root = tempfile::tempdir().unwrap();
    let store = FileStore::new(root.path()).unwrap();
    store.create(&resource()).unwrap();
    store.commit(ID, &history()).unwrap();
    let tombstone = root.path().join(format!(".deleting-{ENCODED}"));
    fs::create_dir(&tombstone).unwrap();
    fs::write(tombstone.join("resource.json"), b"old resource").unwrap();
    fs::write(tombstone.join("history.jsonl"), b"old history").unwrap();
    let restarted = FileStore::new(root.path()).unwrap();
    assert_eq!(restarted.load(ID).unwrap(), Some(resource()));
    assert_eq!(restarted.history(ID).unwrap(), vec![history()]);
    assert_eq!(restarted.list("").unwrap(), vec![ID]);
    assert!(!tombstone.exists());
    restarted.remove(ID).unwrap();
    assert_eq!(restarted.load(ID).unwrap(), None);
}

#[test]
fn internal_names_never_hide_valid_encoded_resource_ids() {
    let root = tempfile::tempdir().unwrap();
    let store = FileStore::new(root.path()).unwrap();
    for id in [".locks", ".deleting-run/delete", "../run/delete", "locks"] {
        let resource = Resource::new(id, "validator/test", json!({}));
        store.create(&resource).unwrap();
        assert_eq!(store.load(id).unwrap(), Some(resource));
        assert!(store.list("").unwrap().contains(&id.to_string()));
        store.remove(id).unwrap();
        assert_eq!(store.load(id).unwrap(), None);
    }
    assert!(store.list("").unwrap().is_empty());
}

#[test]
fn deletion_probes_preserve_reads_of_long_names_and_missing_ids_remain_idempotent() {
    let root = tempfile::tempdir().unwrap();
    let store = FileStore::new(root.path()).unwrap();
    // The live name fits NAME_MAX; adding the deletion prefix does not.
    let id = "a".repeat(250);
    let resource = Resource::new(&id, "validator/test", json!({}));
    store.create(&resource).unwrap();
    assert_eq!(store.load(&id).unwrap(), Some(resource));
    assert!(store.list("").unwrap().contains(&id));
    // The prescribed rename cannot fit on this filesystem. Its error must
    // preserve the live resource rather than partially deleting its files.
    assert!(matches!(store.remove(&id), Err(StoreError::Io(_))));
    assert!(store.load(&id).unwrap().is_some());
    for missing in ["b".repeat(250), "c".repeat(512), "/".repeat(512)] {
        store.remove(&missing).unwrap();
        store.remove(&missing).unwrap();
        assert_eq!(store.load(&missing).unwrap(), None);
    }
}

#[test]
fn concurrent_load_and_remove_finish_one_hundred_rounds_without_errors() {
    let root = tempfile::tempdir().unwrap();
    let reader = Arc::new(FileStore::new(root.path()).unwrap());
    // Separate instances exercise the on-disk lease as well as same-instance
    // calls (the remover alternates between the two).
    let writer = FileStore::new(root.path()).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    thread::scope(|scope| {
        let reader_for_worker = reader.clone();
        let barrier_for_worker = barrier.clone();
        let read = scope.spawn(move || {
            for _ in 0..100 {
                barrier_for_worker.wait();
                for _ in 0..64 {
                    if let Some(loaded) = reader_for_worker.load(ID).unwrap() {
                        assert_eq!(loaded, resource());
                    }
                    thread::yield_now();
                }
                barrier_for_worker.wait();
                assert_eq!(reader_for_worker.load(ID).unwrap(), None);
                barrier_for_worker.wait();
            }
        });
        let remove = scope.spawn(move || {
            for round in 0..100 {
                writer.create(&resource()).unwrap();
                writer.commit(ID, &history()).unwrap();
                barrier.wait();
                if round % 2 == 0 {
                    writer.remove(ID).unwrap();
                } else {
                    reader.remove(ID).unwrap();
                }
                barrier.wait();
                barrier.wait();
            }
        });
        read.join().unwrap();
        remove.join().unwrap();
    });
}
