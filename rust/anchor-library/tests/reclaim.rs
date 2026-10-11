//! Reclamation: a package version is deleted only when no living view points at it.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anchor_library::Library;
use serde_json::{Value, json};

fn write(path: impl AsRef<Path>, content: &str) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

/// The manifest every fixture Plugin uses: one declared package, one skill.
fn manifest(id: &str, idna: &str) -> String {
    json!({
        "name": id,
        "description": "reclaim fixture",
        "skills": "skills/",
        "dependencies": {"python": [format!("idna=={idna}")]}
    })
    .to_string()
}

/// A complete candidate tree under `<root>/source-<id>`.
fn candidate(root: &Path, id: &str, idna: &str) -> PathBuf {
    let source = root.join(format!("source-{id}"));
    write(source.join("plugin.json"), &manifest(id, idna));
    write(source.join("skills/check/SKILL.md"), "# fixture\n");
    source
}

/// The declaration digest of a candidate, without installing it.
///
/// The digest covers the declaration and nothing else, so a probe root laid out
/// the way the catalog expects answers it directly.
fn declaration_of_candidate(root: &Path, id: &str, idna: &str) -> String {
    let probe = root.join(format!("probe-{id}"));
    let directory = probe.join("plugins").join(id);
    write(directory.join("plugin.json"), &manifest(id, idna));
    write(directory.join("skills/check/SKILL.md"), "# fixture\n");
    let digest = declaration_of(&probe, id);
    fs::remove_dir_all(&probe).unwrap();
    digest
}

/// Put an installed Plugin tree in place without going through the installer.
///
/// The installer reclaims on its own way out (see the trigger test), so these
/// cases build the state they want to reason about directly. The declaration is
/// still parsed by the catalog, so the digest is the real one.
fn plugin_tree(root: &Path, id: &str, idna: &str) -> String {
    let directory = root.join("plugins").join(id);
    write(directory.join("plugin.json"), &manifest(id, idna));
    write(directory.join("skills/check/SKILL.md"), "# fixture\n");
    declaration_of(root, id)
}

/// A view the way the host builds one: a `pyvenv.cfg`, an interpreter link and a
/// link into the store for the one package the declaration resolved to.
fn view(root: &Path, id: &str, declaration: &str, package: &str, version: &str) -> PathBuf {
    let directory = root.join("dependencies/views").join(id).join(declaration);
    let site = directory.join("lib/python3.12/site-packages");
    fs::create_dir_all(&site).unwrap();
    fs::create_dir_all(directory.join("bin")).unwrap();
    write(directory.join("pyvenv.cfg"), "home = /usr/bin\n");
    std::os::unix::fs::symlink("/bin/sh", directory.join("bin/python")).unwrap();
    let stored = root
        .join("dependencies/store")
        .join(package)
        .join(version)
        .join(package);
    fs::create_dir_all(&stored).unwrap();
    write(stored.join("__init__.py"), "# package\n");
    std::os::unix::fs::symlink(&stored, site.join(package)).unwrap();
    directory
}

/// A store entry nothing references, as a failed installation leaves behind.
fn store_entry(root: &Path, package: &str, version: &str) -> PathBuf {
    let directory = root.join("dependencies/store").join(package).join(version);
    write(directory.join(package).join("__init__.py"), "# package\n");
    write(directory.join("RECORD"), "package/__init__.py,,\n");
    directory
}

fn declaration_of(root: &Path, id: &str) -> String {
    anchor_graph_host::FilePluginCatalog::new(root)
        .definition(id)
        .unwrap()
        .dependencies
        .declaration_digest()
}

#[test]
fn a_shared_package_version_survives_one_view_becoming_an_orphan() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    // Two Plugins that resolve to the same package version.
    let first = plugin_tree(root, "shared-one", "3.6");
    let second = plugin_tree(root, "shared-two", "3.6");
    let shared = view(root, "shared-one", &first, "idna", "3.6");
    view(root, "shared-two", &second, "idna", "3.6");
    // `shared-one` upgrades to 3.7. Its 3.6 view is now an orphan, but
    // `shared-two` still points at the 3.6 store entry.
    let upgraded = plugin_tree(root, "shared-one", "3.7");
    view(root, "shared-one", &upgraded, "idna", "3.7");

    let report = anchor_library::reclaim(root, true).unwrap();
    assert_eq!(
        report.removed_views,
        vec![format!("dependencies/views/shared-one/{first}")],
        "{report:?}"
    );
    assert!(
        report.unreferenced_package_versions.is_empty(),
        "3.6 is still referenced by shared-two: {report:?}"
    );
    assert!(root.join("dependencies/store/idna/3.6").is_dir());
    assert!(root.join("dependencies/store/idna/3.7").is_dir());
    assert!(!shared.exists());
    assert_eq!(report.kept_referenced, 2, "{report:?}");
}

