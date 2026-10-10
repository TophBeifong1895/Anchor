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
    /// How many records retention has physically removed from this family.
    /// Deleting a durable record without reporting it would be a silent loss.
    pub(crate) pruned: usize,
}

pub(crate) fn list_json_records<T: serde::de::DeserializeOwned>(
    data_root: &std::path::Path,
    directory: &str,
) -> Result<RecordList<T>, ApplicationError> {
    let pruned = pruned_count(data_root, directory);
    let directory = data_root.join(directory);
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RecordList {
                records: Vec::new(),
                unreadable: 0,
                truncated: false,
                pruned,
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
        pruned,
    })
}

/// A request nobody can answer any more, as startup reconciliation records it.
pub(crate) const ABANDONED_REQUEST_REASON: &str =
    "the asking Turn was no longer running, so nobody could answer this request";

/// How many host records one read model returns. Bounding happens *after*
/// sorting, so the subset a caller sees is deterministic, and the caller reports
/// that it bounded.
pub(crate) const ENVIRONMENT_RECORD_LIMIT: usize = 200;

/// The creation time already recorded at this path, if the file is readable.
fn read_created(path: &std::path::Path) -> Option<u64> {
    let bytes = std::fs::read(path).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    value.get("created").and_then(serde_json::Value::as_u64)
}

/// One record, as retention needs to see it. The reader decides which fields of
/// whichever family this is; retention only ever compares these.
pub(crate) struct Retention {
    /// The deployment name the attempt was about.
    pub(crate) name: String,
    /// The identity of the *candidate* the attempt was about: the content digest
    /// for a publication, the lock digest for a build. Two attempts at the same
    /// candidate share it even though their asking Turns differ, which is what
    /// makes an older outcome redundant.
    pub(crate) identity: String,
    /// When the attempt was first recorded; the newest outcome is the one kept.
    pub(crate) created: u64,
    /// An ask a human may still answer. Never removed.
    pub(crate) pending: bool,
    /// The published identity this record points at, when the act completed; the
    /// name may resolve to it right now. Never removed while it is the newest.
    pub(crate) published: Option<String>,
}

struct Recorded {
    path: PathBuf,
    retention: Retention,
}

/// The file one family's deletion counter lives in.
///
/// It is one **cumulative tally per family** — not this round's deletions, and it
/// only ever grows — because the read models answer "how much of this family has
/// been dropped", which a per-write number cannot.
///
/// Deliberately *not* named `.json`: the tolerant readers here only ever look at
/// `.json` files, so a tally can never be read back as one of the records it
/// counts.
fn pruned_counter_path(data_root: &std::path::Path, directory: &str) -> PathBuf {
    data_root.join(directory).join("pruned.count")
}

/// How many records this family has physically removed so far, as the read
/// models report it. It is a running total for the whole family and never
/// decreases. A tally this host cannot read counts as none: it says how much was
/// dropped, it is not itself a fact about an approval.
pub(crate) fn pruned_count(data_root: &std::path::Path, directory: &str) -> usize {
    std::fs::read(pruned_counter_path(data_root, directory))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<usize>(&bytes).ok())
        .unwrap_or(0)
}

/// Drop the terminal records that are provably redundant, and count every
/// deletion.
///
/// A terminal record is redundant only when a *newer* terminal state exists for
/// the same name and the same candidate identity (§D1B): the older outcome can no
/// longer be what the name resolves through. Three kinds are never removed:
///
/// - a `Requested` record, because a human may still answer it;
/// - a record this host cannot read, because retention must not delete what it
///   does not understand;
/// - the newest completed publication of that candidate, because that record is
///   the only proof of what the name may point at right now.
///
/// That last one is deliberately conservative rather than a name-table lookup:
/// retention only has the records here, and it does not need the name table.
/// Every path that moves a name writes a record for the id it moved it to, so
/// within one `(name, candidate identity)` group the *newest* success record is
/// the only one the name can currently resolve through; an older success record
/// of the same group can only become current by a new act, which writes its own
/// newer record first. Keeping it therefore *is* I3's protection.
///
/// Returns how many files were removed; the running total is recorded in the
/// family's counter so the read models can report `pruned`.
pub(crate) fn prune_environment_records(
    data_root: &std::path::Path,
    directory: &str,
    reader: impl Fn(&[u8]) -> Option<Retention>,
) -> Result<usize, ApplicationError> {
    let path = data_root.join(directory);
    let entries = match std::fs::read_dir(&path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(ApplicationError::Storage(error.to_string())),
    };
    let mut by_candidate: std::collections::BTreeMap<(String, String), Vec<Recorded>> =
        std::collections::BTreeMap::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Some(retention) = reader(&bytes) else {
            continue;
        };
        by_candidate
            .entry((retention.name.clone(), retention.identity.clone()))
            .or_default()
            .push(Recorded { path, retention });
    }
    let mut removed = 0usize;
    for (_, mut records) in by_candidate {
        // Newest first; ties by path so the choice is deterministic.
        records.sort_by(|left, right| {
            (right.retention.created, &right.path).cmp(&(left.retention.created, &left.path))
        });
        let mut kept_outcome = false;
        let mut kept_publication = false;
        for record in records {
            if record.retention.pending {
                continue;
            }
            let publication = record.retention.published.is_some();
            let keep = !kept_outcome || (publication && !kept_publication);
            if keep {
                kept_outcome = true;
                kept_publication |= publication;
                continue;
            }
            match std::fs::remove_file(&record.path) {
                Ok(()) => removed += 1,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(ApplicationError::Storage(format!(
                        "environment record {} could not be pruned: {error}",
                        record.retention.name
                    )));
                }
            }
        }
    }
    if removed > 0 {
        let total = pruned_count(data_root, directory) + removed;
        write_json_durable(&pruned_counter_path(data_root, directory), &total)?;
    }
    Ok(removed)
}

/// Rewrite every record a policy decides to replace, in place.
///
/// Startup reconciliation uses this to settle requests nobody can answer any
/// more. Only what this host can read is rewritten: a record it cannot parse is
/// left exactly as it is, never dropped.
pub(crate) fn settle_json_records<T>(
    data_root: &std::path::Path,
    directory: &str,
    settle: impl Fn(T) -> Result<Option<T>, ApplicationError>,
) -> Result<usize, ApplicationError>
where
    T: serde::de::DeserializeOwned + Serialize,
{
    let path = data_root.join(directory);
    let entries = match std::fs::read_dir(&path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(ApplicationError::Storage(error.to_string())),
    };
    let mut settled = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Ok(record) = serde_json::from_slice::<T>(&bytes) else {
            continue;
        };
        if let Some(record) = settle(record)? {
            write_json_durable(&path, &record)?;
            settled += 1;
        }
    }
    Ok(settled)
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
