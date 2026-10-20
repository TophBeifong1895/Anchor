use std::{
    path::Path,
    sync::{Arc, Barrier},
    thread,
};

use anchor_platform_session::{
    ChannelIdentity, ChannelInboundRequest, SessionError, SessionStore, TurnStatus,
};
use rusqlite::{Connection, types::Value};

fn aliases(path: &Path) -> Vec<(String, String, String, String)> {
    let connection = Connection::open(path).unwrap();
    let mut statement = connection.prepare(
        "SELECT owner, source, alias, canonical FROM channel_account_aliases ORDER BY owner, source, alias",
    ).unwrap();
    statement
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

fn history(path: &Path) -> Vec<(String, Vec<Vec<Value>>)> {
    let connection = Connection::open(path).unwrap();
    let mut statement = connection.prepare(
        "SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name != 'channel_account_aliases' ORDER BY name",
    ).unwrap();
    let tables = statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    tables
        .into_iter()
        .map(|table| {
            let mut statement = connection
                .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
                .unwrap();
            let columns = statement.column_count();
            let rows = statement
                .query_map([], |row| {
                    (0..columns)
                        .map(|column| row.get(column))
                        .collect::<Result<Vec<Value>, _>>()
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            (table, rows)
        })
        .collect()
}

#[test]
fn canonical_lookup_uses_one_hop_and_preserves_empty_and_unregistered_accounts() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sessions.sqlite");
    let store = SessionStore::open(&path).unwrap();
    store
        .register_channel_account("owner", "channel", "canonical", &["legacy".into()])
        .unwrap();
    drop(store);
    let store = SessionStore::open(&path).unwrap();
    for (owner, source, account, expected) in [
        ("owner", "channel", "legacy", "canonical"),
        ("owner", "channel", "canonical", "canonical"),
        ("owner", "channel", "", ""),
        ("owner", "channel", "unregistered", "unregistered"),
        ("other", "channel", "legacy", "legacy"),
        ("owner", "other", "legacy", "legacy"),
    ] {
        assert_eq!(
            store
                .canonical_channel_account(owner, source, account)
                .unwrap(),
            expected
        );
    }
    let before = aliases(&path);
    assert!(matches!(
        store.register_channel_account("owner", "channel", "legacy", &[]),
        Err(SessionError::Conflict(_))
    ));
    assert_eq!(aliases(&path), before);
}

#[test]
fn registration_deduplicates_aliases_and_survives_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sessions.sqlite");
    let store = SessionStore::open(&path).unwrap();
    store
        .register_channel_account(
            "owner",
            "channel",
            "canonical",
            &["legacy".into(), "legacy".into(), "canonical".into()],
        )
        .unwrap();
    let before = aliases(&path);
    assert_eq!(
        before,
        vec![
            (
                "owner".into(),
                "channel".into(),
                "canonical".into(),
                "canonical".into()
            ),
            (
                "owner".into(),
                "channel".into(),
                "legacy".into(),
                "canonical".into()
            ),
        ]
    );
    store
        .register_channel_account("owner", "channel", "canonical", &["legacy".into()])
        .unwrap();
    assert_eq!(aliases(&path), before);
    drop(store);
    let store = SessionStore::open(&path).unwrap();
    store
        .register_channel_account("owner", "channel", "canonical", &[])
        .unwrap();
    assert_eq!(aliases(&path), before);
    store
        .register_channel_account("owner", "channel", "canonical", &["second".into()])
        .unwrap();
    assert_eq!(aliases(&path).len(), 3);
}

#[test]
fn conflicting_registration_leaves_no_partial_mapping() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sessions.sqlite");
    let store = SessionStore::open(&path).unwrap();
    store
        .register_channel_account("owner", "channel", "canonical", &["legacy".into()])
        .unwrap();
    let before = aliases(&path);
    for (canonical, proposed) in [
        ("other", vec!["a-new".into(), "legacy".into()]),
        ("legacy", vec!["canonical".into()]),
        ("legacy", vec!["other".into()]),
        ("other", vec!["canonical".into()]),
    ] {
        assert!(matches!(
            store.register_channel_account("owner", "channel", canonical, &proposed),
            Err(SessionError::Conflict(_))
        ));
        assert_eq!(aliases(&path), before);
    }
}

#[test]
fn aliases_are_scoped_to_owner_and_source() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sessions.sqlite");
    let store = SessionStore::open(&path).unwrap();
    for (owner, source, canonical) in [
        ("owner", "channel", "first"),
        ("other", "channel", "second"),
        ("owner", "other", "third"),
    ] {
        store
            .register_channel_account(owner, source, canonical, &["shared".into()])
            .unwrap();
    }
    let rows = aliases(&path);
    assert_eq!(rows.len(), 6);
    for (owner, source, canonical) in [
        ("owner", "channel", "first"),
        ("other", "channel", "second"),
        ("owner", "other", "third"),
    ] {
        assert!(rows.contains(&(
            owner.into(),
            source.into(),
            "shared".into(),
            canonical.into()
        )));
    }
    assert!(matches!(
        store.register_channel_account("owner", "channel", "second", &["shared".into()]),
        Err(SessionError::Conflict(_))
    ));
    assert_eq!(aliases(&path), rows);
}

