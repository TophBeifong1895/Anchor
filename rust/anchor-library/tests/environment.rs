//! Published environments: identity, immutability and the name table.

use std::{
    fs,
    os::unix::{ffi::OsStrExt, fs::PermissionsExt},
    path::{Path, PathBuf},
};

use anchor_library::{EnvironmentError, EnvironmentStore};

fn write(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// A prepared tree shaped like a Python environment: a script, a package and a
/// nested directory.
fn prepared(root: &Path, label: &str) -> PathBuf {
    let directory = root.join(format!("prepared-{label}"));
    write(
        directory.join("bin/run"),
        format!("#!/bin/sh\necho {label}\n"),
    );
    fs::set_permissions(directory.join("bin/run"), fs::Permissions::from_mode(0o755)).unwrap();
    write(
        directory.join("lib/site-packages/pkg/__init__.py"),
        format!("VERSION = \"{label}\"\n"),
    );
    directory
}

/// Published trees are read-only; tests that tamper or delete must first restore
/// the write bits the way a deployment would not.
fn force_writable(root: &Path) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let mut permissions = fs::metadata(&directory).unwrap().permissions();
        permissions.set_mode(permissions.mode() | 0o700);
        fs::set_permissions(&directory, permissions).unwrap();
        for entry in fs::read_dir(&directory).unwrap() {
            let entry = entry.unwrap();
            let metadata = entry.metadata().unwrap();
            if metadata.is_dir() {
                stack.push(entry.path());
            } else {
                let mut permissions = metadata.permissions();
                permissions.set_mode(permissions.mode() | 0o600);
                fs::set_permissions(entry.path(), permissions).unwrap();
            }
        }
    }
}

fn store(root: &Path) -> EnvironmentStore {
    EnvironmentStore::new(root)
}

#[test]
fn the_same_content_publishes_one_environment_and_is_reused() {
    let temporary = tempfile::tempdir().unwrap();
    let store = store(temporary.path());
    let source = prepared(temporary.path(), "one");

    let first = store.publish(&source).unwrap();
    assert!(!first.reused);
    assert_eq!(first.id.len(), 64);
    assert_eq!(first.files, 2);

    let second = store.publish(&source).unwrap();
    assert!(second.reused);
    assert_eq!(second.id, first.id);

    let published = store.list().unwrap();
    assert_eq!(published.len(), 1, "{published:?}");
    assert_eq!(published[0].id, first.id);
    assert_eq!(published[0].files, 2);
    assert!(store.verify().unwrap().is_empty());
}

#[test]
fn content_that_differs_by_directory_or_mode_is_not_reused() {
    let temporary = tempfile::tempdir().unwrap();
    let store = store(temporary.path());
    let source = prepared(temporary.path(), "one");
    let first = store.publish(&source).unwrap();

    // An extra (empty) directory is part of the environment's content: a venv
    // carries empty site-packages, marker directories and the like.
    fs::create_dir_all(source.join("lib/site-packages/empty")).unwrap();
    let with_directory = store.publish(&source).unwrap();
    assert_ne!(
        with_directory.id, first.id,
        "an empty directory changed the tree"
    );
    assert!(!with_directory.reused);

    // Executable bits are load-bearing for bin/ scripts.
    fs::remove_dir_all(source.join("lib/site-packages/empty")).unwrap();
    fs::set_permissions(source.join("bin/run"), fs::Permissions::from_mode(0o644)).unwrap();
    let without_exec = store.publish(&source).unwrap();
    assert_ne!(
        without_exec.id, first.id,
        "a mode change is a different tree"
    );
    assert_eq!(store.list().unwrap().len(), 3);
}

#[test]
fn republishing_restores_read_only_content() {
    let temporary = tempfile::tempdir().unwrap();
    let store = store(temporary.path());
    let source = prepared(temporary.path(), "one");
    let published = store.publish(&source).unwrap();

    // Exactly the "same owner chmods it back" case: the reuse path must repair
    // the published tree instead of blessing it.
    let environment = temporary.path().join("envs").join(&published.id);
    force_writable(&environment);
    assert!(
        store
            .verify()
            .unwrap()
            .iter()
            .any(|problem| problem.contains("still writable")),
        "verify must report a writable published tree"
    );

    let reused = store.publish(&source).unwrap();
    assert!(reused.reused);
    assert_eq!(
        fs::metadata(&environment).unwrap().permissions().mode() & 0o222,
        0
    );
    assert_eq!(
        fs::metadata(environment.join("bin/run"))
            .unwrap()
            .permissions()
            .mode()
            & 0o222,
        0
    );
    assert!(store.verify().unwrap().is_empty());
}

