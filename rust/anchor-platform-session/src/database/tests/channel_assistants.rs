use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Barrier},
    thread,
};

use rusqlite::{Connection, types::Value};
use serde_json::json;
use tempfile::TempDir;

use super::super::{
    APPLICATION_ID, CHANNEL_ASSISTANT_SCHEMA, CHANNEL_SCHEMA, GOOSE_SCHEMA, QUESTION_SCHEMA,
    RELATION_SCHEMA, SCHEMA_V1, SCHEMA_VERSION, TURN_SCHEMA, expected_schema, migrate,
    migrate_channel_assistants, migrate_channel_conversations, schema,
};
use crate::{
    AttachmentManifest, ChannelDeliveryRequest, ChannelIdentity, ChannelInboundRequest,
    CreateSession, GooseExecution, NativeExecution, SessionError, SessionStore, TurnStatus,
};

fn inbound(id: &str, run: Option<&str>) -> ChannelInboundRequest {
    ChannelInboundRequest {
        inbound_id: id.into(),
        identity: ChannelIdentity {
            conversation_kind: Default::default(),
            source: "wecom".into(),
            account: Some("corp".into()),
            conversation_id: "conversation".into(),
            sender_id: "sender".into(),
        },
        graph: "graph".into(),
        reply_node: "assistant".into(),
        text: Some(format!("message {id}")),
        attachments: AttachmentManifest::default(),
        run_id: run.map(str::to_owned),
        replace_running: true,
    }
}

fn downgrade_channel_sessions(connection: &Connection) {
    connection.execute("CREATE TEMP TABLE preserved_sessions AS SELECT session_id, owner, source, account, conversation_id, participant_id FROM channel_sessions", []).unwrap();
    connection
        .execute("DROP TABLE channel_sessions", [])
        .unwrap();
    connection.execute(CHANNEL_SCHEMA[0].2, []).unwrap();
    connection.execute(CHANNEL_SCHEMA[1].2, []).unwrap();
    connection
        .execute(
            "INSERT INTO channel_sessions SELECT * FROM temp.preserved_sessions",
            [],
        )
        .unwrap();
    connection
        .execute("DROP TABLE temp.preserved_sessions", [])
        .unwrap();
}

fn v7_fixture() -> (TempDir, PathBuf, String, String) {
    let (directory, path, _) = v6_fixture();
    let store = SessionStore::open(&path).unwrap();
    let mut first_request = inbound("retired-input", None);
    first_request.identity.conversation_id = "assistant-conversation".into();
    let first = store
        .admit_channel_inbound("owner", first_request.clone())
        .unwrap();
    let session = first.session.id;
    store
        .bind_channel_assistant(
            "owner",
            &session,
            "retired-assistant",
            "wait",
            "assistant",
            "reply",
        )
        .unwrap();
    store
        .claim_channel_assistant_input("owner", &session, "retired-assistant", "wait/1")
        .unwrap()
        .unwrap();
    store
        .finish_turn(
            "owner",
            &session,
            &first.turn.id,
            TurnStatus::Completed,
            None,
        )
        .unwrap();
    store
        .retire_channel_assistant("owner", &session, "retired-assistant")
        .unwrap();
    first_request.inbound_id = "current-input".into();
    let current = store.admit_channel_inbound("owner", first_request).unwrap();
    store
        .bind_channel_assistant(
            "owner",
            &session,
            "current-assistant",
            "wait",
            "assistant",
            "reply",
        )
        .unwrap();
    store
        .claim_channel_assistant_input("owner", &session, "current-assistant", "wait/2")
        .unwrap()
        .unwrap();
    store
        .bind_goose(
            "owner",
            &session,
            &current.turn.id,
            GooseExecution {
                scope: "d".repeat(64),
                session: "group-upgrade-history".into(),
            },
        )
        .unwrap();
    let delivery = ChannelDeliveryRequest {
        key: "unfinished-current".into(),
        kind: "text".into(),
        content_sha256: "e".repeat(64),
    };
    store
        .admit_channel_delivery("owner", &session, &current.turn.id, delivery.clone())
        .unwrap();
    store
        .begin_channel_delivery("owner", &session, &current.turn.id, delivery)
        .unwrap();
    drop(store);
    let mut connection = Connection::open(&path).unwrap();
    connection
        .pragma_update(None, "foreign_keys", true)
        .unwrap();
    let transaction = connection.transaction().unwrap();
    for table in ["channel_assistants", "channel_assistant_inputs"] {
        transaction
            .execute(
                &format!("CREATE TEMP TABLE preserved_{table} AS SELECT * FROM {table}"),
                [],
            )
            .unwrap();
    }
    transaction
        .execute("DROP TABLE channel_assistant_inputs", [])
        .unwrap();
    transaction
        .execute("DROP TABLE channel_assistants", [])
        .unwrap();
    downgrade_channel_sessions(&transaction);
    for (_, _, statement) in CHANNEL_ASSISTANT_SCHEMA.iter().skip(1) {
        transaction.execute(statement, []).unwrap();
    }
    for table in ["channel_assistants", "channel_assistant_inputs"] {
        transaction
            .execute(
                &format!("INSERT INTO {table} SELECT * FROM temp.preserved_{table}"),
                [],
            )
            .unwrap();
        transaction
            .execute(&format!("DROP TABLE temp.preserved_{table}"), [])
            .unwrap();
    }
    transaction.pragma_update(None, "user_version", 7).unwrap();
    transaction.commit().unwrap();
    assert_eq!(schema(&connection).unwrap(), expected_schema(7));
    (directory, path, session, current.turn.id)
}

