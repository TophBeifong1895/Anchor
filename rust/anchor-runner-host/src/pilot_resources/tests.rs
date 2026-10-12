use super::*;
use anchor_platform_session::{CreateSession, TurnStatus};

fn fixture() -> (tempfile::TempDir, SessionStore, Session, PathBuf, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("state");
    let work = root.path().join("work");
    std::fs::create_dir_all(data.join("platform")).unwrap();
    let sessions = SessionStore::open(data.join("platform/sessions.sqlite")).unwrap();
    let session = sessions
        .create(
            "owner",
            CreateSession {
                id: Some("pilot".into()),
                ..Default::default()
            },
        )
        .unwrap();
    (root, sessions, session, data, work)
}

#[test]
fn cleanup_obeys_native_lease_and_removes_only_owned_resources() {
    let (_root, sessions, session, data, work) = fixture();
    register(&data, &work, "owner", &session).unwrap();
    let resources = binding(&data, &work, "owner", &session);
    std::fs::create_dir_all(&resources.workspace).unwrap();
    std::fs::write(resources.workspace.join("candidate.txt"), "candidate").unwrap();
    let held = crate::goose_acp::pilot::execution_lease(&resources.goose).unwrap();
    std::fs::create_dir_all(resources.goose.join("candidates")).unwrap();
    std::fs::write(resources.goose.join("candidates/prepared"), "private").unwrap();
    let other = work.join("pilot/other");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("keep"), "another Session").unwrap();
    assert!(
        matches!(delete(&sessions,&data,&work,"owner",&session),Err(SessionError::Conflict(reason)) if reason.contains("executing"))
    );
    assert!(sessions.get("owner", "pilot").is_ok());
    assert!(resources.workspace.join("candidate.txt").is_file());
    drop(held);
    delete(&sessions, &data, &work, "owner", &session).unwrap();
    assert!(matches!(
        sessions.get("owner", "pilot"),
        Err(SessionError::Missing)
    ));
    assert!(!resources.workspace.exists());
    assert!(!resources.goose.exists());
    assert!(other.join("keep").is_file());
}

#[test]
fn partial_cleanup_keeps_session_and_binding_for_retry() {
    let (_root, sessions, session, data, work) = fixture();
    register(&data, &work, "owner", &session).unwrap();
    let resources = binding(&data, &work, "owner", &session);
    std::fs::create_dir_all(&resources.workspace).unwrap();
    std::fs::create_dir_all(resources.goose.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&resources.workspace, &resources.goose).unwrap();
    assert!(delete(&sessions, &data, &work, "owner", &session).is_err());
    assert!(sessions.get("owner", "pilot").is_ok());
    assert!(path(&data, &resources).is_file());
    std::fs::remove_file(&resources.goose).unwrap();
    std::fs::create_dir_all(&resources.goose).unwrap();
    delete(&sessions, &data, &work, "owner", &session).unwrap();
    assert!(!resources.workspace.exists());
    assert!(!resources.goose.exists());
}

#[test]
fn legacy_resources_and_running_turns_are_preserved() {
    let (_root, sessions, session, data, work) = fixture();
    let resources = binding(&data, &work, "owner", &session);
    std::fs::create_dir_all(&resources.workspace).unwrap();
    assert!(
        matches!(delete(&sessions,&data,&work,"owner",&session),Err(SessionError::Conflict(reason)) if reason.contains("legacy"))
    );
    register(&data, &work, "owner", &session).unwrap();
    let (turn, _) = sessions
        .create_turn("owner", "pilot", "request", Some("develop"))
        .unwrap();
    assert!(matches!(
        delete(&sessions, &data, &work, "owner", &session),
        Err(SessionError::Conflict(_))
    ));
    assert!(resources.workspace.exists());
    sessions
        .finish_turn("owner", "pilot", &turn.id, TurnStatus::Stopped, None)
        .unwrap();
    delete(&sessions, &data, &work, "owner", &session).unwrap();
}