#[test]
fn changed_content_publishes_a_new_environment() {
    let temporary = tempfile::tempdir().unwrap();
    let store = store(temporary.path());
    let source = prepared(temporary.path(), "one");
    let first = store.publish(&source).unwrap();

    write(
        source.join("lib/site-packages/pkg/__init__.py"),
        "VERSION = \"two\"\n",
    );
    let second = store.publish(&source).unwrap();
    assert_ne!(second.id, first.id);
    assert_eq!(store.list().unwrap().len(), 2);
}

#[test]
fn published_content_carries_no_write_bits_and_is_verifiable() {
    let temporary = tempfile::tempdir().unwrap();
    let store = store(temporary.path());
    let source = prepared(temporary.path(), "one");
    let published = store.publish(&source).unwrap();

    let environment = temporary.path().join("envs").join(&published.id);
    assert_eq!(
        fs::metadata(environment.join("bin/run"))
            .unwrap()
            .permissions()
            .mode()
            & 0o222,
        0,
        "published executables must not stay writable"
    );
    assert_eq!(
        fs::metadata(&environment).unwrap().permissions().mode() & 0o222,
        0,
        "published directories must not stay writable"
    );
    // The prepared source is untouched: only the published copy is protected.
    assert_ne!(
        fs::metadata(source.join("bin/run"))
            .unwrap()
            .permissions()
            .mode()
            & 0o222,
        0
    );
    assert!(store.verify().unwrap().is_empty());
    assert_eq!(store.show(&published.id).unwrap().files, 2);
}

#[test]
fn tampering_with_published_content_is_detected() {
    let temporary = tempfile::tempdir().unwrap();
    let store = store(temporary.path());
    let source = prepared(temporary.path(), "one");
    let published = store.publish(&source).unwrap();
    let environment = temporary.path().join("envs").join(&published.id);

    force_writable(&environment);
    write(
        environment.join("lib/site-packages/pkg/__init__.py"),
        "VERSION = \"tampered\"\n",
    );

    let problems = store.verify().unwrap();
    assert!(
        problems
            .iter()
            .any(|problem| problem.contains("does not match its digest")),
        "{problems:?}"
    );
    assert!(matches!(
        store.show(&published.id),
        Err(EnvironmentError::Corrupt(_))
    ));
}

#[test]
fn names_switch_with_compare_and_swap() {
    let temporary = tempfile::tempdir().unwrap();
    let store = store(temporary.path());
    let first = store.publish(prepared(temporary.path(), "one")).unwrap();
    let second = store.publish(prepared(temporary.path(), "two")).unwrap();

    let created = store.set_name("research", &first.id, None).unwrap();
    assert_eq!(created.version, 1);

    // A stale view cannot overwrite a newer switch.
    let refused = store.set_name("research", &second.id, None).unwrap_err();
    assert!(matches!(refused, EnvironmentError::VersionConflict { .. }));
    let refused = store.set_name("research", &second.id, Some(0)).unwrap_err();
    assert!(matches!(refused, EnvironmentError::VersionConflict { .. }));
    assert_eq!(store.names().unwrap()[0].id, first.id);

    // Switching from the version actually observed succeeds and advances it.
    let switched = store.set_name("research", &second.id, Some(1)).unwrap();
    assert_eq!(switched.version, 2);
    assert_eq!(store.names().unwrap()[0].id, second.id);
}

#[test]
fn names_must_point_at_published_content() {
    let temporary = tempfile::tempdir().unwrap();
    let store = store(temporary.path());
    for id in ["", "not-a-digest", &"a".repeat(64)] {
        assert!(store.set_name("research", id, None).is_err(), "{id:?}");
    }
    assert!(matches!(
        store.set_name("bad name", &"b".repeat(64), None),
        Err(EnvironmentError::InvalidName)
    ));
    assert!(store.names().unwrap().is_empty());
}

