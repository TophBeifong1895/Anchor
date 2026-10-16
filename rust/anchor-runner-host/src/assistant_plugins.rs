//! Host authorization receipts and immutable Plugin policy for assistant Turns.
//!
//! Graph remains the desired configuration. These secret-free facts prove an
//! operator authorized its grants and what a particular Turn actually received.
use crate::application::RunMetadata;
use anchor_graph_host::{FilePluginCatalog, PluginCatalog};
use anchor_runtime::graph::{
    FileRunStore, GraphError, GraphRunRecord, GraphSnapshot, PluginBinding, PluginRef, RunLease,
    RunStore,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

fn hash(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

/// Shared with Graph publication/admission: reading desired policy and freezing
/// its grants is serialized with saving the Graph, never with model execution.
pub(crate) fn graph_lease(root: &Path, source: &Path) -> Result<Box<dyn RunLease>, GraphError> {
    let identity = crate::application::RunApplication::graph_identity(source).map_err(|error| {
        GraphError::Unsupported(format!("Graph identity unavailable: {error:?}"))
    })?;
    FileRunStore::new(root.join("admission-locks")).acquire_lease(&format!(
        "graph-{}",
        hash(identity.as_os_str().as_encoded_bytes())
    ))
}

/// Only the work node's Plugin selection and its Agent's network flag are hot.
pub(crate) fn compatible(admitted: &GraphSnapshot, current: &GraphSnapshot, work: &str) -> bool {
    let normalize = |snapshot: &GraphSnapshot| {
        let mut snapshot = snapshot.clone();
        if let Some(node) = snapshot.nodes.iter_mut().find(|node| node.id == work) {
            node.plugins.clear();
            if let Some(agent) = node
                .agent
                .as_ref()
                .and_then(|name| snapshot.agents.get_mut(name))
            {
                agent.network = false;
            }
        }
        snapshot
    };
    normalize(admitted) == normalize(current)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Publication {
    format: u32,
    source: PathBuf,
    pub(crate) graph_digest: String,
    pub(crate) bindings: BTreeMap<String, PluginBinding>,
}

fn publication_path(root: &Path, source: &Path, digest: &str) -> Result<PathBuf, String> {
    let source = crate::application::RunApplication::graph_identity(source)
        .map_err(|error| format!("Graph identity unavailable: {error:?}"))?;
    Ok(root
        .join("graph-plugin-authorizations")
        .join(hash(source.as_os_str().as_encoded_bytes()))
        .join(format!("{}.json", hash(digest.as_bytes()))))
}

/// Called only by the operator Graph save path while holding its Graph lease.
/// Agent-authored Graph updates must never call this authorization operation.
pub(crate) fn publish(
    root: &Path,
    source: &Path,
    snapshot: &GraphSnapshot,
    library_root: &Path,
) -> Result<(), String> {
    let _library = anchor_library::Library::new(library_root)
        .catalog_read_guard()
        .map_err(|error| error.to_string())?;
    let catalog = FilePluginCatalog::new(library_root);
    let ids = snapshot
        .nodes
        .iter()
        .flat_map(|node| node.plugins.iter().map(|plugin| plugin.id.clone()))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let mut bindings = catalog
        .resolve(&ids)
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|binding| (binding.id.clone(), binding))
        .collect::<BTreeMap<_, _>>();
    // A dependency-derived network grant is authorized with the same publication.
    if snapshot.agents.values().any(|agent| agent.network)
        && let Ok(mut network) = catalog.resolve(&[crate::plugin_network::NETWORK_PLUGIN.into()])
        && let Some(network) = network.pop()
    {
        bindings.insert(network.id.clone(), network);
    }
    let digest = snapshot.digest().map_err(|error| error.to_string())?;
    let publication = Publication {
        format: 1,
        source: crate::application::RunApplication::graph_identity(source)
            .map_err(|error| format!("Graph identity unavailable: {error:?}"))?,
        graph_digest: digest.clone(),
        bindings,
    };
    if let Some(existing) = self::publication(root, source, &digest)? {
        if existing.bindings.len() == publication.bindings.len()
            && existing.bindings.iter().all(|(id, frozen)| {
                publication
                    .bindings
                    .get(id)
                    .is_some_and(|current| frozen.same_authorization(current))
            })
        {
            return Ok(());
        }
        return Err("Graph Plugin Host authorization changed since approval; retire existing assistants and publish a new Graph revision".into());
    }
    crate::assistant::save_immutable(&publication_path(root, source, &digest)?, &publication)
}

pub(crate) fn publication(
    root: &Path,
    source: &Path,
    digest: &str,
) -> Result<Option<Publication>, String> {
    let saved: Option<Publication> =
        crate::assistant::read_json(&publication_path(root, source, digest)?)?;
    if let Some(saved) = &saved {
        let source = crate::application::RunApplication::graph_identity(source)
            .map_err(|error| format!("Graph identity unavailable: {error:?}"))?;
        if saved.format != 1
            || saved.source != source
            || saved.graph_digest != digest
            || saved
                .bindings
                .iter()
                .any(|(id, binding)| id != &binding.id || binding.authorization_digest.is_empty())
        {
            return Err("Graph Plugin authorization receipt is corrupt".into());
        }
    }
    Ok(saved)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TurnPluginPolicy {
    pub(crate) format: u32,
    pub(crate) run: String,
    pub(crate) node: String,
    pub(crate) graph_digest: String,
    pub(crate) mounts: crate::plugin_mounts::MountSnapshot,
    pub(crate) declarations: Vec<PluginRef>,
    pub(crate) bindings: BTreeMap<String, PluginBinding>,
    pub(crate) network: bool,
    pub(crate) reasons: Vec<String>,
    pub(crate) by_declaration: Vec<String>,
    pub(crate) by_switch: Vec<String>,
}

pub(crate) fn turn_path(root: &Path, run: &str, session: &str, turn: &str) -> PathBuf {
    root.join("assistant-plugin-turns")
        .join(hash(run.as_bytes()))
        .join(format!(
            "{}.json",
            hash(&serde_json::to_vec(&(session, turn)).expect("identity is serializable"))
        ))
}

fn marker_path(root: &Path, run: &str) -> PathBuf {
    root.join("assistant-plugin-turns")
        .join(hash(run.as_bytes()))
        .join("initialized.json")
}

pub(crate) fn initialized(root: &Path, run: &str) -> Result<bool, String> {
    match crate::assistant::read_json::<String>(&marker_path(root, run))? {
        Some(saved) if saved == run => Ok(true),
        Some(_) => Err("assistant Turn Plugin policy marker is corrupt".into()),
        None => Ok(false),
    }
}

pub(crate) fn mark_initialized(root: &Path, run: &str) -> Result<(), String> {
    crate::assistant::save_immutable(&marker_path(root, run), &run.to_owned())
}

pub(crate) fn load(
    root: &Path,
    run: &str,
    session: &str,
    turn: &str,
) -> Result<Option<TurnPluginPolicy>, String> {
    let saved: Option<TurnPluginPolicy> =
        crate::assistant::read_json(&turn_path(root, run, session, turn))
            .map_err(|error| format!("Turn Plugin policy unavailable: {error}"))?;
    if let Some(saved) = &saved
        && (saved.format != 1
            || !matches!(saved.mounts.format, 1 | 2)
            || saved.run != run
            || saved.mounts.session != session
            || saved.mounts.turn != turn
            || saved.node.is_empty()
            || saved.graph_digest.is_empty()
            || saved
                .bindings
                .iter()
                .any(|(id, binding)| id != &binding.id || binding.authorization_digest.is_empty()))
    {
        return Err("Turn Plugin policy is corrupt".into());
    }
    Ok(saved)
}

/// A verified Host-only fact selects invocation authority. Generic and legacy
/// Runs keep the Kernel's frozen admission policy.
pub(crate) fn invocation_authority(root: &Path, record: &GraphRunRecord) -> Result<bool, String> {
    let Some(metadata) = crate::application::metadata::load(root, &record.run_id)
        .map_err(|error| format!("Run identity unavailable: {error:?}"))?
    else {
        return Ok(false);
    };
    let Some(source) = metadata.assistant else {
        return Ok(false);
    };
    let plan = crate::assistant::AssistantPlan::from_snapshot(&record.snapshot, &source.work_node)?
        .ok_or("assistant Graph plan is missing")?;
    if metadata.graph_digest != record.graph_digest
        || source.owner.is_empty()
        || source.session.is_empty()
        || plan.wait_node != source.wait_node
        || plan.reply_node != source.reply_node
    {
        return Err("assistant authority identity changed".into());
    }
    Ok(true)
}

/// Only the cursor's still executable Turn holds references, never old history.
pub(crate) fn active_bindings(
    root: &Path,
    record: &GraphRunRecord,
    metadata: &RunMetadata,
) -> Result<BTreeMap<String, PluginBinding>, String> {
    let Some(source) = &metadata.assistant else {
        return Ok(BTreeMap::new());
    };
    let Some(cursor) = &record.cursor else {
        return Ok(BTreeMap::new());
    };
    let mut binding = crate::assistant::binding(root, &cursor.key)?;
    if binding.is_none() && cursor.node_id != source.wait_node {
        for commit in &cursor.input_commits {
            let key = anchor_runtime::graph::InvocationKey {
                node_id: commit.node_id.clone(),
                invocation: commit.invocation,
                ..cursor.key.clone()
            };
            if let Some(prior) = crate::assistant::binding(root, &key)? {
                binding = Some(prior);
                break;
            }
        }
    }
    let Some(binding) = binding else {
        if cursor.node_id != source.wait_node {
            return Err("assistant active invocation has no Turn binding".into());
        }
        return Ok(BTreeMap::new());
    };
    if let Some(policy) = load(root, &record.run_id, &source.session, &binding.turn)? {
        if policy.node != source.work_node {
            return Err("assistant Turn Plugin node identity changed".into());
        }
        return Ok(policy.bindings);
    }
    if initialized(root, &record.run_id)? {
        return Err("assistant Turn Plugin policy is missing".into());
    }
    // Only pre-upgrade active Turns lack a policy. Retain conservative baseline
    // references until that Turn finishes; idle assistants hold no such grants.
    let mut bindings = record.plugin_bindings.clone();
    let path = crate::plugin_mounts::snapshot_path(root, &source.session, &binding.turn);
    if let Some(snapshot) =
        crate::plugin_mounts::read_snapshot_strict(&path, &source.session, &binding.turn)?
    {
        for (id, enabled) in snapshot.mounts {
            if enabled {
                bindings.entry(id.clone()).or_insert(PluginBinding {
                    id,
                    digest: String::new(),
                    resources: vec![],
                    mcp_servers: vec![],
                    authorization_digest: String::new(),
                });
            }
        }
    }
    Ok(bindings)
}

/// Delete only parsed facts whose exact owner and hashed filename agree. Open
/// directory descriptors keep cleanup inside that owner even if a path changes.
fn remove_owned_facts(
    root: &Path,
    kind: &str,
    owner: &str,
    validate: impl Fn(&str, &[u8]) -> Result<(), String>,
) -> Result<(), String> {
    use rustix::fs::{AtFlags, Mode, OFlags, fsync, open, openat, unlinkat};
    use std::{io::Read, os::fd::AsRawFd};
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let parent = match open(root.join(kind), flags, Mode::empty()) {
        Ok(directory) => directory,
        Err(rustix::io::Errno::NOENT) => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    let directory = match openat(&parent, owner, flags, Mode::empty()) {
        Ok(directory) => directory,
        Err(rustix::io::Errno::NOENT) => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    let mut names = Vec::new();
    for entry in std::fs::read_dir(format!("/proc/self/fd/{}", directory.as_raw_fd()))
        .map_err(|error| error.to_string())?
    {
        let name = entry
            .map_err(|error| error.to_string())?
            .file_name()
            .into_string()
            .map_err(|_| "invalid policy fact filename")?;
        let fd = openat(
            &directory,
            name.as_str(),
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| error.to_string())?;
        let mut file = std::fs::File::from(fd);
        let metadata = file.metadata().map_err(|error| error.to_string())?;
        if !metadata.is_file() || metadata.len() > 32 * 1024 * 1024 {
            return Err("invalid policy cleanup fact".into());
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        validate(&name, &bytes)?;
        names.push(name);
    }
    for name in names {
        unlinkat(&directory, name.as_str(), AtFlags::empty()).map_err(|error| error.to_string())?;
    }
    fsync(&directory).map_err(|error| error.to_string())?;
    unlinkat(&parent, owner, AtFlags::REMOVEDIR).map_err(|error| error.to_string())?;
    fsync(&parent).map_err(|error| error.to_string())
}

pub(crate) fn cleanup(root: &Path, record: &GraphRunRecord) -> Result<(), String> {
    let metadata = crate::application::metadata::load(root, &record.run_id)
        .map_err(|error| format!("policy cleanup identity unavailable: {error:?}"))?;
    remove_owned_facts(
        root,
        "assistant-plugin-turns",
        &hash(record.run_id.as_bytes()),
        |name, bytes| {
            if name == "initialized.json" {
                let owner: String =
                    serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
                if owner == record.run_id {
                    return Ok(());
                }
                return Err("policy cleanup marker owner changed".into());
            }
            let policy: TurnPluginPolicy =
                serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
            let source = metadata
                .as_ref()
                .and_then(|metadata| metadata.assistant.as_ref())
                .ok_or("policy cleanup has no assistant owner")?;
            let expected = turn_path(root, &record.run_id, &source.session, &policy.mounts.turn);
            if policy.format != 1
                || policy.run != record.run_id
                || policy.node != source.work_node
                || policy.mounts.session != source.session
                || expected.file_name().and_then(|name| name.to_str()) != Some(name)
            {
                return Err("policy cleanup owner changed".into());
            }
            Ok(())
        },
    )
}

pub(crate) fn cleanup_publications(root: &Path, source: &Path) -> Result<(), String> {
    let identity = crate::application::RunApplication::graph_identity(source)
        .map_err(|error| format!("Graph cleanup identity unavailable: {error:?}"))?;
    remove_owned_facts(
        root,
        "graph-plugin-authorizations",
        &hash(identity.as_os_str().as_encoded_bytes()),
        |name, bytes| {
            let publication: Publication =
                serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
            if publication.format != 1
                || publication.source != identity
                || name != format!("{}.json", hash(publication.graph_digest.as_bytes()))
            {
                return Err("Graph authorization cleanup owner changed".into());
            }
            Ok(())
        },
    )
}

#[derive(Debug, Serialize)]
pub(crate) struct PluginUpdateStatus {
    pub(crate) state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) message: Option<String>,
}

/// Current claimed Turn, or the last committed Turn while the assistant waits.
/// This is a display projection of execution facts, never an authorization source.
pub(crate) fn latest_policy(
    root: &Path,
    record: &GraphRunRecord,
    metadata: &RunMetadata,
) -> Result<Option<TurnPluginPolicy>, String> {
    let source = metadata
        .assistant
        .as_ref()
        .ok_or("Run is not an assistant")?;
    let last = record
        .results
        .values()
        .flatten()
        .max_by_key(|result| result.sequence);
    let cursor_binding = record
        .cursor
        .as_ref()
        .map(|cursor| crate::assistant::binding(root, &cursor.key))
        .transpose()?
        .flatten();
    let binding = match cursor_binding {
        Some(binding) => Some(binding),
        None => last
            .map(|result| crate::assistant::binding(root, &result.key))
            .transpose()?
            .flatten(),
    };
    let policy = binding
        .map(|binding| load(root, &record.run_id, &source.session, &binding.turn))
        .transpose()?
        .flatten();
    if policy
        .as_ref()
        .is_some_and(|policy| policy.node != source.work_node)
    {
        return Err("assistant Turn Plugin node identity changed".into());
    }
    Ok(policy)
}

pub(crate) fn status(
    root: &Path,
    record: &GraphRunRecord,
    metadata: &RunMetadata,
    current: &GraphSnapshot,
) -> Result<PluginUpdateStatus, String> {
    let source = metadata
        .assistant
        .as_ref()
        .ok_or("Run is not an assistant")?;
    if !compatible(&record.snapshot, current, &source.work_node) {
        return Ok(PluginUpdateStatus {
            state: "restart_required",
            message: Some(
                "Graph structure changed; stop and retire the assistant to apply it".into(),
            ),
        });
    }
    let desired = current.digest().map_err(|error| error.to_string())?;
    if desired != record.graph_digest
        && publication(root, &metadata.bundle_source, &desired)?.is_none()
    {
        return Ok(PluginUpdateStatus {
            state: "unavailable",
            message: Some("Graph Plugin permissions have no operator authorization".into()),
        });
    }
    let effective = latest_policy(root, record, metadata)?
        .map(|policy| policy.graph_digest)
        .unwrap_or_else(|| record.graph_digest.clone());
    Ok(PluginUpdateStatus {
        state: if effective == desired {
            "current"
        } else {
            "pending"
        },
        message: None,
    })
}

#[cfg(test)]
mod tests;
