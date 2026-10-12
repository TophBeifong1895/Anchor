use anchor_library::{DevelopmentError, Library, reclaim};
use std::{
    fs,
    path::{Path, PathBuf},
};

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn stored(root: &Path, version: &str) -> PathBuf {
    let path = root.join("dependencies/store/idna").join(version);
    write(&path.join("idna/__init__.py"), "# package\n");
    path
}

fn development_view(scope: &anchor_library::DevelopmentScope, store: &Path) -> PathBuf {
    scope.register().unwrap();
    let view = scope.root().join("projects/demo/solve/view");
    write(&view.join("pyvenv.cfg"), "home = /usr/bin\n");
    fs::create_dir_all(view.join("bin")).unwrap();
    fs::create_dir_all(view.join("lib/python3.12/site-packages")).unwrap();
    std::os::unix::fs::symlink("/usr/bin/python3", view.join("bin/python")).unwrap();
    std::os::unix::fs::symlink(
        store.join("idna"),
        view.join("lib/python3.12/site-packages/idna"),
    )
    .unwrap();
    scope.record_view(&view).unwrap();
    view
}

fn scope_fact(root: &Path, scope: &anchor_library::DevelopmentScope, suffix: &str) -> PathBuf {
    root.join("dependencies/.development-leases").join(format!(
        "{}.{suffix}",
        scope.root().file_name().unwrap().to_str().unwrap()
    ))
}

#[test]
fn active_scope_blocks_cleanup_and_persistent_references_survive_restart() {
    let root = tempfile::tempdir().unwrap();
    let library = Library::new(root.path());
    let identity = root.path().join("pilot/owner-session-incarnation");
    let scope = library.development_scope(&identity, true).unwrap().unwrap();
    let package = stored(root.path(), "3.6");
    let garbage = stored(root.path(), "3.7");
    let view = development_view(&scope, &package);
    assert!(matches!(
        library.release_development_scope(&identity),
        Err(DevelopmentError::Busy)
    ));
    let report = reclaim(root.path(), true).unwrap();
    assert!(report.deferred.is_none(), "{report:?}");
    assert!(view.is_dir() && package.is_dir());
    assert!(!garbage.exists());
    drop(scope);
    // Reopening does not inspect source-directory existence or need a running
    // process. The durable scope itself is the reference fact.
    let restarted = Library::new(root.path());
    let report = reclaim(root.path(), true).unwrap();
    assert_eq!(report.kept_referenced, 1);
    assert!(package.is_dir());
    restarted.release_development_scope(&identity).unwrap();
    assert!(!view.exists());
    assert!(
        restarted.development_scope(&identity, true).is_err(),
        "released Session cannot be resurrected by a stale tool call"
    );
    restarted.release_development_scope(&identity).unwrap();
    let report = reclaim(root.path(), true).unwrap();
    assert!(report.deferred.is_none(), "{report:?}");
    assert!(
        !package.exists(),
        "released dependency-only Library can reclaim its last package"
    );
}