#[test]
fn removing_a_name_needs_the_observed_version_and_keeps_content() {
    let temporary = tempfile::tempdir().unwrap();
    let store = store(temporary.path());
    let published = store.publish(prepared(temporary.path(), "one")).unwrap();
    store.set_name("research", &published.id, None).unwrap();

    let refused = store.remove_name("research", Some(7)).unwrap_err();
    assert!(matches!(refused, EnvironmentError::VersionConflict { .. }));
    // No expectation means "assert absent"; it must never clobber a live name.
    let refused = store.remove_name("research", None).unwrap_err();
    assert!(matches!(refused, EnvironmentError::VersionConflict { .. }));
    assert_eq!(store.names().unwrap().len(), 1);

    let removed = store.remove_name("research", Some(1)).unwrap();
    assert_eq!(removed.id, published.id);
    assert!(store.names().unwrap().is_empty());
    assert_eq!(store.list().unwrap().len(), 1, "content is never deleted");
    assert!(matches!(
        store.remove_name("research", Some(1)),
        Err(EnvironmentError::UnknownName(_))
    ));
}

#[test]
fn unsafe_or_empty_candidates_are_rejected() {
    let temporary = tempfile::tempdir().unwrap();
    let store = store(temporary.path());

    let empty = temporary.path().join("empty");
    fs::create_dir_all(&empty).unwrap();
    assert!(matches!(
        store.publish(&empty),
        Err(EnvironmentError::EmptyContent)
    ));

    let linked = temporary.path().join("linked");
    write(linked.join("real.txt"), "real");
    std::os::unix::fs::symlink("real.txt", linked.join("alias.txt")).unwrap();
    assert!(matches!(
        store.publish(&linked),
        Err(EnvironmentError::UnsafePath)
    ));

    let escaping = temporary.path().join("escaping");
    fs::create_dir_all(&escaping).unwrap();
    std::os::unix::fs::symlink("/etc/passwd", escaping.join("passwd")).unwrap();
    assert!(matches!(
        store.publish(&escaping),
        Err(EnvironmentError::UnsafePath)
    ));

    let missing = temporary.path().join("missing");
    assert!(store.publish(&missing).is_err());
}

#[test]
fn verify_reports_names_without_content() {
    let temporary = tempfile::tempdir().unwrap();
    let store = store(temporary.path());
    let published = store.publish(prepared(temporary.path(), "one")).unwrap();
    store.set_name("research", &published.id, None).unwrap();

    // Deleting content behind the store's back is exactly what verify catches.
    let environment = temporary.path().join("envs").join(&published.id);
    force_writable(&environment);
    fs::remove_dir_all(&environment).unwrap();

    let problems = store.verify().unwrap();
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("points at missing environment"));
}

#[test]
fn show_refuses_a_published_tree_that_became_writable_again() {
    // `show` is what admission consults before running, so it must refuse a tree
    // anyone can still change — not only `verify`.
    let temporary = tempfile::tempdir().unwrap();
    let store = store(temporary.path());
    let published = store.publish(prepared(temporary.path(), "one")).unwrap();
    assert!(store.show(&published.id).is_ok());

    let environment = temporary.path().join("envs").join(&published.id);
    force_writable(&environment);

    let refused = store.show(&published.id).unwrap_err();
    assert!(refused.to_string().contains("still writable"), "{refused}");
    // verify() keeps reporting the same fact for the whole store.
    let problems = store.verify().unwrap();
    assert!(
        problems
            .iter()
            .any(|problem| problem.contains("still writable")),
        "{problems:?}"
    );
}

#[test]
fn cli_publishes_names_and_verifies() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("library");
    let source = prepared(temporary.path(), "one");
    let binary = env!("CARGO_BIN_EXE_anchor-library");

    let publish = std::process::Command::new(binary)
        .arg("--root")
        .arg(&root)
        .args(["env", "publish", "--directory"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        publish.status.success(),
        "{}",
        String::from_utf8_lossy(&publish.stderr)
    );
    let published: serde_json::Value = serde_json::from_slice(&publish.stdout).unwrap();
    let id = published["id"].as_str().unwrap().to_owned();
    assert_eq!(published["reused"], false);

    let set = std::process::Command::new(binary)
        .arg("--root")
        .arg(&root)
        .args(["env", "name", "set", "research", "--id", &id])
        .output()
        .unwrap();
    assert!(
        set.status.success(),
        "{}",
        String::from_utf8_lossy(&set.stderr)
    );

    let list = std::process::Command::new(binary)
        .arg("--root")
        .arg(&root)
        .args(["env", "name", "list"])
        .output()
        .unwrap();
    let names: serde_json::Value = serde_json::from_slice(&list.stdout).unwrap();
    assert_eq!(names[0]["name"], "research");
    assert_eq!(names[0]["id"], id.as_str());
    assert_eq!(names[0]["version"], 1);

    // The verify subcommand exits non-zero once the store is inconsistent.
    let verify = std::process::Command::new(binary)
        .arg("--root")
        .arg(&root)
        .args(["env", "verify"])
        .output()
        .unwrap();
    assert!(verify.status.success());
    let report: serde_json::Value = serde_json::from_slice(&verify.stdout).unwrap();
    assert_eq!(report["ok"], true);
    assert_eq!(report["problems"].as_array().unwrap().len(), 0);
}

