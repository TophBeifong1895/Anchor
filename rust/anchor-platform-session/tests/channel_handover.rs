use anchor_platform_session::{
    ChannelAdmission, ChannelIdentity, ChannelInboundRequest, SessionError, SessionStore,
    TurnStatus,
};
use rusqlite::Connection;
use std::{
    sync::{Arc, Barrier},
    thread,
};
use tempfile::TempDir;

fn fixture() -> (TempDir, SessionStore, ChannelAdmission) {
    let directory = tempfile::tempdir().unwrap();
    let store = SessionStore::open(directory.path().join("sessions.sqlite")).unwrap();
    let first = store
        .admit_channel_inbound(
            "owner",
            ChannelInboundRequest {
                inbound_id: "message-1".into(),
                identity: ChannelIdentity {
                    conversation_kind: Default::default(),
                    source: "test-channel".into(),
                    account: Some("account".into()),
                    conversation_id: "conversation".into(),
                    sender_id: "sender".into(),
                },
                graph: "fixture".into(),
                reply_node: "assistant".into(),
                text: Some("input".into()),
                attachments: Default::default(),
                run_id: None,
                replace_running: false,
            },
        )
        .unwrap();
    bind(&store, &first.session.id, "run-a");
    (directory, store, first)
}

fn bind(store: &SessionStore, session: &str, run: &str) {
    store
        .bind_channel_assistant("owner", session, run, "wait", "assistant", "reply")
        .unwrap();
}

fn finish(store: &SessionStore, first: &ChannelAdmission) {
    store
        .finish_turn(
            "owner",
            &first.session.id,
            &first.turn.id,
            TurnStatus::Stopped,
            None,
        )
        .unwrap();
}

fn assert_conflict<T: std::fmt::Debug>(result: Result<T, SessionError>) {
    assert!(
        matches!(result, Err(SessionError::Conflict(_))),
        "{result:?}"
    );
}

#[test]
fn explicit_retirement_cannot_handover_to_a_new_run_even_after_reopen() {
    let (directory, store, first) = fixture();
    let session = &first.session.id;
    finish(&store, &first);
    store
        .retire_channel_assistant("owner", session, "run-a")
        .unwrap();
    let snapshot = store.get("owner", session).unwrap();
    let events = store.events("owner", session, 0).unwrap();
    drop(store);

    let reopened = SessionStore::open(directory.path().join("sessions.sqlite")).unwrap();
    assert_conflict(
        reopened.handover_channel_assistant("owner", session, "run-a", "run-b", "wait/1"),
    );
    assert_eq!(
        reopened.get_channel_assistant("owner", session).unwrap(),
        None
    );
    assert_eq!(reopened.get("owner", session).unwrap(), snapshot);
    assert_eq!(reopened.events("owner", session, 0).unwrap(), events);
    assert!(reopened.list_channel_assistants().unwrap().is_empty());
    let connection = Connection::open(directory.path().join("sessions.sqlite")).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM channel_assistants", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        1
    );
}

#[test]
fn independently_bound_target_does_not_prove_a_handover() {
    let (_directory, store, first) = fixture();
    let session = &first.session.id;
    finish(&store, &first);
    store
        .retire_channel_assistant("owner", session, "run-a")
        .unwrap();
    bind(&store, session, "run-b");
    let current = store.get_channel_assistant("owner", session).unwrap();
    let snapshot = store.get("owner", session).unwrap();
    let events = store.events("owner", session, 0).unwrap();

    assert_conflict(store.handover_channel_assistant("owner", session, "run-a", "run-b", "wait/1"));
    assert_conflict(store.handover_channel_assistant("owner", session, "run-a", "run-c", "wait/1"));
    assert_eq!(
        store.get_channel_assistant("owner", session).unwrap(),
        current
    );
    assert_eq!(store.get("owner", session).unwrap(), snapshot);
    assert_eq!(store.events("owner", session, 0).unwrap(), events);
}