#[test]
fn a_package_version_only_an_orphan_view_referenced_is_removed_whole() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let first = plugin_tree(root, "alone", "3.6");
    let old = view(root, "alone", &first, "idna", "3.6");
    let exclusive = store_entry(root, "exclusive", "1.0");
    let upgraded = plugin_tree(root, "alone", "3.7");
    view(root, "alone", &upgraded, "idna", "3.7");

    // A dry run must not touch anything.
    let dry = anchor_library::reclaim(root, false).unwrap();
    assert!(!dry.applied, "a dry run reports without applying: {dry:?}");
    assert_eq!(
        dry.unreferenced_package_versions,
        vec!["exclusive/1.0".to_owned(), "idna/3.6".to_owned()],
        "{dry:?}"
    );
    assert!(exclusive.is_dir() && old.is_dir());
    assert_eq!(dry.reclaimed_bytes, 0, "a dry run frees nothing");
    assert!(
        dry.reclaimable_bytes > 0,
        "a dry run still says how much it would free: {dry:?}"
    );

    let report = anchor_library::reclaim(root, true).unwrap();
    assert_eq!(
        report.removed_package_versions,
        vec!["exclusive/1.0".to_owned(), "idna/3.6".to_owned()],
        "{report:?}"
    );
    assert!(!exclusive.exists(), "the whole version directory is gone");
    assert!(!old.exists());
    assert!(
        report.reclaimed_bytes > 0,
        "the report counts what it freed: {report:?}"
    );
    assert!(root.join("dependencies/store/idna/3.7").is_dir());
}

#[test]
fn an_unreadable_manifest_keeps_everything() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let live = plugin_tree(root, "keeper", "3.6");
    view(root, "keeper", &live, "idna", "3.6");
    let orphan = plugin_tree(root, "broken", "3.6");
    view(root, "broken", &orphan, "uuid", "1.0");
    let garbage = store_entry(root, "unused", "9.9");
    // The second Plugin's manifest becomes unreadable: we can no longer say which
    // views it maps to, so nothing is deleted at all.
    write(root.join("plugins/broken/plugin.json"), "{ not json");

    let report = anchor_library::reclaim(root, true).unwrap();
    assert!(!report.applied, "{report:?}");
    assert!(report.deferred.is_some(), "{report:?}");
    assert!(
        report
            .kept_reasons
            .iter()
            .any(|reason| reason.contains("broken")),
        "{report:?}"
    );
    assert!(report.orphan_views.is_empty(), "{report:?}");
    assert!(report.removed_views.is_empty() && report.removed_package_versions.is_empty());
    assert!(garbage.is_dir(), "even an unreferenced entry is kept");
    assert!(root.join("dependencies/store/uuid/1.0").is_dir());
    assert!(
        root.join(format!("dependencies/views/broken/{orphan}"))
            .is_dir()
    );
}

#[test]
fn a_writer_holding_the_install_lock_defers_reclamation() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let live = plugin_tree(root, "demo", "3.6");
    view(root, "demo", &live, "idna", "3.6");
    let garbage = store_entry(root, "unused", "9.9");

    // Another writer holds the Plugin installation lock, the way the installer
    // takes it.
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join("plugins/.install.lock"))
        .unwrap();
    rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive).unwrap();
    let report = anchor_library::reclaim(root, true).unwrap();
    assert!(!report.applied, "{report:?}");
    assert!(
        report
            .deferred
            .as_deref()
            .unwrap()
            .contains("installation is in progress"),
        "{report:?}"
    );
    assert!(report.removed_package_versions.is_empty(), "{report:?}");
    assert!(garbage.is_dir(), "a busy Library is left alone");

    rustix::fs::flock(&lock, rustix::fs::FlockOperation::Unlock).unwrap();
    drop(lock);
    let report = anchor_library::reclaim(root, true).unwrap();
    assert_eq!(
        report.removed_package_versions,
        vec!["unused/9.9".to_owned()],
        "{report:?}"
    );
    assert!(!garbage.exists());
}