#[test]
fn verify_reports_leftovers_in_the_environment_store() {
    let temporary = tempfile::tempdir().unwrap();
    let store = store(temporary.path());
    store.publish(prepared(temporary.path(), "one")).unwrap();

    // A crash between staging and publication leaves a directory behind.
    let leftover = temporary.path().join("envs/.env-staging-crashed");
    fs::create_dir_all(leftover.join("content")).unwrap();

    let problems = store.verify().unwrap();
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("unexpected entry"), "{problems:?}");
}

#[test]
fn verify_refuses_a_name_target_that_is_not_a_published_digest() {
    let temporary = tempfile::tempdir().unwrap();
    let store = store(temporary.path());
    store.publish(prepared(temporary.path(), "one")).unwrap();
    // The write side refuses this, but the table is also a file on disk: verify
    // must not certify a path that was never published.
    write(
        temporary.path().join("envs/names.json"),
        r#"{"names":{"escape":{"id":"/etc","version":3}}}"#,
    );

    assert!(matches!(
        store.verify(),
        Err(EnvironmentError::InvalidTable)
    ));
    assert!(
        matches!(store.names(), Err(EnvironmentError::InvalidTable)),
        "resolving names must refuse a table this version cannot represent"
    );
}

#[test]
fn an_unrepresentable_name_table_is_refused_instead_of_truncated() {
    let temporary = tempfile::tempdir().unwrap();
    let store = store(temporary.path());
    let published = store.publish(prepared(temporary.path(), "one")).unwrap();
    store.set_name("research", &published.id, None).unwrap();

    // A field this version does not know: rewriting the table would drop it.
    let table = temporary.path().join("envs/names.json");
    let mut parsed: serde_json::Value = serde_json::from_slice(&fs::read(&table).unwrap()).unwrap();
    parsed["future_field"] = serde_json::json!({"mode": "shadow"});
    write(&table, parsed.to_string());

    let refused = store.set_name("other", &published.id, None).unwrap_err();
    assert!(matches!(refused, EnvironmentError::InvalidTable));
    let after: serde_json::Value = serde_json::from_slice(&fs::read(&table).unwrap()).unwrap();
    assert!(
        after["names"]["research"].is_object(),
        "the existing table must survive a refused write: {after}"
    );
}

#[test]
fn content_with_distinct_non_utf8_names_gets_distinct_ids() {
    let temporary = tempfile::tempdir().unwrap();
    let store = store(temporary.path());
    let mut ids = Vec::new();
    for byte in [0xff_u8, 0xfe_u8] {
        let source = temporary.path().join(format!("source-{byte}"));
        fs::create_dir_all(&source).unwrap();
        let raw = [b'f', b'i', b'l', b'e', b'-', byte];
        fs::write(source.join(std::ffi::OsStr::from_bytes(&raw)), "same body").unwrap();
        let outcome = store.publish(&source).unwrap();
        assert!(!outcome.reused, "different raw names are different content");
        ids.push(outcome.id);
    }
    assert_ne!(ids[0], ids[1], "names must be hashed as bytes, not lossily");
    assert_eq!(store.list().unwrap().len(), 2);
    assert!(store.verify().unwrap().is_empty());
}

#[test]
fn publishing_the_store_or_its_parent_is_refused() {
    let temporary = tempfile::tempdir().unwrap();
    let store = store(temporary.path());
    for source in [
        temporary.path().to_path_buf(),
        temporary.path().join("envs"),
    ] {
        assert!(
            matches!(
                store.publish(&source),
                Err(EnvironmentError::OverlappingSource)
            ),
            "{source:?} was accepted"
        );
    }
}

