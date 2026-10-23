mod support;

use anchor_platform_session::{ChannelInboundRequest, SessionError, SessionStore};
use std::{
    sync::{Arc, Barrier},
    thread,
};

fn request(account: Option<&str>, inbound: &str) -> ChannelInboundRequest {
    ChannelInboundRequest {
        replace_running: true,
        ..support::channel_request(
            inbound,
            "fixture-channel",
            account,
            "conversation",
            "member",
        )
    }
}

fn register(store: &SessionStore) {
    store
        .register_channel_account(
            "owner",
            "fixture-channel",
            "canonical-bot",
            &["legacy-bot".into()],
        )
        .unwrap();
}

#[test]
fn aliases_reuse_legacy_history_and_deduplicate_the_same_event_after_restart() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("sessions.sqlite");
    let store = SessionStore::open(&path).unwrap();
    let old = store
        .admit_channel_inbound("owner", request(Some("legacy-bot"), "first"))
        .unwrap();
    let history = store
        .get_channel_inbound("owner", &old.session.id, "first")
        .unwrap();
    register(&store);
    drop(store);
    let store = SessionStore::open(&path).unwrap();
    let retry = store
        .admit_channel_inbound("owner", request(Some("canonical-bot"), "first"))
        .unwrap();
    assert_eq!(old, retry);
    assert_eq!(
        history,
        store
            .get_channel_inbound("owner", &old.session.id, "first")
            .unwrap()
    );
    let next = store
        .admit_channel_inbound("owner", request(Some("canonical-bot"), "second"))
        .unwrap();
    assert_eq!(old.session.id, next.session.id);
    assert!(
        store
            .channel_session_is_selected("owner", &old.session.id)
            .unwrap()
    );
    let mut changed = request(Some("legacy-bot"), "second");
    changed.text = Some("different input".into());
    assert!(matches!(
        store.admit_channel_inbound("owner", changed),
        Err(SessionError::Conflict(_))
    ));
}

#[test]
fn preexisting_duplicate_sessions_keep_history_but_select_only_the_canonical_session() {
    let root = tempfile::tempdir().unwrap();
    let store = SessionStore::open(root.path().join("sessions.sqlite")).unwrap();
    let old = store
        .admit_channel_inbound("owner", request(Some("legacy-bot"), "old-event"))
        .unwrap();
    let new = store
        .admit_channel_inbound("owner", request(Some("canonical-bot"), "new-event"))
        .unwrap();
    store
        .bind_channel_assistant(
            "owner",
            &old.session.id,
            "old-run",
            "wait",
            "assistant",
            "reply",
        )
        .unwrap();
    store
        .bind_channel_assistant(
            "owner",
            &new.session.id,
            "new-run",
            "wait",
            "assistant",
            "reply",
        )
        .unwrap();
    let old_history = store.get("owner", &old.session.id).unwrap();
    register(&store);
    assert!(
        !store
            .channel_session_is_selected("owner", &old.session.id)
            .unwrap()
    );
    assert!(
        store
            .channel_session_is_selected("owner", &new.session.id)
            .unwrap()
    );
    let admitted = store
        .admit_channel_inbound("owner", request(Some("legacy-bot"), "next"))
        .unwrap();
    assert_eq!(new.session.id, admitted.session.id);
    assert_eq!(old_history, store.get("owner", &old.session.id).unwrap());
    assert_eq!(store.list_channel_assistants().unwrap().len(), 2);
    assert!(matches!(
        store.handover_channel_assistant(
            "owner",
            &old.session.id,
            "old-run",
            "forbidden-new-run",
            "wait-key"
        ),
        Err(SessionError::Conflict(_))
    ));
    let before = store.list_turns("owner", &new.session.id).unwrap();
    assert!(matches!(
        store.admit_channel_inbound("owner", request(Some("canonical-bot"), "old-event")),
        Err(SessionError::Conflict(_))
    ));
    assert_eq!(before, store.list_turns("owner", &new.session.id).unwrap());
}

#[test]
fn competing_legacy_sessions_fail_closed_instead_of_choosing_a_history() {
    let root = tempfile::tempdir().unwrap();
    let store = SessionStore::open(root.path().join("sessions.sqlite")).unwrap();
    let first = store
        .admit_channel_inbound("owner", request(Some("legacy-bot"), "first"))
        .unwrap();
    store
        .admit_channel_inbound("owner", request(Some("another-legacy"), "second"))
        .unwrap();
    store
        .register_channel_account(
            "owner",
            "fixture-channel",
            "canonical-bot",
            &["legacy-bot".into(), "another-legacy".into()],
        )
        .unwrap();
    assert!(matches!(
        store.channel_session_is_selected("owner", &first.session.id),
        Err(SessionError::Conflict(_))
    ));
    assert!(matches!(
        store.admit_channel_inbound("owner", request(Some("canonical-bot"), "next")),
        Err(SessionError::Conflict(_))
    ));
    assert_eq!(store.list("owner").unwrap().len(), 2);
}

#[test]
fn absent_account_and_different_accounts_remain_isolated() {
    let root = tempfile::tempdir().unwrap();
    let store = SessionStore::open(root.path().join("sessions.sqlite")).unwrap();
    register(&store);
    let first = store
        .admit_channel_inbound("owner", request(None, "first"))
        .unwrap();
    let second = store
        .admit_channel_inbound("owner", request(Some("canonical-bot"), "first"))
        .unwrap();
    let third = store
        .admit_channel_inbound("owner", request(Some("another-bot"), "first"))
        .unwrap();
    assert_ne!(first.session.id, second.session.id);
    assert_ne!(third.session.id, second.session.id);
}

#[test]
fn concurrent_alias_admission_creates_one_session_and_one_turn() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("sessions.sqlite");
    let store = SessionStore::open(&path).unwrap();
    register(&store);
    let barrier = Arc::new(Barrier::new(2));
    let handles = ["legacy-bot", "canonical-bot"].map(|account| {
        let sessions = SessionStore::open(&path).unwrap();
        let barrier = barrier.clone();
        thread::spawn(move || {
            barrier.wait();
            sessions
                .admit_channel_inbound("owner", request(Some(account), "same-event"))
                .unwrap()
        })
    });
    let [first, second] = handles.map(|handle| handle.join().unwrap());
    assert_eq!(first.session.id, second.session.id);
    assert_eq!(first.turn.id, second.turn.id);
    assert_eq!(store.list("owner").unwrap().len(), 1);
}
