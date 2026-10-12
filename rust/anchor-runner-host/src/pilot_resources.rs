//! Durable, narrow ownership needed to delete Pilot files. This contains no
//! recovery cursor and never interprets Goose history.

use anchor_platform_session::{Session, SessionError, SessionStore, Turn};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[cfg(test)]
mod tests;

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Binding {
    format: u32,
    owner: String,
    session: String,
    created_at: String,
    workspace: PathBuf,
    goose: PathBuf,
}

fn binding(data: &Path, workspaces: &Path, owner: &str, session: &Session) -> Binding {
    Binding {
        format: 1,
        owner: owner.into(),
        session: session.id.clone(),
        created_at: session.created_at.to_rfc3339(),
        workspace: std::path::absolute(crate::pilot_tools::session_workspace(
            workspaces,
            owner,
            &session.id,
        ))
        .expect("Host workspace path"),
        goose: crate::pilot_host::scope(data, owner, session),
    }
}

fn path(data: &Path, value: &Binding) -> PathBuf {
    data.join("platform/pilot-resources").join(format!(
        "{}.json",
        value
            .goose
            .file_name()
            .expect("Pilot scope hash")
            .to_string_lossy()
    ))
}

pub(crate) fn register(
    data: &Path,
    workspaces: &Path,
    owner: &str,
    session: &Session,
) -> Result<(), String> {
    let value = binding(data, workspaces, owner, session);
    crate::assistant::save_immutable(&path(data, &value), &value)
}

fn remove_directory(path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            crate::goose_acp::pilot::directory(path)?;
            std::fs::remove_dir_all(path).map_err(|error| error.to_string())
        }
        Ok(_) => Err("owned Pilot resource is no longer a non-symlink directory".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn resource_present(path: &Path) -> Result<bool, String> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

fn adopt_legacy(
    data: &Path,
    workspaces: &Path,
    owner: &str,
    session: &Session,
    expected: &Binding,
    turns: &[Turn],
) -> Result<(), SessionError> {
    let scope = expected
        .goose
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| SessionError::Conflict("Pilot scope identity unavailable".into()))?;
    let mut native_sessions = std::collections::BTreeSet::new();
    for turn in turns {
        if let Some(goose) = &turn.goose {
            if goose.scope != scope {
                return Err(SessionError::Conflict(
                    "legacy Pilot resources have a conflicting Goose scope; inspect them before cleanup".into(),
                ));
            }
            native_sessions.insert(goose.session.clone());
        }
    }
    if native_sessions.is_empty() {
        return Err(SessionError::Conflict(
            "legacy Pilot resources have no durable Goose Turn ownership; inspect them before cleanup".into(),
        ));
    }

    for resource in [&expected.workspace, &expected.goose] {
        if !resource_present(resource).map_err(SessionError::Storage)? {
            continue;
        }
        let metadata = std::fs::symlink_metadata(resource).map_err(|error| {
            SessionError::Storage(format!("Pilot resource inspection failed: {error}"))
        })?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(SessionError::Conflict(
                "legacy Pilot resource is not a non-symlink directory; inspect it before cleanup"
                    .into(),
            ));
        }
    }

    // A Goose fact is not a recovery cursor, but when it exists it must agree
    // with the durable Turn association before a legacy root can be adopted.
    let fact = expected.goose.join("goose.json");
    if resource_present(&fact).map_err(SessionError::Storage)? {
        let metadata = std::fs::symlink_metadata(&fact).map_err(|error| {
            SessionError::Storage(format!("Pilot Goose fact inspection failed: {error}"))
        })?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() > 8 * 1024 * 1024
        {
            return Err(SessionError::Conflict(
                "legacy Goose fact is not a bounded regular file; inspect it before cleanup".into(),
            ));
        }
        let bytes = std::fs::read(&fact).map_err(|error| {
            SessionError::Storage(format!("Pilot Goose fact inspection failed: {error}"))
        })?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| {
            SessionError::Conflict(
                "legacy Goose fact is malformed; inspect it before cleanup".into(),
            )
        })?;
        if value
            .get("session_id")
            .and_then(Value::as_str)
            .is_some_and(|native| !native_sessions.contains(native))
        {
            return Err(SessionError::Conflict(
                "legacy Goose fact belongs to another Session; inspect it before cleanup".into(),
            ));
        }
    }

    register(data, workspaces, owner, session).map_err(SessionError::Storage)
}

pub(crate) fn delete(
    sessions: &SessionStore,
    data: &Path,
    workspaces: &Path,
    owner: &str,
    session: &Session,
) -> Result<(), SessionError> {
    let expected = binding(data, workspaces, owner, session);
    let mut retained = crate::assistant::read_json::<Binding>(&path(data, &expected))
        .map_err(SessionError::Storage)?;
    if retained.as_ref().is_some_and(|value| value != &expected) {
        return Err(SessionError::Conflict(
            "Pilot resource binding changed; inspect deployment roots before deleting".into(),
        ));
    }
    // The caller holds the Pilot admission gate. Read Turn ownership before the
    // cleanup transaction takes the store mutex; reentering it would deadlock.
    let turns = sessions.list_turns(owner, &session.id)?;
    let turn_ids = turns.iter().map(|turn| turn.id.clone()).collect::<Vec<_>>();
    sessions.delete_with_cleanup(owner, &session.id, || {
        if retained.is_none()
            && (resource_present(&expected.workspace).map_err(SessionError::Storage)?
                || resource_present(&expected.goose).map_err(SessionError::Storage)?)
        {
            adopt_legacy(data, workspaces, owner, session, &expected, &turns)?;
            retained = crate::assistant::read_json::<Binding>(&path(data, &expected))
                .map_err(SessionError::Storage)?;
        }
        if retained.is_some() {
            // The fact remains outside both deleted roots, and the Session row
            // is removed last: a partial filesystem failure is retryable.
            let _lease = if resource_present(&expected.goose).map_err(SessionError::Storage)? {
                Some(
                    crate::goose_acp::pilot::execution_lease(&expected.goose)
                        .map_err(SessionError::Conflict)?,
                )
            } else {
                None
            };
            remove_directory(&expected.workspace).map_err(SessionError::Storage)?;
            remove_directory(&expected.goose).map_err(SessionError::Storage)?;
            crate::plugin_mounts::cleanup_snapshots(data, &session.id, &turn_ids)
                .map_err(|error| SessionError::Storage(error.to_string()))?;
            std::fs::remove_file(path(data, &expected)).map_err(|error| {
                SessionError::Storage(format!("Pilot resource binding cleanup failed: {error}"))
            })?;
        }
        Ok(())
    })
}