#[test]
fn releasing_a_session_keeps_formal_plugin_bytes_and_reclaims_only_its_private_dependencies() {
    let root = tempfile::tempdir().unwrap();
    let library = Library::new(root.path());
    let identity = root.path().join("pilot/session");
    let scope = library.development_scope(&identity, true).unwrap().unwrap();
    let package = stored(root.path(), "3.6");
    development_view(&scope, &package);
    write(
        &root.path().join("plugins/demo/plugin.json"),
        r#"{"name":"demo","dependencies":{"python":["idna==3.6"]}}"#,
    );
    let declaration = anchor_graph_host::FilePluginCatalog::new(root.path())
        .definition("demo")
        .unwrap()
        .dependencies
        .declaration_digest();
    let formal = anchor_graph_host::plugin_view_path(root.path(), "demo", &declaration);
    write(&formal.join("pyvenv.cfg"), "home = /usr/bin\n");
    fs::create_dir_all(formal.join("lib/python3.12/site-packages")).unwrap();
    std::os::unix::fs::symlink(
        package.join("idna"),
        formal.join("lib/python3.12/site-packages/idna"),
    )
    .unwrap();
    let private = stored(root.path(), "3.7");
    let second = scope.root().join("projects/other/solve/view");
    write(&second.join("pyvenv.cfg"), "home = /usr/bin\n");
    fs::create_dir_all(second.join("bin")).unwrap();
    std::os::unix::fs::symlink("/usr/bin/python3", second.join("bin/python")).unwrap();
    fs::create_dir_all(second.join("lib/python3.12/site-packages")).unwrap();
    std::os::unix::fs::symlink(
        private.join("idna"),
        second.join("lib/python3.12/site-packages/idna"),
    )
    .unwrap();
    scope.record_view(&second).unwrap();
    drop(scope);
    library.release_development_scope(&identity).unwrap();
    let report = reclaim(root.path(), true).unwrap();
    assert!(report.deferred.is_none(), "{report:?}");
    assert!(package.is_dir() && formal.is_dir());
    assert!(!private.exists());
}

#[test]
fn missing_or_corrupt_scope_or_view_reference_refuses_all_reclamation() {
    for failure in [
        "binding",
        "reference",
        "missing_view",
        "unknown_view",
        "outside_store",
        "missing_package",
        "scope_symlink",
        "missing_scope",
        "missing_development",
        "missing_ledger",
        "corrupt_ledger",
        "missing_scope_and_ledger",
        "missing_lease",
        "corrupt_lease",
    ] {
        let root = tempfile::tempdir().unwrap();
        let library = Library::new(root.path());
        let identity = root.path().join("pilot/session");
        let scope = library.development_scope(&identity, true).unwrap().unwrap();
        let package = stored(root.path(), "3.6");
        let garbage = stored(root.path(), "3.7");
        let view = development_view(&scope, &package);
        match failure {
            "binding" => fs::write(scope.root().join("binding.json"), "broken").unwrap(),
            "reference" => {
                let path = fs::read_dir(scope.root().join("references"))
                    .unwrap()
                    .next()
                    .unwrap()
                    .unwrap()
                    .path();
                fs::write(path, "broken").unwrap();
            }
            "missing_view" => fs::remove_dir_all(view).unwrap(),
            "unknown_view" => write(
                &scope.root().join("projects/unknown/view/pyvenv.cfg"),
                "home = /usr/bin\n",
            ),
            "outside_store" => {
                let link = view.join("lib/python3.12/site-packages/idna");
                fs::remove_file(&link).unwrap();
                std::os::unix::fs::symlink(root.path(), link).unwrap();
            }
            "missing_package" => fs::remove_dir_all(package.join("idna")).unwrap(),
            "scope_symlink" => {
                let bound = scope.root().to_owned();
                let moved = root.path().join("moved-scope");
                fs::rename(&bound, &moved).unwrap();
                std::os::unix::fs::symlink(moved, bound).unwrap();
            }
            "missing_scope" => fs::remove_dir_all(scope.root()).unwrap(),
            "missing_development" => fs::remove_dir_all(scope.root().parent().unwrap()).unwrap(),
            "missing_ledger" => {
                fs::remove_file(scope_fact(root.path(), &scope, "scope.json")).unwrap()
            }
            "corrupt_ledger" => {
                fs::write(scope_fact(root.path(), &scope, "scope.json"), "broken").unwrap()
            }
            "missing_scope_and_ledger" => {
                fs::remove_dir_all(scope.root()).unwrap();
                fs::remove_file(scope_fact(root.path(), &scope, "scope.json")).unwrap();
            }
            "missing_lease" => fs::remove_file(scope_fact(root.path(), &scope, "lock")).unwrap(),
            "corrupt_lease" => {
                fs::write(scope_fact(root.path(), &scope, "lock"), "broken").unwrap()
            }
            _ => unreachable!(),
        }
        drop(scope);
        let report = reclaim(root.path(), true).unwrap();
        assert!(
            report
                .deferred
                .as_ref()
                .is_some_and(|reason| reason.contains("development")),
            "{failure}: {report:?}"
        );
        assert!(
            package.is_dir() && garbage.is_dir(),
            "{failure}: uncertain references preserve every package"
        );
    }
}

