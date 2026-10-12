//! Minimal Graph ownership that outlives deleted Run metadata. Native scope
//! leases and surviving Run/hand-off checks remain the cleanup authorities.

use crate::application::{ApplicationError, RunApplication, RunMetadata};
use anchor_runtime::graph::{FileRunStore, GraphRunRecord, RunStore};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Ownership {
    format: u32,
    graph: PathBuf,
    session: String,
    node: String,
    hint: String,
}

pub(crate) fn retain(
    data: &Path,
    record: &GraphRunRecord,
    metadata: &RunMetadata,
) -> Result<(), ApplicationError> {
    let Some(conversation) = &metadata.conversation else {
        return Ok(());
    };
    let graph = RunApplication::graph_identity(&metadata.bundle_source)?;
    for node in &record.snapshot.nodes {
        if let Some(hint) = crate::node_host::conversation_hint_for(metadata, &node.id) {
            let ownership = Ownership {
                format: 1,
                graph: graph.clone(),
                session: conversation.session.clone(),
                node: node.id.clone(),
                hint: hint.key,
            };
            crate::assistant::save_immutable(
                &data
                    .join("conversation-resources")
                    .join(format!("{}.json", ownership.hint)),
                &ownership,
            )
            .map_err(ApplicationError::Storage)?;
        }
    }
    Ok(())
}

fn ownerships(data: &Path, graph: &Path) -> Result<Vec<Ownership>, ApplicationError> {
    let directory = data.join("conversation-resources");
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(ApplicationError::Storage(error.to_string())),
    };
    let mut retained = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|error| ApplicationError::Storage(error.to_string()))?
            .path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let ownership = crate::assistant::read_json::<Ownership>(&path)
            .map_err(ApplicationError::Storage)?
            .ok_or_else(|| {
                ApplicationError::Storage("conversation ownership disappeared".into())
            })?;
        if ownership.format != 1
            || ownership.hint.len() != 64
            || !ownership
                .hint
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            || path.file_stem().and_then(|stem| stem.to_str()) != Some(&ownership.hint)
        {
            return Err(ApplicationError::Storage(
                "conversation ownership identity changed".into(),
            ));
        }
        if ownership.graph == graph {
            retained.push(ownership);
        }
    }
    Ok(retained)
}

pub(crate) fn for_graph(data: &Path, graph: &Path) -> Result<Vec<String>, ApplicationError> {
    Ok(ownerships(data, graph)?
        .into_iter()
        .map(|ownership| ownership.hint)
        .collect())
}

fn remove(data: &Path, workspaces: &Path, hint: &str) -> Result<(), ApplicationError> {
    crate::goose_acp::remove_conversation(&workspaces.join(".goose-process"), hint)
        .map_err(ApplicationError::Storage)?;
    crate::run_data::remove_legacy_conversation(data, hint).map_err(ApplicationError::Storage)
}

/// Called with Graph admission and active-execution gates held. A surviving
/// Run retains only the nodes it actually owns; a metadata-only hand-off keeps
/// all of its Session's scopes until its snapshot is known.
pub(crate) fn release_unreferenced(
    application: &RunApplication,
    data: &Path,
    graph: &Path,
    deleting: &str,
    workspaces: &Path,
) -> Result<(), ApplicationError> {
    let references = crate::application::metadata::ids(data)?
        .into_iter()
        .filter(|id| id != deleting)
        .map(|id| application.metadata(&id).map(|metadata| (id, metadata)))
        .collect::<Result<Vec<_>, _>>()?;
    let store = FileRunStore::new(data.join("runs"));
    for ownership in ownerships(data, graph)? {
        let mut referenced = false;
        for (id, metadata) in &references {
            let Some(metadata) = metadata else {
                continue;
            };
            if RunApplication::graph_identity(&metadata.bundle_source)? != ownership.graph
                || metadata
                    .conversation
                    .as_ref()
                    .is_none_or(|source| source.session != ownership.session)
            {
                continue;
            }
            let record = store.load(id)?;
            if record.as_ref().is_none_or(|record| {
                record
                    .snapshot
                    .nodes
                    .iter()
                    .any(|node| node.id == ownership.node)
            }) {
                referenced = true;
                break;
            }
        }
        if !referenced {
            remove(data, workspaces, &ownership.hint)?;
        }
    }
    Ok(())
}

/// Existing admission semantics start a fresh chain when its head was deleted.
/// Its native binding must also start fresh, rather than refer to deleted facts.
pub(crate) fn start_fresh(
    application: &RunApplication,
    data: &Path,
    graph: &Path,
    session: &str,
    workspaces: &Path,
) -> Result<(), ApplicationError> {
    let store = FileRunStore::new(data.join("runs"));
    for id in crate::application::metadata::ids(data)? {
        if let Some(metadata) = application.metadata(&id)?
            && RunApplication::graph_identity(&metadata.bundle_source)? == graph
            && metadata
                .conversation
                .as_ref()
                .is_some_and(|source| source.session == session)
            && store.load(&id)?.is_none()
        {
            return Err(ApplicationError::Conflict(format!(
                "Session has retained hand-off `{id}` without its Run"
            )));
        }
    }
    for ownership in ownerships(data, graph)? {
        if ownership.session == session {
            remove(data, workspaces, &ownership.hint)?;
        }
    }
    Ok(())
}