#[test]
fn legacy_resources_with_a_matching_goose_turn_are_adopted() {
    let (_root, sessions, session, data, work) = fixture();
    let resources = binding(&data, &work, "owner", &session);
    std::fs::create_dir_all(&resources.workspace).unwrap();
    std::fs::create_dir_all(&resources.goose).unwrap();
    let scope = resources
        .goose
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    std::fs::write(
        resources.goose.join("goose.json"),
        serde_json::json!({"session_id":"native-session"}).to_string(),
    )
    .unwrap();
    let (turn, _) = sessions
        .create_turn("owner", "pilot", "request", Some("develop"))
        .unwrap();
    sessions
        .bind_goose(
            "owner",
            "pilot",
            &turn.id,
            anchor_platform_session::GooseExecution {
                scope,
                session: "native-session".into(),
            },
        )
        .unwrap();
    sessions
        .finish_turn("owner", "pilot", &turn.id, TurnStatus::Stopped, None)
        .unwrap();

    delete(&sessions, &data, &work, "owner", &session).unwrap();
    assert!(!resources.workspace.exists());
    assert!(!resources.goose.exists());
    assert!(!path(&data, &resources).exists());
    assert!(matches!(
        sessions.get("owner", "pilot"),
        Err(SessionError::Missing)
    ));
}

fn bound_session(sessions: &SessionStore, data: &Path, work: &Path, id: &str) -> Session {
    let session = sessions
        .create(
            "owner",
            CreateSession {
                id: Some(id.into()),
                ..Default::default()
            },
        )
        .unwrap();
    register(data, work, "owner", &session).unwrap();
    session
}

fn completed_snapshot(
    sessions: &SessionStore,
    data: &Path,
    session: &Session,
    request: &str,
) -> PathBuf {
    let (turn, _) = sessions
        .create_turn("owner", &session.id, request, Some("develop"))
        .unwrap();
    sessions
        .finish_turn("owner", &session.id, &turn.id, TurnStatus::Completed, None)
        .unwrap();
    let snapshot = crate::plugin_mounts::MountSnapshot::new(
        &session.id,
        &turn.id,
        &std::collections::BTreeMap::new(),
    );
    let path = crate::plugin_mounts::snapshot_path(data, &session.id, &turn.id);
    crate::plugin_mounts::write_snapshot(&path, &snapshot).unwrap();
    path
}

fn assert_colliding_session_cleanup(deleted_id: &str) {
    let (_root, sessions, _session, data, work) = fixture();
    let colon = bound_session(&sessions, &data, &work, "a:b");
    let underscore = bound_session(&sessions, &data, &work, "a_b");
    let (deleted, survivor) = if deleted_id == colon.id {
        (&colon, &underscore)
    } else {
        (&underscore, &colon)
    };
    let owned = completed_snapshot(&sessions, &data, deleted, "owned");
    let surviving = completed_snapshot(&sessions, &data, survivor, "surviving");
    let surviving_bytes = std::fs::read(&surviving).unwrap();
    assert_eq!(owned.parent(), surviving.parent());

    let unknown = owned.parent().unwrap().join("unknown.json");
    let unknown_bytes = br#"{"unrecognized":"keep"}"#;
    std::fs::write(&unknown, unknown_bytes).unwrap();
    let unrecorded = crate::plugin_mounts::snapshot_path(&data, deleted_id, "unrecorded");
    crate::plugin_mounts::write_snapshot(
        &unrecorded,
        &crate::plugin_mounts::MountSnapshot::new(
            deleted_id,
            "unrecorded",
            &std::collections::BTreeMap::new(),
        ),
    )
    .unwrap();
    let unrecorded_bytes = std::fs::read(&unrecorded).unwrap();
    let malformed = completed_snapshot(&sessions, &data, deleted, "malformed");
    std::fs::write(&malformed, "{ broken").unwrap();
    let mismatched = completed_snapshot(&sessions, &data, deleted, "mismatched");
    std::fs::write(&mismatched, &surviving_bytes).unwrap();
    let linked = completed_snapshot(&sessions, &data, deleted, "linked");
    let outside = data.join("outside-snapshot.json");
    std::fs::rename(&linked, &outside).unwrap();
    let outside_bytes = std::fs::read(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, &linked).unwrap();

    delete(&sessions, &data, &work, "owner", deleted).unwrap();
    assert!(!owned.exists(), "snapshot leaked for {deleted_id}");
    assert!(matches!(
        sessions.get("owner", deleted_id),
        Err(SessionError::Missing)
    ));
    assert!(sessions.get("owner", &survivor.id).is_ok());
    assert_eq!(std::fs::read(&surviving).unwrap(), surviving_bytes);
    assert_eq!(std::fs::read(&unknown).unwrap(), unknown_bytes);
    assert_eq!(std::fs::read(&unrecorded).unwrap(), unrecorded_bytes);
    assert_eq!(std::fs::read(&malformed).unwrap(), b"{ broken");
    assert_eq!(std::fs::read(&mismatched).unwrap(), surviving_bytes);
    assert!(std::fs::symlink_metadata(&linked).unwrap().is_symlink());
    assert_eq!(std::fs::read(&outside).unwrap(), outside_bytes);
}