#[test]
fn missing_live_scope_survives_restart_until_the_host_explicitly_releases_it() {
    let root = tempfile::tempdir().unwrap();
    let library = Library::new(root.path());
    let identity = root.path().join("pilot/session");
    let scope = library.development_scope(&identity, true).unwrap().unwrap();
    let package = stored(root.path(), "3.6");
    development_view(&scope, &package);
    fs::remove_dir_all(scope.root()).unwrap();
    drop(scope);
    let restarted = Library::new(root.path());
    assert!(restarted.development_scope(&identity, false).is_err());
    assert!(restarted.development_scope(&identity, true).is_err());
    assert!(reclaim(root.path(), true).unwrap().deferred.is_some());
    assert!(package.is_dir());
    restarted.release_development_scope(&identity).unwrap();
    assert!(restarted.development_scope(&identity, true).is_err());
    assert!(reclaim(root.path(), true).unwrap().deferred.is_none());
    assert!(!package.exists());
}

#[test]
fn a_removed_live_lease_is_never_recreated_while_an_old_command_holds_its_inode() {
    let root = tempfile::tempdir().unwrap();
    let library = Library::new(root.path());
    let identity = root.path().join("pilot/session");
    let scope = library.development_scope(&identity, true).unwrap().unwrap();
    let package = stored(root.path(), "3.6");
    let view = development_view(&scope, &package);
    let lease = scope_fact(root.path(), &scope, "lock");
    fs::remove_file(&lease).unwrap();
    assert!(library.release_development_scope(&identity).is_err());
    assert!(library.development_scope(&identity, true).is_err());
    assert!(library.development_scope(&identity, false).is_err());
    assert!(!lease.exists());
    assert!(reclaim(root.path(), true).unwrap().deferred.is_some());
    assert!(package.is_dir() && view.is_dir());
    drop(scope);
    assert!(
        Library::new(root.path())
            .release_development_scope(&identity)
            .is_err()
    );
    assert!(!lease.exists());
}

#[test]
fn missing_or_corrupt_ownership_does_not_authorize_cleanup_of_existing_views() {
    for failure in ["missing_ledger", "corrupt_ledger", "missing_registry"] {
        let root = tempfile::tempdir().unwrap();
        let library = Library::new(root.path());
        let identity = root.path().join("pilot/session");
        let scope = library.development_scope(&identity, true).unwrap().unwrap();
        let package = stored(root.path(), "3.6");
        let view = development_view(&scope, &package);
        let ledger = scope_fact(root.path(), &scope, "scope.json");
        match failure {
            "missing_ledger" => fs::remove_file(&ledger).unwrap(),
            "corrupt_ledger" => fs::write(&ledger, "broken").unwrap(),
            "missing_registry" => fs::remove_dir_all(ledger.parent().unwrap()).unwrap(),
            _ => unreachable!(),
        }
        drop(scope);
        assert!(
            library.release_development_scope(&identity).is_err(),
            "{failure}"
        );
        assert!(
            library.development_scope(&identity, false).is_err(),
            "{failure}"
        );
        assert!(
            library.development_scope(&identity, true).is_err(),
            "{failure}"
        );
        assert!(reclaim(root.path(), true).unwrap().deferred.is_some());
        assert!(package.is_dir() && view.is_dir(), "{failure}");
    }
}