fn assistant_facts(connection: &Connection) -> Vec<Vec<Vec<Value>>> {
    ["channel_assistants", "channel_assistant_inputs"]
        .iter()
        .map(|table| {
            let mut statement = connection
                .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
                .unwrap();
            let columns = statement.column_count();
            statement
                .query_map([], |row| {
                    (0..columns)
                        .map(|column| row.get(column))
                        .collect::<rusqlite::Result<Vec<Value>>>()
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        })
        .collect()
}

#[test]
fn v7_group_scope_migration_preserves_legacy_sessions_native_links_assistants_inputs_and_delivery()
{
    let (_directory, path, session, turn) = v7_fixture();
    let connection = Connection::open(&path).unwrap();
    let before = old_facts(&connection);
    let assistants = assistant_facts(&connection);
    let store = SessionStore::open(&path).unwrap();
    assert_eq!(old_facts(&connection), before);
    assert_eq!(assistant_facts(&connection), assistants);
    assert_eq!(
        schema(&connection).unwrap(),
        expected_schema(SCHEMA_VERSION)
    );
    assert!(
        !connection
            .prepare("PRAGMA foreign_key_check")
            .unwrap()
            .exists([])
            .unwrap()
    );
    assert_eq!(
        store
            .get_channel_assistant("owner", &session)
            .unwrap()
            .unwrap()
            .run_id,
        "current-assistant"
    );
    assert_eq!(
        store
            .get_channel_assistant_input("owner", &session, "retired-assistant", "wait/1")
            .unwrap()
            .unwrap()
            .request
            .inbound_id,
        "retired-input"
    );
    assert_eq!(
        store
            .get_channel_assistant_input("owner", &session, "current-assistant", "wait/2")
            .unwrap()
            .unwrap()
            .turn
            .id,
        turn
    );
    let single_scopes: bool = connection.query_row("SELECT NOT EXISTS(SELECT 1 FROM channel_sessions WHERE conversation_kind != 'single' OR participant_id != 'sender')",[],|row|row.get(0)).unwrap();
    assert!(single_scopes);
    let mut next = inbound("post-upgrade", None);
    next.identity.conversation_id = "assistant-conversation".into();
    assert_eq!(
        store
            .admit_channel_inbound("owner", next)
            .unwrap()
            .session
            .id,
        session
    );
}

fn v6_fixture() -> (TempDir, PathBuf, String) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sessions.sqlite");
    let store = SessionStore::open(&path).unwrap();
    let pilot = store.create("owner", CreateSession::default()).unwrap();
    let first = store
        .create_turn("owner", &pilot.id, "native-turn", Some("old prompt"))
        .unwrap()
        .0;
    store
        .bind_native(
            "owner",
            &pilot.id,
            &first.id,
            NativeExecution {
                scope: "a".repeat(64),
                session: 1,
                run: 1,
            },
        )
        .unwrap();
    store
        .associate_run("owner", &pilot.id, &first.id, "pilot-run")
        .unwrap();
    store
        .append_turn_event(
            "owner",
            &pilot.id,
            &first.id,
            json!({"type":"text-delta", "delta":"old event"}),
        )
        .unwrap();
    store
        .create_question(
            "owner",
            &pilot.id,
            &first.id,
            "Confirm?",
            json!({"type":"object", "properties":{"confirm":{"type":"boolean"}}}),
        )
        .unwrap();
    store
        .finish_turn("owner", &pilot.id, &first.id, TurnStatus::Completed, None)
        .unwrap();
    let second = store
        .create_turn("owner", &pilot.id, "goose-turn", None)
        .unwrap()
        .0;
    store
        .bind_goose(
            "owner",
            &pilot.id,
            &second.id,
            GooseExecution {
                scope: "b".repeat(64),
                session: "opaque-native-session".into(),
            },
        )
        .unwrap();
    store
        .finish_turn("owner", &pilot.id, &second.id, TurnStatus::Completed, None)
        .unwrap();
    let first = store
        .admit_channel_inbound("owner", inbound("old-1", Some("old-run-1")))
        .unwrap();
    let delivery = ChannelDeliveryRequest {
        key: "old-delivery".into(),
        kind: "text".into(),
        content_sha256: "c".repeat(64),
    };
    store
        .admit_channel_delivery("owner", &first.session.id, &first.turn.id, delivery.clone())
        .unwrap();
    store
        .begin_channel_delivery("owner", &first.session.id, &first.turn.id, delivery)
        .unwrap();
    let second = store
        .admit_channel_inbound("owner", inbound("old-2", Some("old-run-2")))
        .unwrap();
    drop(store);
    let mut connection = Connection::open(&path).unwrap();
    connection
        .pragma_update(None, "foreign_keys", true)
        .unwrap();
    let transaction = connection.transaction().unwrap();
    transaction
        .execute("DROP TABLE channel_assistant_inputs", [])
        .unwrap();
    transaction
        .execute("DROP TABLE channel_assistants", [])
        .unwrap();
    transaction
        .execute(
            "CREATE TEMP TABLE preserved_inbounds AS SELECT * FROM channel_inbounds",
            [],
        )
        .unwrap();
    transaction
        .execute("DROP TABLE channel_inbounds", [])
        .unwrap();
    for (_, name, statement) in CHANNEL_SCHEMA {
        if name == "channel_inbounds" || name == "channel_inbounds_session_created" {
            transaction.execute(statement, []).unwrap();
        }
    }
    transaction
        .execute(
            "INSERT INTO channel_inbounds SELECT * FROM temp.preserved_inbounds",
            [],
        )
        .unwrap();
    transaction
        .execute("DROP TABLE temp.preserved_inbounds", [])
        .unwrap();
    downgrade_channel_sessions(&transaction);
    transaction.pragma_update(None, "user_version", 6).unwrap();
    transaction.commit().unwrap();
    connection.execute_batch("VACUUM").unwrap();
    assert_eq!(schema(&connection).unwrap(), expected_schema(6));
    (directory, path, second.session.id)
}

fn old_facts(connection: &Connection) -> BTreeMap<String, Vec<Vec<Value>>> {
    SCHEMA_V1
        .iter()
        .chain(TURN_SCHEMA.iter())
        .chain(RELATION_SCHEMA.iter())
        .chain(GOOSE_SCHEMA.iter())
        .chain(QUESTION_SCHEMA.iter())
        .chain(CHANNEL_SCHEMA.iter())
        .filter(|(kind, _, _)| *kind == "table")
        .map(|(_, name, _)| {
            let query = if *name == "channel_sessions" && connection.prepare("SELECT conversation_kind FROM channel_sessions").is_ok() {
                "SELECT session_id, owner, source, account, conversation_id, participant_id AS sender_id FROM channel_sessions ORDER BY rowid".into()
            } else {
                format!("SELECT * FROM {name} ORDER BY rowid")
            };
            let mut statement = connection
                .prepare(&query)
                .unwrap();
            let columns = statement.column_count();
            let rows = statement
                .query_map([], |row| {
                    (0..columns)
                        .map(|column| row.get(column))
                        .collect::<rusqlite::Result<Vec<Value>>>()
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            (name.to_string(), rows)
        })
        .collect()
}

#[test]
fn v6_migration_preserves_every_old_table_index_and_legacy_run_rejection() {
    let (_directory, path, session) = v6_fixture();
    let connection = Connection::open(&path).unwrap();
    let before = old_facts(&connection);
    let definitions = schema(&connection).unwrap();
    let store = SessionStore::open(&path).unwrap();
    assert_eq!(old_facts(&connection), before);
    let after = schema(&connection).unwrap();
    assert_eq!(after, expected_schema(SCHEMA_VERSION));
    for definition in definitions {
        if definition.1 != "channel_inbounds" && definition.1 != "channel_sessions" {
            assert!(after.contains(&definition));
        }
    }
    assert_eq!(
        connection
            .pragma_query_value(None, "application_id", |row| row.get::<_, i32>(0))
            .unwrap(),
        APPLICATION_ID
    );
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i32>(0))
            .unwrap(),
        SCHEMA_VERSION
    );
    assert!(
        !connection
            .prepare("PRAGMA foreign_key_check")
            .unwrap()
            .exists([])
            .unwrap()
    );
    assert_eq!(
        store.get_channel_assistant("owner", &session).unwrap(),
        None
    );
    store
        .bind_channel_assistant("owner", &session, "old-run-2", "wait", "assistant", "reply")
        .unwrap();
    let first = store
        .claim_channel_assistant_input("owner", &session, "old-run-2", "wait/1")
        .unwrap()
        .unwrap();
    assert_eq!(first.request.inbound_id, "old-2");
    let next = store
        .admit_channel_inbound("owner", inbound("new-3", None))
        .unwrap();
    assert!(matches!(
        store.associate_channel_run("owner", &session, "new-3", "old-run-2"),
        Err(SessionError::Conflict(_))
    ));
    assert_eq!(
        store
            .get_channel_relation("owner", &session, "new-3")
            .unwrap()
            .run_id,
        None
    );
    let second = store
        .claim_channel_assistant_input("owner", &session, "old-run-2", "wait/2")
        .unwrap()
        .unwrap();
    assert_eq!(second.turn.id, next.turn.id);
    assert_eq!(second.turn.runs, ["old-run-2"]);
    assert_eq!(
        store
            .claim_channel_assistant_input("owner", &session, "old-run-2", "wait/1")
            .unwrap()
            .unwrap()
            .turn
            .id,
        first.turn.id
    );
    drop(store);
    let reopened = SessionStore::open(path).unwrap();
    assert_eq!(
        reopened
            .get_channel_assistant_input("owner", &session, "old-run-2", "wait/2")
            .unwrap(),
        Some(second)
    );
}

#[test]
fn v6_failed_rebuild_rolls_back_schema_version_data_and_indexes() {
    let (_directory, path, _session) = v6_fixture();
    let mut connection = Connection::open(&path).unwrap();
    connection
        .pragma_update(None, "foreign_keys", true)
        .unwrap();
    let original_schema = schema(&connection).unwrap();
    let before = old_facts(&connection);
    let budget: i64 = connection
        .pragma_query_value(None, "page_count", |row| row.get(0))
        .unwrap();
    let transaction = connection.transaction().unwrap();
    migrate_channel_assistants(&transaction).unwrap();
    migrate_channel_conversations(&transaction).unwrap();
    let required: i64 = transaction
        .pragma_query_value(None, "page_count", |row| row.get(0))
        .unwrap();
    assert!(required > budget);
    transaction.rollback().unwrap();
    connection
        .pragma_update(None, "max_page_count", budget)
        .unwrap();
    let result = migrate(&mut connection);
    assert!(
        matches!(&result, Err(SessionError::Storage(message)) if message.contains("full")),
        "{result:?}"
    );
    assert_eq!(schema(&connection).unwrap(), original_schema);
    assert_eq!(old_facts(&connection), before);
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i32>(0))
            .unwrap(),
        6
    );
    assert_eq!(
        connection
            .pragma_query_value(None, "application_id", |row| row.get::<_, i32>(0))
            .unwrap(),
        APPLICATION_ID
    );
    connection
        .pragma_update(None, "max_page_count", required)
        .unwrap();
    migrate(&mut connection).unwrap();
    assert_eq!(old_facts(&connection), before);
    assert_eq!(
        schema(&connection).unwrap(),
        expected_schema(SCHEMA_VERSION)
    );
}