#[test]
fn cli_verify_exits_non_zero_and_remove_requires_the_observed_version() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("library");
    let source = prepared(temporary.path(), "one");
    let binary = env!("CARGO_BIN_EXE_anchor-library");

    let publish = std::process::Command::new(binary)
        .arg("--root")
        .arg(&root)
        .args(["env", "publish", "--directory"])
        .arg(&source)
        .output()
        .unwrap();
    let published: serde_json::Value = serde_json::from_slice(&publish.stdout).unwrap();
    let id = published["id"].as_str().unwrap().to_owned();
    let environment = root.join("envs").join(&id);

    // Remove without an observed version is refused by the CLI itself.
    let missing_version = std::process::Command::new(binary)
        .arg("--root")
        .arg(&root)
        .args(["env", "name", "set", "research", "--id", &id])
        .output()
        .unwrap();
    assert!(missing_version.status.success());
    let without_version = std::process::Command::new(binary)
        .arg("--root")
        .arg(&root)
        .args(["env", "name", "remove", "research"])
        .output()
        .unwrap();
    assert!(!without_version.status.success());

    // An inconsistent store makes `verify` fail, not just report.
    force_writable(&environment);
    fs::remove_dir_all(environment.join("lib")).unwrap();
    let verify = std::process::Command::new(binary)
        .arg("--root")
        .arg(&root)
        .args(["env", "verify"])
        .output()
        .unwrap();
    assert!(!verify.status.success());
    let report: serde_json::Value = serde_json::from_slice(&verify.stdout).unwrap();
    assert_eq!(report["ok"], false);
    assert!(!report["problems"].as_array().unwrap().is_empty());
}

#[test]
fn concurrent_stale_switches_commit_at_most_one_winner() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("library");
    let store = store(&root);
    let first = store.publish(prepared(temporary.path(), "one")).unwrap();
    let second = store.publish(prepared(temporary.path(), "two")).unwrap();
    let binary = env!("CARGO_BIN_EXE_anchor-library");
    assert!(
        std::process::Command::new(binary)
            .arg("--root")
            .arg(&root)
            .args(["env", "name", "set", "research", "--id", &first.id])
            .output()
            .unwrap()
            .status
            .success()
    );

    // Two independent processes both believe the name is at version 1.
    let mut children = Vec::new();
    for id in [&first.id, &second.id] {
        children.push(
            std::process::Command::new(binary)
                .arg("--root")
                .arg(&root)
                .args([
                    "env",
                    "name",
                    "set",
                    "research",
                    "--id",
                    id,
                    "--expect-version",
                    "1",
                ])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap(),
        );
    }
    let mut winners = 0;
    for mut child in children {
        if child.wait().unwrap().success() {
            winners += 1;
        }
    }
    assert_eq!(winners, 1, "exactly one stale switch may commit");

    let names = store.names().unwrap();
    assert_eq!(names.len(), 1);
    assert_eq!(names[0].version, 2);
    assert!(store.verify().unwrap().is_empty());
}

#[test]
fn read_paths_work_when_the_store_is_not_writable() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let prepared = root.join("prepared");
    std::fs::create_dir_all(prepared.join("bin")).unwrap();
    std::fs::write(prepared.join("marker.txt"), "one").unwrap();
    let store = EnvironmentStore::new(root);
    let published = store.publish(&prepared).unwrap();
    store.set_name("research", &published.id, None).unwrap();

    // Simulate a hardened deployment: the store directory itself is no longer
    // writable. Even then an operator must be able to read identities and names.
    let lock = root.join("envs").join(".env.lock");
    assert!(lock.is_file(), "publication creates the lock file");
    let mut permissions = std::fs::metadata(&lock).unwrap().permissions();
    permissions.set_mode(0o400);
    std::fs::set_permissions(&lock, permissions).unwrap();
    let mut permissions = std::fs::metadata(root.join("envs")).unwrap().permissions();
    permissions.set_mode(0o500);
    std::fs::set_permissions(root.join("envs"), permissions).unwrap();

    let names = store.names().unwrap();
    assert_eq!(names.len(), 1);
    assert_eq!(names[0].name, "research");
    assert_eq!(store.show(&published.id).unwrap().id, published.id);
    assert_eq!(store.list().unwrap().len(), 1);
    assert!(store.verify().unwrap().is_empty());

    // Restore write permission so the temporary directory can be cleaned up.
    let mut permissions = std::fs::metadata(root.join("envs")).unwrap().permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(root.join("envs"), permissions).unwrap();
}