#[test]
fn cancelling_before_registration_does_not_trap_session_cleanup() {
    let root = tempfile::tempdir().unwrap();
    let library = Library::new(root.path());
    let identity = root.path().join("pilot/session");
    let scope = library.development_scope(&identity, true).unwrap().unwrap();
    assert!(matches!(
        library.release_development_scope(&identity),
        Err(DevelopmentError::Busy)
    ));
    // Simulate cancellation while the first preparation waits for the writer:
    // the shared lease exists, but no package or view was published.
    drop(scope);
    let restarted = Library::new(root.path());
    restarted.release_development_scope(&identity).unwrap();
    restarted.release_development_scope(&identity).unwrap();
    assert!(restarted.development_scope(&identity, true).is_err());
    assert!(restarted.development_scope(&identity, false).is_err());
    assert!(reclaim(root.path(), true).unwrap().deferred.is_none());
}

#[test]
fn read_only_selection_does_not_create_an_unknown_scope_lease() {
    let root = tempfile::tempdir().unwrap();
    let library = Library::new(root.path());
    let known = root.path().join("pilot/known");
    let unknown = root.path().join("pilot/unknown");
    assert!(
        library
            .development_scope(&unknown, false)
            .unwrap()
            .is_none()
    );
    assert!(!root.path().join("dependencies").exists());
    let scope = library.development_scope(&known, true).unwrap().unwrap();
    scope.register().unwrap();
    let directory = root.path().join("dependencies/.development-leases");
    let before = fs::read_dir(&directory).unwrap().count();
    assert!(
        library
            .development_scope(&unknown, false)
            .unwrap()
            .is_none()
    );
    assert_eq!(fs::read_dir(directory).unwrap().count(), before);
}

#[test]
fn cleanup_never_waits_for_the_dependency_writer_or_reverses_its_lock_order() {
    let root = tempfile::tempdir().unwrap();
    let library = Library::new(root.path());
    let identity = root.path().join("pilot/session");
    let scope = library.development_scope(&identity, true).unwrap().unwrap();
    let package = stored(root.path(), "3.6");
    let view = development_view(&scope, &package);
    drop(scope);
    let writer = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(root.path().join("dependencies/.lock"))
        .unwrap();
    rustix::fs::flock(
        &writer,
        rustix::fs::FlockOperation::NonBlockingLockExclusive,
    )
    .unwrap();
    assert!(matches!(
        library.release_development_scope(&identity),
        Err(DevelopmentError::Busy)
    ));
    // Failed cleanup drops its exclusive scope lease immediately, so a command
    // can still select the previous view while the dependency writer is busy.
    let selected = library
        .development_scope(&identity, false)
        .unwrap()
        .unwrap();
    assert_eq!(
        selected.root(),
        view.parent()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
    );
    assert!(reclaim(root.path(), true).unwrap().deferred.is_some());
    assert!(package.is_dir() && view.is_dir());
    drop(writer);
    drop(selected);
    library.release_development_scope(&identity).unwrap();
}

#[test]
fn separate_session_incarnations_have_independent_bindings_and_cleanup() {
    let root = tempfile::tempdir().unwrap();
    let library = Library::new(root.path());
    let first = root.path().join("pilot/owner-session-created-one");
    let second = root.path().join("pilot/owner-session-created-two");
    let scope_a = library.development_scope(&first, true).unwrap().unwrap();
    let scope_b = library.development_scope(&second, true).unwrap().unwrap();
    let package = stored(root.path(), "3.6");
    let view_a = development_view(&scope_a, &package);
    let view_b = development_view(&scope_b, &package);
    assert_ne!(view_a, view_b);
    drop(scope_a);
    library.release_development_scope(&first).unwrap();
    assert!(!view_a.exists() && view_b.is_dir());
    assert_eq!(reclaim(root.path(), true).unwrap().kept_referenced, 1);
    drop(scope_b);
    library.release_development_scope(&second).unwrap();
}