#[test]
fn concurrent_v6_migrations_preserve_all_old_facts() {
    let (_directory, path, session) = v6_fixture();
    let before = old_facts(&Connection::open(&path).unwrap());
    let barrier = Arc::new(Barrier::new(3));
    let workers = (0..3)
        .map(|_| {
            let barrier = barrier.clone();
            let path = path.clone();
            thread::spawn(move || {
                barrier.wait();
                SessionStore::open(path).unwrap()
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        assert_eq!(
            worker
                .join()
                .unwrap()
                .get_channel_assistant("owner", &session)
                .unwrap(),
            None
        );
    }
    assert_eq!(old_facts(&Connection::open(&path).unwrap()), before);
}

#[test]
fn altered_v6_current_and_future_schemas_are_rejected_before_any_mutation() {
    for (version, alteration) in [
        (6, "DROP INDEX channel_inbounds_session_created"),
        (8, "DROP INDEX channel_inbounds_run"),
        (8, "PRAGMA user_version = 9"),
        (8, "PRAGMA application_id = 0"),
    ] {
        let (_directory, path, _session) = v6_fixture();
        let mut connection = Connection::open(&path).unwrap();
        if version == 8 {
            migrate(&mut connection).unwrap();
        }
        connection.execute_batch(alteration).unwrap();
        let original_schema = schema(&connection).unwrap();
        let before = old_facts(&connection);
        let version: i32 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        let application: i32 = connection
            .pragma_query_value(None, "application_id", |row| row.get(0))
            .unwrap();
        assert!(matches!(
            SessionStore::open(&path),
            Err(SessionError::Storage(_))
        ));
        assert_eq!(schema(&connection).unwrap(), original_schema);
        assert_eq!(old_facts(&connection), before);
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i32>(0))
                .unwrap(),
            version
        );
        assert_eq!(
            connection
                .pragma_query_value(None, "application_id", |row| row.get::<_, i32>(0))
                .unwrap(),
            application
        );
    }
}