#[test]
fn invalid_identity_rejects_the_entire_registration() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sessions.sqlite");
    let store = SessionStore::open(&path).unwrap();
    let invalid = [
        "".into(),
        " ".into(),
        ".".into(),
        "..".into(),
        "bad\0value".into(),
        "bad\nvalue".into(),
        "bad\u{7f}value".into(),
        "bad\u{85}value".into(),
        "a".repeat(257),
        "é".repeat(129),
    ];
    for value in invalid {
        assert!(matches!(
            store.register_channel_account(&value, "channel", "canonical", &[]),
            Err(SessionError::Invalid(_))
        ));
        assert!(matches!(
            store.register_channel_account("owner", &value, "canonical", &[]),
            Err(SessionError::Invalid(_))
        ));
        assert!(matches!(
            store.register_channel_account("owner", "channel", &value, &[]),
            Err(SessionError::Invalid(_))
        ));
        assert!(matches!(
            store.register_channel_account(
                "owner",
                "channel",
                "canonical",
                &["valid".into(), value]
            ),
            Err(SessionError::Invalid(_))
        ));
        assert!(aliases(&path).is_empty());
    }
    let boundary = "é".repeat(128);
    store
        .register_channel_account(&boundary, &boundary, &boundary, &["a".repeat(256)])
        .unwrap();
    assert_eq!(aliases(&path).len(), 2);
}

#[test]
fn independent_connections_register_the_same_account_atomically() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sessions.sqlite");
    drop(SessionStore::open(&path).unwrap());
    let stores = (0..6)
        .map(|_| SessionStore::open(&path).unwrap())
        .collect::<Vec<_>>();
    let barrier = Arc::new(Barrier::new(stores.len()));
    let workers = stores
        .into_iter()
        .enumerate()
        .map(|(index, store)| {
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                store.register_channel_account(
                    "owner",
                    "channel",
                    "canonical",
                    &["shared".into(), format!("alias-{index}")],
                )
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        worker.join().unwrap().unwrap();
    }
    let rows = aliases(&path);
    assert_eq!(rows.len(), 8);
    assert!(rows.iter().all(|row| row.3 == "canonical"));
}

#[test]
fn competing_canonicals_cannot_claim_the_same_alias() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sessions.sqlite");
    drop(SessionStore::open(&path).unwrap());
    let stores = (0..2)
        .map(|_| SessionStore::open(&path).unwrap())
        .collect::<Vec<_>>();
    let barrier = Arc::new(Barrier::new(stores.len()));
    let workers = stores
        .into_iter()
        .enumerate()
        .map(|(index, store)| {
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                store.register_channel_account(
                    "owner",
                    "channel",
                    &format!("canonical-{index}"),
                    &["shared".into()],
                )
            })
        })
        .collect::<Vec<_>>();
    let results = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(SessionError::Conflict(_))))
            .count(),
        1
    );
    let rows = aliases(&path);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].3, rows[1].3);
    assert!(rows.iter().any(|row| row.2 == row.3));
}

#[test]
fn v8_migration_and_registration_preserve_existing_session_and_input_history() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sessions.sqlite");
    let store = SessionStore::open(&path).unwrap();
    let admitted = store
        .admit_channel_inbound(
            "owner",
            ChannelInboundRequest {
                inbound_id: "message".into(),
                identity: ChannelIdentity {
                    source: "channel".into(),
                    account: Some("legacy".into()),
                    conversation_kind: Default::default(),
                    conversation_id: "conversation".into(),
                    sender_id: "sender".into(),
                },
                graph: "graph".into(),
                reply_node: "work".into(),
                text: Some("hello".into()),
                attachments: Default::default(),
                run_id: None,
                replace_running: true,
            },
        )
        .unwrap();
    store
        .bind_channel_assistant(
            "owner",
            &admitted.session.id,
            "run",
            "wait",
            "work",
            "reply",
        )
        .unwrap();
    store
        .claim_channel_assistant_input("owner", &admitted.session.id, "run", "wait/1")
        .unwrap()
        .unwrap();
    store
        .finish_turn(
            "owner",
            &admitted.session.id,
            &admitted.turn.id,
            TurnStatus::Completed,
            None,
        )
        .unwrap();
    drop(store);
    let before = history(&path);
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch("DROP TABLE channel_account_aliases; PRAGMA user_version = 8;")
        .unwrap();
    drop(connection);
    let barrier = Arc::new(Barrier::new(3));
    let workers = (0..3)
        .map(|_| {
            let path = path.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                let store = SessionStore::open(path).unwrap();
                store
                    .register_channel_account("owner", "channel", "canonical", &["legacy".into()])
                    .unwrap();
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(history(&path), before);
    assert_eq!(aliases(&path).len(), 2);
    let connection = Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i32>(0))
            .unwrap(),
        9
    );
    let store = SessionStore::open(&path).unwrap();
    assert_eq!(
        store
            .get_channel_assistant("owner", &admitted.session.id)
            .unwrap()
            .unwrap()
            .run_id,
        "run"
    );
    assert_eq!(
        store
            .get_channel_assistant_input("owner", &admitted.session.id, "run", "wait/1")
            .unwrap()
            .unwrap()
            .request
            .identity
            .account
            .as_deref(),
        Some("legacy")
    );
}
