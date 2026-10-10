//! Dependency environments: the facts the host keeps about them.
//!
//! Three record families with one shape each:
//! - `plans` — publishing a prepared directory behind a native approval,
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
mod references;
mod requirements;

pub(crate) use plans::{
    EnvironmentPlan, EnvironmentPlanStatus, EnvironmentPublishPrecondition, list_environment_plans,
};
pub(crate) use references::{EnvironmentCoverage, EnvironmentReferences};
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