/// An upgrade through the Library cleans up after itself.
///
/// The host lands the new declaration's view *before* it publishes the Plugin (a
/// failed dependency landing must leave nothing), so by the time the install
/// returns, the previous view is the only orphan and its exclusive packages are
/// the only unreferenced entries.
#[test]
fn installing_a_new_declaration_reclaims_the_previous_view_automatically() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let first = plugin_tree(root, "alone", "3.6");
    view(root, "alone", &first, "idna", "3.6");
    // The new declaration's view lands first, as the host does it.
    let upgraded = declaration_after_install(root, "alone", "3.7");

    assert!(
        !root
            .join(format!("dependencies/views/alone/{first}"))
            .exists(),
        "the previous declaration's view is gone without a manual command"
    );
    assert!(
        !root.join("dependencies/store/idna/3.6").exists(),
        "and so are the packages only it referenced"
    );
    assert!(root.join("dependencies/store/idna/3.7").is_dir());
    assert!(
        root.join(format!("dependencies/views/alone/{upgraded}"))
            .is_dir()
    );
}

/// Install `idna==3.7` while the view it maps to already exists, and return the
/// declaration digest the Library published.
fn declaration_after_install(root: &Path, id: &str, idna: &str) -> String {
    let source = candidate(root, id, idna);
    let pending = declaration_of_candidate(root, id, idna);
    view(root, id, &pending, "idna", idna);
    Library::new(root)
        .install_directory(id, &source, true)
        .unwrap();
    declaration_of(root, id)
}

/// A view whose Plugin is not installed was kept by the first pass, so its packages
/// must survive the second one: a view that stays on disk must not be left dangling.
#[test]
fn a_kept_unknown_id_view_still_references_its_packages() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    plugin_tree(root, "installed", "3.7");
    view(
        root,
        "installed",
        &declaration_of(root, "installed"),
        "idna",
        "3.7",
    );
    // A view for an id no Plugin carries — a failed installation's leftover.
    view(root, "ghost", "abc", "idna", "3.6");
    let garbage = store_entry(root, "unused", "9.9");

    let report = anchor_library::reclaim(root, true).unwrap();
    assert_eq!(
        report.removed_package_versions,
        vec!["unused/9.9".to_owned()],
        "{report:?}"
    );
    assert!(
        root.join("dependencies/store/idna/3.6").is_dir(),
        "the kept view still points at 3.6: {report:?}"
    );
    assert!(root.join("dependencies/views/ghost/abc").is_dir());
    assert!(!garbage.exists());
}

/// Nothing is installed while the store still holds bytes: every entry would look
/// unreferenced, and clearing a whole shared store on that evidence is the one
/// deletion that cannot be undone from here.
#[test]
fn an_empty_plugins_directory_never_clears_the_store() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    fs::create_dir_all(root.join("plugins")).unwrap();
    let first = store_entry(root, "aaa", "1.0");
    let second = store_entry(root, "bbb", "2.0");
    view(root, "ghost", "abc", "aaa", "1.0");

    let report = anchor_library::reclaim(root, true).unwrap();
    assert!(!report.applied, "{report:?}");
    assert!(report.deferred.is_some(), "{report:?}");
    assert!(report.removed_package_versions.is_empty(), "{report:?}");
    assert!(report.removed_views.is_empty(), "{report:?}");
    assert!(first.is_dir() && second.is_dir());
    assert!(root.join("dependencies/views/ghost/abc").is_dir());
}

/// A host holds the installation lease across "views landed, Plugin not published
/// yet". A reclamation running in that window must delete nothing — and once the
/// Plugin is published under the same lease, the view is live anyway.
#[test]
fn a_host_holding_the_install_lease_defers_reclamation() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let source = candidate(root, "pending", "3.6");
    let declaration = declaration_of_candidate(root, "pending", "3.6");

    let library = Library::new(root);
    let lease = library.install_lease().unwrap();
    // The dependency landing happened; the Plugin is not published yet.
    view(root, "pending", &declaration, "idna", "3.6");
    let report = anchor_library::reclaim(root, true).unwrap();
    assert!(!report.applied, "{report:?}");
    assert!(
        report
            .deferred
            .as_deref()
            .unwrap()
            .contains("installation is in progress"),
        "{report:?}"
    );
    assert!(report.removed_views.is_empty() && report.removed_package_versions.is_empty());
    assert!(root.join("dependencies/store/idna/3.6").is_dir());

    // Publishing under the lease makes that view the live one.
    library
        .install_directory_locked("pending", &source, false, &lease)
        .unwrap();
    drop(lease);
    let report = anchor_library::reclaim(root, true).unwrap();
    assert!(report.orphan_views.is_empty(), "{report:?}");
    assert!(report.removed_views.is_empty(), "{report:?}");
    assert!(root.join("dependencies/store/idna/3.6").is_dir());
}