#[test]
fn only_the_exact_committed_handover_can_retry_against_the_current_target() {
    let (_directory, store, first) = fixture();
    let session = &first.session.id;
    store
        .handover_channel_assistant("owner", session, "run-a", "run-b", "wait/b")
        .unwrap();
    let current = store
        .handover_channel_assistant("owner", session, "run-b", "run-c", "wait/c")
        .unwrap();
    let events = store.events("owner", session, 0).unwrap();

    assert_conflict(store.handover_channel_assistant("owner", session, "run-a", "run-c", "wait/c"));
    assert_conflict(store.handover_channel_assistant("owner", session, "run-a", "run-b", "wait/b"));
    assert_eq!(
        store
            .handover_channel_assistant("owner", session, "run-b", "run-c", "wait/c")
            .unwrap(),
        current
    );
    assert_eq!(store.events("owner", session, 0).unwrap(), events);
    assert_eq!(store.list_channel_assistants().unwrap().len(), 1);
}

#[test]
fn a_current_binding_is_not_a_committed_self_handover() {
    let (_directory, store, first) = fixture();
    let session = &first.session.id;
    let current = store.get_channel_assistant("owner", session).unwrap();
    let events = store.events("owner", session, 0).unwrap();
    assert_conflict(store.handover_channel_assistant("owner", session, "run-a", "run-a", "wait/1"));
    assert_eq!(
        store.get_channel_assistant("owner", session).unwrap(),
        current
    );
    assert_eq!(store.events("owner", session, 0).unwrap(), events);
}

#[test]
fn committed_handover_retry_after_reopen_preserves_the_in_flight_turn_and_events() {
    let (directory, store, first) = fixture();
    let session = &first.session.id;
    let claimed = store
        .claim_channel_assistant_input("owner", session, "run-a", "wait/a")
        .unwrap()
        .unwrap();
    let current = store
        .handover_channel_assistant("owner", session, "run-a", "run-b", "wait/b")
        .unwrap();
    let committed_turn = store.get_turn("owner", session, &first.turn.id).unwrap();
    assert_eq!(committed_turn.id, claimed.turn.id);
    assert_eq!(committed_turn.status, TurnStatus::Running);
    assert_eq!(committed_turn.runs, ["run-a", "run-b"]);
    let snapshot = store.get("owner", session).unwrap();
    let events = store.events("owner", session, 0).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == "channel.assistant_handover")
            .count(),
        1
    );
    drop(store);

    let reopened = SessionStore::open(directory.path().join("sessions.sqlite")).unwrap();
    for _ in 0..2 {
        assert_eq!(
            reopened
                .handover_channel_assistant("owner", session, "run-a", "run-b", "wait/b")
                .unwrap(),
            current
        );
    }
    assert_eq!(reopened.get("owner", session).unwrap(), snapshot);
    assert_eq!(reopened.events("owner", session, 0).unwrap(), events);
    let carried = reopened
        .claim_channel_assistant_input("owner", session, "run-b", "wait/b")
        .unwrap()
        .unwrap();
    assert_eq!(carried.turn, committed_turn);
    assert_eq!(carried.request, claimed.request);
    assert_eq!(carried.relation.turn_id, claimed.relation.turn_id);
    assert_eq!(carried.relation.inbound_id, claimed.relation.inbound_id);
    assert_eq!(carried.relation.run_id.as_deref(), Some("run-b"));
    assert_eq!(
        reopened
            .get_channel_relation("owner", session, "message-1")
            .unwrap(),
        carried.relation
    );
    assert_conflict(reopened.claim_channel_assistant_input("owner", session, "run-a", "wait/a"));
    assert_eq!(
        reopened
            .channel_assistant_wait_key("owner", session, &first.turn.id)
            .unwrap()
            .as_deref(),
        Some("wait/b")
    );
}

