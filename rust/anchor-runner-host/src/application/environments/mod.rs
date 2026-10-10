//! Dependency environments: the facts the host keeps about them.
//!
//! Three record families with one shape each:
//! - `plans` — publishing a prepared directory behind a native approval,
//! - `provision` — building one from a recorded requirement behind the same kind
//!   of approval,
//! - `requirements` — what the assistant needs an environment to provide,
//! - `references` — who uses a name right now (a derived view, never stored).
//!
//! Shared here: the requester identity, the durable JSON writer and the one
//! tolerant read policy every family uses.

use super::{ApplicationError, RunApplication};
use anchor_library::{EnvironmentError, EnvironmentStore};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

mod plans;
mod provision;
mod requirements;

pub(crate) use plans::{
    EnvironmentPlan, EnvironmentPlanStatus, EnvironmentPublishPrecondition, list_environment_plans,
};
pub(crate) use provision::{
    EnvironmentProvisionPlan, EnvironmentProvisionPrecondition, EnvironmentProvisionStatus,
    list_environment_provision_plans,
};
#[cfg(test)]
pub(crate) use requirements::EnvironmentRequirement;
pub(crate) use requirements::{
    EnvironmentRequirementAsk, EnvironmentRequirements, list_environment_requirements,
};

/// Who asked, and in which turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EnvironmentPlanRequester {
    pub(crate) owner: String,
    pub(crate) session: String,
    pub(crate) turn: String,
}

fn environment_error(context: &str, error: EnvironmentError) -> ApplicationError {
    match error {
        EnvironmentError::VersionConflict { .. } => {
            ApplicationError::Conflict(format!("{context}: {error}"))
        }
        EnvironmentError::Io(_) | EnvironmentError::PublicationUncertain(_) => {
            ApplicationError::Storage(format!("{context}: {error}"))
        }
        _ => ApplicationError::Invalid(format!("{context}: {error}")),
    }
}

/// Write one durable JSON record: durable directory chain, fsynced file and
/// directory, unique temporary name so two writers cannot rename over each other.
fn write_json_durable(
    path: &std::path::Path,
    value: &impl Serialize,
) -> Result<(), ApplicationError> {
    let directory = path
        .parent()
        .ok_or_else(|| ApplicationError::Storage("record path is invalid".into()))?;
    // The crate's helper fsyncs the chain that makes the directory reachable.
    crate::create_durable_directory(directory)
        .map_err(|error| ApplicationError::Storage(error.to_string()))?;
    let bytes =
        serde_json::to_vec(value).map_err(|error| ApplicationError::Storage(error.to_string()))?;
    let temporary = directory.join(format!(
        ".{}-{}.tmp",
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "record".to_owned()),
        crate::application::metadata::now_nanos()
    ));
    {
        use std::io::Write;
        let mut file = std::fs::File::create(&temporary)
            .map_err(|error| ApplicationError::Storage(error.to_string()))?;
        file.write_all(&bytes)
            .map_err(|error| ApplicationError::Storage(error.to_string()))?;
        file.sync_all()
            .map_err(|error| ApplicationError::Storage(error.to_string()))?;
    }
    std::fs::rename(&temporary, path)
        .map_err(|error| ApplicationError::Storage(error.to_string()))?;
    std::fs::File::open(directory)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| ApplicationError::Storage(error.to_string()))?;
    Ok(())
}

/// One tolerant read policy for every host record family: skip what this host
/// cannot read, count it, and never let one bad file hide the rest.
#[derive(Debug)]
pub(crate) struct RecordList<T> {
    pub(crate) records: Vec<T>,
    pub(crate) unreadable: usize,
    /// Set by the caller's projection when it bounded the list.
    pub(crate) truncated: bool,
}

pub(crate) fn list_json_records<T: serde::de::DeserializeOwned>(
    data_root: &std::path::Path,
    directory: &str,
) -> Result<RecordList<T>, ApplicationError> {
    let directory = data_root.join(directory);
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RecordList {
                records: Vec::new(),
                unreadable: 0,
                truncated: false,
            });
        }
        Err(error) => return Err(ApplicationError::Storage(error.to_string())),
    };
    let mut records = Vec::new();
    let mut unreadable = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        match std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<T>(&bytes).ok())
        {
            Some(record) => records.push(record),
            None => unreadable += 1,
        }
    }
    Ok(RecordList {
        records,
        unreadable,
        truncated: false,
    })
}

/// How many host records one read model returns. Bounding happens *after*
/// sorting, so the subset a caller sees is deterministic, and the caller reports
/// that it bounded.
pub(crate) const ENVIRONMENT_RECORD_LIMIT: usize = 200;

