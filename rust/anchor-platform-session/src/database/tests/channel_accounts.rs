use rusqlite::{Connection, types::Value};

use super::super::{APPLICATION_ID, SCHEMA_VERSION, expected_schema, migrate, schema};
use crate::SessionError;

fn v8_database() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    connection.pragma_update(None, "page_size", 512).unwrap();
    connection
        .pragma_update(None, "foreign_keys", true)
        .unwrap();
    let definitions = expected_schema(8);
    for kind in ["table", "index"] {
        for (object_kind, _, statement) in &definitions {
            if object_kind == kind {
                connection.execute(statement, []).unwrap();
            }
        }
    }
    connection
        .pragma_update(None, "application_id", APPLICATION_ID)
        .unwrap();
    connection.pragma_update(None, "user_version", 8).unwrap();
    connection.execute_batch(
        "INSERT INTO sessions VALUES ('session', 'owner', '{\"preserved\":true}', 'original');
         INSERT INTO session_events VALUES ('session', 1, 'original', 'created', '{}');
         INSERT INTO turns VALUES ('turn', 'session', 'request', 'hello', 'completed', NULL, 'original', 'original');
         INSERT INTO turn_events VALUES ('turn', 1, '{\"type\":\"completed\"}');
         INSERT INTO channel_sessions VALUES ('session', 'owner', 'channel', 'legacy', 'single', 'chat', 'sender');
         INSERT INTO channel_inbounds VALUES ('session', 'inbound', 'turn', 'assistant', '{}', NULL, 'original');
         INSERT INTO channel_assistants VALUES ('session', 'assistant', 'wait', 'work', 'reply', 0);
         INSERT INTO channel_assistant_inputs VALUES ('session', 'assistant', 'wait/1', 'turn');",
    ).unwrap();
    connection
}

fn facts(connection: &Connection) -> Vec<(String, Vec<Vec<Value>>)> {
    let mut tables = connection.prepare(
        "SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name != 'channel_account_aliases' ORDER BY name",
    ).unwrap();
    let names = tables
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    names
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
fn v8_upgrade_adds_only_alias_schema_and_preserves_all_facts() {
    let mut connection = v8_database();
    let before = facts(&connection);
    assert_eq!(schema(&connection).unwrap(), expected_schema(8));
    migrate(&mut connection).unwrap();
    assert_eq!(
        schema(&connection).unwrap(),
        expected_schema(SCHEMA_VERSION)
    );
    assert_eq!(facts(&connection), before);
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i32>(0))
            .unwrap(),
        9
    );
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM channel_account_aliases", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        0
    );
    migrate(&mut connection).unwrap();
    assert_eq!(facts(&connection), before);
}

#[test]
fn v8_upgrade_failure_preserves_schema_version_and_history() {
    let mut connection = v8_database();
    let before = facts(&connection);
    let original_schema = schema(&connection).unwrap();
    let budget: i64 = connection
        .pragma_query_value(None, "page_count", |row| row.get(0))
        .unwrap();
    connection
        .pragma_update(None, "max_page_count", budget)
        .unwrap();
    let result = migrate(&mut connection);
    assert!(
        matches!(&result, Err(SessionError::Storage(message)) if message.contains("full")),
        "{result:?}"
    );
    assert_eq!(schema(&connection).unwrap(), original_schema);
    assert_eq!(facts(&connection), before);
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i32>(0))
            .unwrap(),
        8
    );
    assert_eq!(
        connection
            .pragma_query_value(None, "application_id", |row| row.get::<_, i32>(0))
            .unwrap(),
        APPLICATION_ID
    );
    connection
        .pragma_update(None, "max_page_count", budget + 16)
        .unwrap();
    migrate(&mut connection).unwrap();
    assert_eq!(facts(&connection), before);
}

#[test]
fn altered_v8_schema_is_rejected_before_any_mutation() {
    for alteration in [
        "DROP INDEX channel_assistants_current_session",
        "CREATE TABLE channel_account_aliases (owner TEXT)",
        "CREATE TRIGGER altered AFTER INSERT ON sessions BEGIN SELECT 1; END",
        "PRAGMA application_id = 0",
    ] {
        let mut connection = v8_database();
        connection.execute_batch(alteration).unwrap();
        let before = facts(&connection);
        let original_schema = schema(&connection).unwrap();
        let application: i32 = connection
            .pragma_query_value(None, "application_id", |row| row.get(0))
            .unwrap();
        assert!(matches!(
            migrate(&mut connection),
            Err(SessionError::Storage(_))
        ));
        assert_eq!(schema(&connection).unwrap(), original_schema);
        assert_eq!(facts(&connection), before);
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i32>(0))
                .unwrap(),
            8
        );
        assert_eq!(
            connection
                .pragma_query_value(None, "application_id", |row| row.get::<_, i32>(0))
                .unwrap(),
            application
        );
    }
}

#[test]
fn altered_v9_schema_is_rejected_without_repairing_or_erasing_aliases() {
    let mut connection = v8_database();
    migrate(&mut connection).unwrap();
    connection.execute_batch(
        "INSERT INTO channel_account_aliases VALUES ('owner', 'channel', 'canonical', 'canonical');
         INSERT INTO channel_account_aliases VALUES ('owner', 'channel', 'legacy', 'canonical');
         CREATE INDEX unexpected_alias_index ON channel_account_aliases(canonical);",
    ).unwrap();
    let before = facts(&connection);
    let original_schema = schema(&connection).unwrap();
    assert!(matches!(
        migrate(&mut connection),
        Err(SessionError::Storage(_))
    ));
    assert_eq!(schema(&connection).unwrap(), original_schema);
    assert_eq!(facts(&connection), before);
    assert_eq!(
        connection
            .query_row(
                "SELECT canonical FROM channel_account_aliases WHERE alias = 'legacy'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "canonical"
    );
}