#[test]
fn committed_handover_cannot_be_replayed_after_its_target_is_explicitly_retired() {
    let (_directory, store, first) = fixture();
    let session = &first.session.id;
    finish(&store, &first);
    store
        .handover_channel_assistant("owner", session, "run-a", "run-b", "wait/b")
        .unwrap();
    store
        .retire_channel_assistant("owner", session, "run-b")
        .unwrap();
    let events = store.events("owner", session, 0).unwrap();

    assert_conflict(store.handover_channel_assistant("owner", session, "run-a", "run-b", "wait/b"));
    assert_conflict(store.handover_channel_assistant("owner", session, "run-b", "run-c", "wait/c"));
    assert_eq!(store.get_channel_assistant("owner", session).unwrap(), None);
    assert_eq!(store.events("owner", session, 0).unwrap(), events);
}

#[test]
fn handover_event_failure_rolls_back_bindings_input_and_run_associations() {
    let (directory, store, first) = fixture();
    let session = &first.session.id;
    let claimed = store
        .claim_channel_assistant_input("owner", session, "run-a", "wait/a")
        .unwrap()
        .unwrap();
    let snapshot = store.get("owner", session).unwrap();
    let events = store.events("owner", session, 0).unwrap();
    let connection = Connection::open(directory.path().join("sessions.sqlite")).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER fail_handover BEFORE INSERT ON session_events
             WHEN NEW.kind = 'channel.assistant_handover'
             BEGIN SELECT RAISE(ABORT, 'handover event failure'); END;",
        )
        .unwrap();
    assert!(
        store
            .handover_channel_assistant("owner", session, "run-a", "run-b", "wait/b")
            .is_err()
    );
    assert_eq!(store.get("owner", session).unwrap(), snapshot);
    assert_eq!(store.events("owner", session, 0).unwrap(), events);
    assert_eq!(
        store
            .get_channel_assistant("owner", session)
            .unwrap()
            .unwrap()
            .run_id,
        "run-a"
    );
    assert_eq!(
        store
            .get_channel_assistant_input("owner", session, "run-a", "wait/a")
            .unwrap()
            .unwrap(),
        claimed
    );
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM channel_assistants", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        1
    );
    connection
        .execute_batch("DROP TRIGGER fail_handover")
        .unwrap();
    store
        .handover_channel_assistant("owner", session, "run-a", "run-b", "wait/b")
        .unwrap();
    let carried = store
        .claim_channel_assistant_input("owner", session, "run-b", "wait/b")
        .unwrap()
        .unwrap();
    assert_eq!(carried.turn.id, first.turn.id);
    assert_eq!(carried.turn.runs, ["run-a", "run-b"]);
}

#[test]
fn independent_connections_commit_the_same_handover_only_once() {
    let (directory, store, first) = fixture();
    let session = &first.session.id;
    store
        .claim_channel_assistant_input("owner", session, "run-a", "wait/a")
        .unwrap()
        .unwrap();
    let contenders = (0..4)
        .map(|_| SessionStore::open(directory.path().join("sessions.sqlite")).unwrap())
        .collect::<Vec<_>>();
    let barrier = Arc::new(Barrier::new(contenders.len()));
    let workers = contenders
        .into_iter()
        .map(|contender| {
            let barrier = barrier.clone();
            let session = session.clone();
            thread::spawn(move || {
                barrier.wait();
                contender
                    .handover_channel_assistant("owner", &session, "run-a", "run-b", "wait/b")
                    .unwrap()
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        assert_eq!(worker.join().unwrap().run_id, "run-b");
    }
    let events = store.events("owner", session, 0).unwrap();
    let handovers = events
        .iter()
        .filter(|event| event.kind == "channel.assistant_handover")
        .collect::<Vec<_>>();
    assert_eq!(handovers.len(), 1);
    assert_eq!(handovers[0].data["from"], "run-a");
    assert_eq!(handovers[0].data["to"], "run-b");
    assert_eq!(store.list_channel_assistants().unwrap().len(), 1);
    let carried = store
        .claim_channel_assistant_input("owner", session, "run-b", "wait/b")
        .unwrap()
        .unwrap();
    assert_eq!(carried.turn.id, first.turn.id);
    assert_eq!(carried.turn.status, TurnStatus::Running);
    assert_eq!(carried.turn.runs, ["run-a", "run-b"]);
    assert_eq!(carried.relation.run_id.as_deref(), Some("run-b"));
}
