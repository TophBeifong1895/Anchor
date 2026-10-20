use std::collections::BTreeSet;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use super::{MAX_CHANNEL_VALUE_BYTES, validate_channel_value};
use crate::{SessionError, SessionStore, store};

impl SessionStore {
    pub fn canonical_channel_account(
        &self,
        owner: &str,
        source: &str,
        account: &str,
    ) -> Result<String, SessionError> {
        let connection = self.lock()?;
        canonical_account(&connection, owner, source, account)
    }

    pub fn register_channel_account(
        &self,
        owner: &str,
        source: &str,
        canonical: &str,
        aliases: &[String],
    ) -> Result<(), SessionError> {
        store::validate_identity(owner, "owner")?;
        validate_channel_value(source, "channel source", MAX_CHANNEL_VALUE_BYTES)?;
        validate_channel_value(
            canonical,
            "canonical channel account",
            MAX_CHANNEL_VALUE_BYTES,
        )?;
        let mut identities = BTreeSet::from([canonical]);
        for alias in aliases {
            validate_channel_value(alias, "channel account alias", MAX_CHANNEL_VALUE_BYTES)?;
            identities.insert(alias.as_str());
        }
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for alias in &identities {
            if registered_account(&transaction, owner, source, alias)?
                .is_some_and(|existing| existing != canonical)
            {
                return Err(SessionError::Conflict(
                    "channel account alias already belongs to a different canonical account".into(),
                ));
            }
        }
        for alias in identities {
            transaction.execute(
                "INSERT INTO channel_account_aliases(owner, source, alias, canonical)
                 VALUES (?1, ?2, ?3, ?4) ON CONFLICT(owner, source, alias) DO NOTHING",
                params![owner, source, alias, canonical],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }
}

pub(super) fn canonical_account(
    connection: &Connection,
    owner: &str,
    source: &str,
    account: &str,
) -> Result<String, SessionError> {
    Ok(registered_account(connection, owner, source, account)?
        .unwrap_or_else(|| account.to_owned()))
}

fn registered_account(
    connection: &Connection,
    owner: &str,
    source: &str,
    account: &str,
) -> Result<Option<String>, SessionError> {
    Ok(connection
        .query_row(
            "SELECT canonical FROM channel_account_aliases
             WHERE owner = ?1 AND source = ?2 AND alias = ?3",
            params![owner, source, account],
            |row| row.get(0),
        )
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::canonical_account;
    use crate::{SessionError, SessionStore};

    #[test]
    fn failed_insert_rolls_back_all_new_aliases_and_preserves_existing_mapping() {
        let directory = tempfile::tempdir().unwrap();
        let store = SessionStore::open(directory.path().join("sessions.sqlite")).unwrap();
        store
            .register_channel_account("owner", "channel", "existing", &["legacy".into()])
            .unwrap();
        store.lock().unwrap().execute_batch(
            "CREATE TEMP TRIGGER fail_channel_alias BEFORE INSERT ON main.channel_account_aliases
             WHEN NEW.alias = 'z-fail' BEGIN SELECT RAISE(ABORT, 'injected alias failure'); END;",
        ).unwrap();
        let result = store.register_channel_account(
            "owner",
            "channel",
            "new-canonical",
            &["a-first".into(), "z-fail".into()],
        );
        assert!(
            matches!(&result, Err(SessionError::Storage(message)) if message.contains("injected alias failure")),
            "{result:?}"
        );
        assert_eq!(
            store
                .canonical_channel_account("owner", "channel", "legacy")
                .unwrap(),
            "existing"
        );
        assert_eq!(
            store
                .lock()
                .unwrap()
                .query_row("SELECT count(*) FROM channel_account_aliases", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            2
        );
        store
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER temp.fail_channel_alias")
            .unwrap();
        store
            .register_channel_account(
                "owner",
                "channel",
                "new-canonical",
                &["a-first".into(), "z-fail".into()],
            )
            .unwrap();
        assert_eq!(
            store
                .canonical_channel_account("owner", "channel", "z-fail")
                .unwrap(),
            "new-canonical"
        );
    }

    #[test]
    fn canonical_lookup_preserves_unregistered_values_and_namespace_boundaries() {
        let directory = tempfile::tempdir().unwrap();
        let store = SessionStore::open(directory.path().join("sessions.sqlite")).unwrap();
        store
            .register_channel_account("owner", "channel", "canonical", &["alias".into()])
            .unwrap();
        let connection = store.lock().unwrap();
        for (owner, source, account, expected) in [
            ("owner", "channel", "alias", "canonical"),
            ("owner", "channel", "canonical", "canonical"),
            ("owner", "channel", "", ""),
            ("owner", "channel", "unregistered", "unregistered"),
            ("other", "channel", "alias", "alias"),
            ("owner", "other", "alias", "alias"),
        ] {
            assert_eq!(
                canonical_account(&connection, owner, source, account).unwrap(),
                expected
            );
        }
    }
}