#[test]
fn encoded_session_cleanup_preserves_colliding_sessions_and_unknown_snapshots() {
    assert_colliding_session_cleanup("a:b");
}

#[test]
fn plain_session_cleanup_preserves_colliding_sessions_and_unknown_snapshots() {
    assert_colliding_session_cleanup("a_b");
}

#[test]
fn unicode_session_snapshots_use_the_existing_encoded_path_during_cleanup() {
    let (_root, sessions, _session, data, work) = fixture();
    let session = bound_session(&sessions, &data, &work, "开发@团队");
    let snapshot = completed_snapshot(&sessions, &data, &session, "develop");
    assert_eq!(snapshot.parent().unwrap().file_name().unwrap(), "_____");
    let mut legacy: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&snapshot).unwrap()).unwrap();
    legacy["format"] = 1.into();
    legacy.as_object_mut().unwrap().remove("network_allowed");
    std::fs::write(&snapshot, serde_json::to_vec(&legacy).unwrap()).unwrap();
    delete(&sessions, &data, &work, "owner", &session).unwrap();
    assert!(!snapshot.exists());
    assert!(!snapshot.parent().unwrap().exists());
    assert!(matches!(
        sessions.get("owner", &session.id),
        Err(SessionError::Missing)
    ));
}

#[test]
fn snapshot_directory_symlinks_preserve_identity_and_can_be_retried() {
    let (_root, sessions, _session, data, work) = fixture();
    let session = bound_session(&sessions, &data, &work, "a:b");
    let snapshot = completed_snapshot(&sessions, &data, &session, "develop");
    let resources = binding(&data, &work, "owner", &session);
    std::fs::create_dir_all(&resources.workspace).unwrap();
    let directory = snapshot.parent().unwrap();
    let outside = data.join("outside-snapshots");
    std::fs::rename(directory, &outside).unwrap();
    let outside_snapshot = outside.join(snapshot.file_name().unwrap());
    let bytes = std::fs::read(&outside_snapshot).unwrap();
    std::os::unix::fs::symlink(&outside, directory).unwrap();

    assert!(matches!(
        delete(&sessions, &data, &work, "owner", &session),
        Err(SessionError::Storage(_))
    ));
    assert!(sessions.get("owner", &session.id).is_ok());
    assert!(path(&data, &resources).is_file());
    assert!(!resources.workspace.exists());
    assert_eq!(std::fs::read(&outside_snapshot).unwrap(), bytes);
    assert!(std::fs::symlink_metadata(directory).unwrap().is_symlink());

    std::fs::remove_file(directory).unwrap();
    std::fs::rename(outside, directory).unwrap();
    delete(&sessions, &data, &work, "owner", &session).unwrap();
    assert!(!snapshot.exists());
    assert!(!directory.exists());
    assert!(matches!(
        sessions.get("owner", &session.id),
        Err(SessionError::Missing)
    ));
}