/// A Library that never landed dependencies is not an error, and a report leaves no
/// new files behind.
#[test]
fn an_empty_library_reports_nothing_and_a_dry_run_creates_no_files() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    plugin_tree(root, "solo", "3.6");

    let report = anchor_library::reclaim(root, true).unwrap();
    assert!(report.deferred.is_none(), "{report:?}");
    assert!(report.removed_views.is_empty() && report.removed_package_versions.is_empty());
    assert_eq!(report.reclaimed_bytes, 0);
    assert!(!root.join("dependencies").exists(), "nothing was created");

    // Now there is a store to sweep: a dry run reports it without writing.
    let orphan = view(root, "solo", "old", "idna", "3.5");
    let dry = anchor_library::reclaim(root, false).unwrap();
    assert_eq!(
        dry.unreferenced_package_versions,
        vec!["idna/3.5".to_owned()]
    );
    assert!(
        !orphan.join("../../.lock").exists(),
        "no store lock created"
    );
    assert!(!root.join("dependencies/.lock").exists());
    assert!(!root.join("plugins/.install.lock").exists());
    assert!(orphan.is_dir());
}

#[test]
fn reclaiming_twice_removes_nothing_the_second_time() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let first = plugin_tree(root, "alone", "3.6");
    view(root, "alone", &first, "idna", "3.6");
    store_entry(root, "unused", "9.9");
    let upgraded = plugin_tree(root, "alone", "3.7");
    view(root, "alone", &upgraded, "idna", "3.7");

    let once = anchor_library::reclaim(root, true).unwrap();
    assert!(!once.removed_package_versions.is_empty(), "{once:?}");
    let twice = anchor_library::reclaim(root, true).unwrap();
    assert!(twice.removed_views.is_empty(), "{twice:?}");
    assert!(twice.removed_package_versions.is_empty(), "{twice:?}");
    assert!(twice.orphan_views.is_empty(), "{twice:?}");
    assert_eq!(twice.reclaimed_bytes, 0);
}

#[test]
fn the_operator_command_reports_by_default_and_applies_with_a_flag() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let first = plugin_tree(root, "alone", "3.6");
    let orphan = view(root, "alone", &first, "idna", "3.6");
    let upgraded = plugin_tree(root, "alone", "3.7");
    view(root, "alone", &upgraded, "idna", "3.7");

    let report: Value = serde_json::from_slice(
        &Command::new(env!("CARGO_BIN_EXE_anchor-library"))
            .arg("--root")
            .arg(root)
            .arg("reclaim")
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(report["applied"], json!(false), "{report}");
    // The dry run lists what it *would* delete, and says what it keeps.
    assert_eq!(
        report["unreferenced_package_versions"],
        json!(["idna/3.6"]),
        "{report}"
    );
    assert_eq!(
        report["orphan_views"].as_array().unwrap().len(),
        1,
        "{report}"
    );
    assert_eq!(report["removed_package_versions"], json!([]), "{report}");
    assert_eq!(report["removed_views"], json!([]), "{report}");
    assert_eq!(report["kept_referenced"], json!(1), "{report}");
    assert_eq!(report["reclaimed_bytes"], json!(0), "{report}");
    assert!(
        report["reclaimable_bytes"].as_u64().unwrap() > 0,
        "the operator sees how much the deletion would free: {report}"
    );
    assert!(orphan.is_dir(), "a dry run changes nothing on disk");

    let applied: Value = serde_json::from_slice(
        &Command::new(env!("CARGO_BIN_EXE_anchor-library"))
            .arg("--root")
            .arg(root)
            .arg("reclaim")
            .arg("--apply")
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(applied["applied"], json!(true), "{applied}");
    assert!(
        applied["removed_package_versions"]
            .as_array()
            .unwrap()
            .contains(&json!("idna/3.6")),
        "{applied}"
    );
    assert!(
        applied["reclaimed_bytes"].as_u64().unwrap() > 0,
        "the operator sees how much was freed: {applied}"
    );
    assert!(!orphan.exists());
    assert!(!root.join("dependencies/store/idna/3.6").exists());
}
