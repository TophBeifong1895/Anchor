use std::fs;

use rusqlite::Connection;

use super::{APPLICATION_ID, SCHEMA_VERSION, expected_schema, initialize, open, schema};
use crate::SessionError;

#[test]
fn fresh_database_creates_the_exact_current_schema_and_reopens_with_its_data() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sessions.sqlite");
    let connection = open(&path).unwrap();
    assert_eq!(schema(&connection).unwrap(), expected_schema());
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i32>(0))
            .unwrap(),
        SCHEMA_VERSION
    );
    assert_eq!(
        connection
            .pragma_query_value(None, "application_id", |row| row.get::<_, i32>(0))
            .unwrap(),
        APPLICATION_ID
    );
    assert_eq!(
        connection
            .pragma_query_value(None, "journal_mode", |row| row.get::<_, String>(0))
            .unwrap(),
        "wal"
    );
    connection
        .execute(
            "INSERT INTO sessions VALUES ('saved', 'alice', '{}', 'unchanged')",
            [],
        )
        .unwrap();
    drop(connection);
    let reopened = open(&path).unwrap();
    assert_eq!(schema(&reopened).unwrap(), expected_schema());
    assert_eq!(
        reopened
            .query_row("SELECT owner FROM sessions WHERE id = 'saved'", [], |row| {
                row.get::<_, String>(0)
            })
            .unwrap(),
        "alice"
    );
}

#[test]
fn older_versions_are_refused_without_changing_bytes_version_or_facts() {
    for (version, journal) in [(1, "delete"), (8, "delete"), (1, "wal"), (8, "wal")] {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.sqlite");
        let path = directory.path().join("sessions.sqlite");
        let connection = Connection::open(&source).unwrap();
        connection
            .pragma_update(None, "journal_mode", journal)
            .unwrap();
        connection
            .execute_batch(
                "CREATE TABLE preserved(value TEXT); INSERT INTO preserved VALUES ('untouched');",
            )
            .unwrap();
        connection
            .pragma_update(None, "application_id", APPLICATION_ID)
            .unwrap();
        connection
            .pragma_update(None, "user_version", version)
            .unwrap();
        // Copy a live WAL snapshot so the rejected database would checkpoint
        // on closing a read-write connection. No user database is involved.
        fs::copy(&source, &path).unwrap();
        let wal_before = if journal == "wal" {
            let wal = fs::read(source.with_extension("sqlite-wal")).unwrap();
            fs::write(path.with_extension("sqlite-wal"), &wal).unwrap();
            Some(wal)
        } else {
            None
        };
        let before = fs::read(&path).unwrap();
        assert!(matches!(
            open(&path),
            Err(SessionError::Storage(message)) if message == format!(
                "incompatible database: expected exact Anchor platform Session schema version 9 (found version {version})"
            )
        ));
        assert_eq!(
            fs::read(&path).unwrap(),
            before,
            "version {version}, journal {journal}"
        );
        assert_eq!(fs::read(path.with_extension("sqlite-wal")).ok(), wal_before);
        assert!(!path.with_extension("sqlite-journal").exists());
        let readonly =
            Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        assert_eq!(
            readonly
                .pragma_query_value(None, "user_version", |row| row.get::<_, i32>(0))
                .unwrap(),
            version
        );
        assert_eq!(
            readonly
                .query_row("SELECT value FROM preserved", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            "untouched"
        );
    }
}

#[test]
fn altered_current_schema_identity_and_future_version_are_refused_without_changes() {
    for alteration in [
        "DROP INDEX channel_assistants_current_session",
        "DROP INDEX sessions_owner_updated",
        "CREATE TABLE unexpected(value TEXT)",
        "CREATE VIEW unexpected AS SELECT * FROM sessions",
        "CREATE TRIGGER unexpected AFTER INSERT ON sessions BEGIN SELECT 1; END",
        "ALTER TABLE sessions ADD COLUMN extra TEXT",
        "DROP TABLE session_events; CREATE TABLE session_events(session_id TEXT, seq INTEGER)",
        "CREATE INDEX unexpected_alias_index ON channel_account_aliases(canonical)",
        "PRAGMA application_id = 0",
        "PRAGMA application_id = 42",
        "PRAGMA user_version = 0",
        "PRAGMA user_version = 999",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("sessions.sqlite");
        let connection = open(&path).unwrap();
        connection.execute_batch(
            "INSERT INTO sessions VALUES ('protected', 'local', '{}', 'unchanged');
             INSERT INTO channel_account_aliases VALUES ('owner', 'channel', 'canonical', 'canonical');
             INSERT INTO channel_account_aliases VALUES ('owner', 'channel', 'legacy', 'canonical');
             PRAGMA journal_mode = DELETE;",
        ).unwrap();
        connection.execute_batch(alteration).unwrap();
        let original_schema = schema(&connection).unwrap();
        drop(connection);
        let before = fs::read(&path).unwrap();
        assert!(
            matches!(open(&path), Err(SessionError::Storage(_))),
            "{alteration}"
        );
        assert_eq!(fs::read(&path).unwrap(), before, "{alteration}");
        let connection = Connection::open(&path).unwrap();
        assert_eq!(
            schema(&connection).unwrap(),
            original_schema,
            "{alteration}"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT canonical FROM channel_account_aliases WHERE alias = 'legacy'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "canonical",
            "{alteration}"
        );
    }
}

#[test]
fn failed_fresh_creation_rolls_back_schema_and_identity() {
    let mut connection = Connection::open_in_memory().unwrap();
    connection.pragma_update(None, "max_page_count", 2).unwrap();
    assert!(
        matches!(initialize(&mut connection), Err(SessionError::Storage(message)) if message.contains("full"))
    );
    assert!(schema(&connection).unwrap().is_empty());
    for pragma in ["application_id", "user_version"] {
        assert_eq!(
            connection
                .pragma_query_value(None, pragma, |row| row.get::<_, i32>(0))
                .unwrap(),
            0
        );
    }
    connection
        .pragma_update(None, "max_page_count", 100)
        .unwrap();
    initialize(&mut connection).unwrap();
    assert_eq!(schema(&connection).unwrap(), expected_schema());
}