/// How many decision records one name keeps, counting pending requests.
///
/// A record is written per approval attempt, so a deployment that prepares
/// environments regularly would otherwise accumulate them without bound. A name
/// keeps its newest few terminal records plus *every* pending one, because a
/// pending record is a question a human may still answer; terminal records that
/// fall off are the oldest, and the read models keep reporting how many records
/// they could not read.
pub(crate) const ENVIRONMENT_RECORDS_PER_NAME: usize = 20;

/// The creation time already recorded at this path, if the file is readable.
fn read_created(path: &std::path::Path) -> Option<u64> {
    let bytes = std::fs::read(path).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    value.get("created").and_then(serde_json::Value::as_u64)
}

/// One record, as retention needs to see it.
struct Retained {
    path: PathBuf,
    name: String,
    created: u64,
    pending: bool,
}

/// Drop the oldest terminal records of each name beyond the bound.
///
/// Returns how many files were removed. A record this host cannot read is left
/// alone: retention must not delete what it does not understand, and a pending
/// (`Requested`) record is never dropped because a human may still answer it.
pub(crate) fn prune_environment_records(
    data_root: &std::path::Path,
    directory: &str,
    reader: impl Fn(&[u8]) -> Option<(String, u64, bool)>,
) -> Result<usize, ApplicationError> {
    let directory = data_root.join(directory);
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(ApplicationError::Storage(error.to_string())),
    };
    let mut by_name: std::collections::BTreeMap<String, Vec<Retained>> =
        std::collections::BTreeMap::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Some((name, created, pending)) = reader(&bytes) else {
            continue;
        };
        by_name.entry(name.clone()).or_default().push(Retained {
            path,
            name,
            created,
            pending,
        });
    }
    let mut removed = 0usize;
    for (_, mut records) in by_name {
        let pending = records.iter().filter(|record| record.pending).count();
        let keep = ENVIRONMENT_RECORDS_PER_NAME.saturating_sub(pending);
        // Newest first; ties by path so the choice is deterministic.
        records
            .sort_by(|left, right| (right.created, &right.path).cmp(&(left.created, &left.path)));
        let mut kept_terminal = 0usize;
        for record in records {
            if record.pending {
                continue;
            }
            if kept_terminal < keep {
                kept_terminal += 1;
                continue;
            }
            match std::fs::remove_file(&record.path) {
                Ok(()) => removed += 1,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(ApplicationError::Storage(format!(
                        "environment record {} could not be pruned: {error}",
                        record.name
                    )));
                }
            }
        }
    }
    Ok(removed)
}

/// Sort a projection and keep at most [`ENVIRONMENT_RECORD_LIMIT`] records.
/// Returns whether anything was left out, so a listing never implies it is whole.
pub(crate) fn bound_projection<T>(
    records: &mut Vec<T>,
    order: impl Fn(&T, &T) -> std::cmp::Ordering,
) -> bool {
    records.sort_by(order);
    let truncated = records.len() > ENVIRONMENT_RECORD_LIMIT;
    records.truncate(ENVIRONMENT_RECORD_LIMIT);
    truncated
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use std::fs;

    pub(crate) fn application(root: &std::path::Path) -> RunApplication {
        RunApplication::new(root.join("state"), root.to_path_buf())
    }

    pub(crate) fn candidate(root: &std::path::Path, marker: &str) -> PathBuf {
        let directory = root.join(format!("candidate-{marker}"));
        fs::create_dir_all(directory.join("bin")).unwrap();
        fs::write(directory.join("marker.txt"), marker).unwrap();
        fs::write(directory.join("bin/python"), "#!/bin/sh\n").unwrap();
        directory
    }

    pub(crate) fn requester() -> EnvironmentPlanRequester {
        EnvironmentPlanRequester {
            owner: "local".into(),
            session: "pilot".into(),
            turn: "turn-1".into(),
        }
    }

    /// Published trees are read-only; a test that prunes one needs the bits back.
    pub(crate) fn force_writable(root: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut stack = vec![root.to_path_buf()];
        while let Some(directory) = stack.pop() {
            let mut permissions = fs::metadata(&directory).unwrap().permissions();
            permissions.set_mode(permissions.mode() | 0o700);
            fs::set_permissions(&directory, permissions).unwrap();
            for entry in fs::read_dir(&directory).unwrap() {
                let entry = entry.unwrap();
                let metadata = entry.metadata().unwrap();
                if metadata.is_dir() {
                    stack.push(entry.path());
                } else {
                    let mut permissions = metadata.permissions();
                    permissions.set_mode(permissions.mode() | 0o600);
                    fs::set_permissions(entry.path(), permissions).unwrap();
                }
            }
        }
    }
}
