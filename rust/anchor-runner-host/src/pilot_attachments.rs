//! Pilot Turn-owned references to immutable Host attachments. No Run is invented.
use crate::attachments::{self, AttachmentManifest, PreparedAttachments};
use anchor_platform_session::{Session, SessionStore, Turn, TurnStatus};
use anchor_runtime::ReadOnlyInput;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::Mutex,
};

static PUBLICATION_GATE: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Attachment {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) size: u64,
    pub(crate) sha256: String,
    pub(crate) media_type: Option<String>,
    pub(crate) url: String,
    pub(crate) download_url: String,
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Frozen {
    format: u32,
    owner: String,
    session: String,
    session_created: String,
    turn: String,
    turn_created: String,
    direction: String,
    source_turn: Option<String>,
    files_turn: Option<String>,
    files_created: Option<String>,
    attachments: Vec<AttachmentManifest>,
}

/// Already verified before the SessionStore admission transaction begins.
/// IDs and timestamps grant a reference, never a caller supplied Host path.
#[derive(Clone)]
pub(crate) struct InputSource {
    pub(crate) turn: String,
    files_turn: String,
    files_created: String,
    references: Vec<Attachment>,
}

fn directory(data: &Path, owner: &str, session: &Session, turn: &Turn) -> Result<PathBuf, String> {
    if turn.session != session.id || !anchor_runtime::identifier::is_name(&turn.id) {
        return Err("Pilot attachment Turn identity changed".into());
    }
    Ok(crate::pilot_host::scope(data, owner, session)
        .join("attachments")
        .join(&turn.id))
}

fn frozen(
    owner: &str,
    session: &Session,
    turn: &Turn,
    direction: &str,
    source: Option<&InputSource>,
    manifests: Vec<AttachmentManifest>,
) -> Frozen {
    Frozen {
        format: 1,
        owner: owner.into(),
        session: session.id.clone(),
        session_created: session.created_at.to_rfc3339(),
        turn: turn.id.clone(),
        turn_created: turn.created_at.to_rfc3339(),
        direction: direction.into(),
        source_turn: source.map(|source| source.turn.clone()),
        files_turn: source.map(|source| source.files_turn.clone()),
        files_created: source.map(|source| source.files_created.clone()),
        attachments: manifests,
    }
}

pub(crate) fn url_component(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

fn descriptor(session: &str, turn: &str, id: String, manifest: &AttachmentManifest) -> Attachment {
    let session = url_component(session);
    let turn = url_component(turn);
    let url = format!("/sessions/{session}/turns/{turn}/attachments/{id}");
    Attachment {
        id,
        name: manifest.name.clone(),
        size: manifest.size,
        sha256: manifest.sha256.clone(),
        media_type: manifest.media_type.clone(),
        download_url: format!("{url}?download=1"),
        url,
    }
}

pub(crate) fn identity(prepared: &PreparedAttachments, source: Option<&InputSource>) -> Value {
    record_identity(
        &prepared.manifest(),
        source.map(|source| source.turn.as_str()),
        source.map(|source| source.files_turn.as_str()),
        source.map(|source| source.files_created.as_str()),
    )
}
fn record_identity(
    manifests: &[AttachmentManifest],
    source: Option<&str>,
    files_turn: Option<&str>,
    files_created: Option<&str>,
) -> Value {
    if manifests.is_empty() && source.is_none() {
        json!([])
    } else {
        json!({"attachments":manifests,"source_turn":source,"files_turn":files_turn,"files_created":files_created})
    }
}

pub(crate) fn freeze_inputs(
    data: &Path,
    owner: &str,
    session: &Session,
    turn: &Turn,
    source: Option<&InputSource>,
    prepared: &PreparedAttachments,
) -> Result<Value, String> {
    if prepared.0.is_empty() {
        return Ok(json!([]));
    }
    let record = frozen(owner, session, turn, "input", source, prepared.manifest());
    let target = directory(data, owner, session, turn)?.join("input");
    let empty = PreparedAttachments::default();
    attachments::freeze_directory(
        &target,
        "attachment.json",
        &serde_json::to_vec(&record).map_err(|e| e.to_string())?,
        if source.is_some() { &empty } else { prepared },
    )?;
    if let Some(source) = source {
        return Ok(json!(source.references));
    }
    for manifest in &record.attachments {
        attachments::read_bytes(&target.join("files"), manifest)?;
    }
    Ok(json!(
        record
            .attachments
            .iter()
            .enumerate()
            .map(|(index, item)| descriptor(&session.id, &turn.id, format!("input-{index}"), item))
            .collect::<Vec<_>>()
    ))
}

fn load_record(
    target: &Path,
    owner: &str,
    session: &Session,
    turn: &Turn,
    direction: &str,
) -> Result<Frozen, String> {
    let mut bytes = Vec::new();
    crate::resource_read::open_resource(target, "attachment.json")
        .map_err(|e| e.to_string())?
        .take(64 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 64 * 1024 {
        return Err("Pilot attachment metadata exceeds its limit".into());
    }
    let record: Frozen =
        serde_json::from_slice(&bytes).map_err(|_| "Pilot attachment metadata is unavailable")?;
    if record.format != 1
        || record.owner != owner
        || record.session != session.id
        || record.session_created != session.created_at.to_rfc3339()
        || record.turn != turn.id
        || record.turn_created != turn.created_at.to_rfc3339()
        || record.direction != direction
        || record.source_turn.is_some() != record.files_turn.is_some()
        || record.files_turn.is_some() != record.files_created.is_some()
    {
        return Err("Pilot attachment ownership changed".into());
    }
    let mut sizes = attachments::Sizes::default();
    for item in &record.attachments {
        sizes.add(item.size, item.media_type.as_deref())?;
    }
    Ok(record)
}

pub(crate) fn input_references(
    sessions: &SessionStore,
    owner: &str,
    session: &str,
    turn: &str,
) -> Result<Vec<Attachment>, String> {
    let events = sessions
        .turn_events(owner, session, turn, 0)
        .map_err(|e| e.to_string())?;
    let Some(event) = events
        .into_iter()
        .find(|event| event.data["type"] == "input-attachments")
    else {
        return Ok(Vec::new());
    };
    serde_json::from_value(event.data["attachments"].clone())
        .map_err(|_| "Pilot input references are malformed".into())
}

fn checked_inputs(
    data: &Path,
    sessions: &SessionStore,
    owner: &str,
    session: &Session,
    turn: &Turn,
) -> Result<Option<(Frozen, Vec<Attachment>, PathBuf)>, String> {
    let events = sessions
        .turn_events(owner, &session.id, &turn.id, 0)
        .map_err(|e| e.to_string())?;
    let Some(event) = events
        .into_iter()
        .find(|event| event.data["type"] == "input-attachments")
    else {
        return Ok(None);
    };
    let refs: Vec<Attachment> = serde_json::from_value(event.data["attachments"].clone())
        .map_err(|_| "Pilot input references are malformed")?;
    if refs.is_empty() {
        return Ok(None);
    };
    let target = directory(data, owner, session, turn)?.join("input");
    let record = load_record(&target, owner, session, turn, "input")?;
    if event.data["manifest"]
        != record_identity(
            &record.attachments,
            record.source_turn.as_deref(),
            record.files_turn.as_deref(),
            record.files_created.as_deref(),
        )
    {
        return Err("Pilot input source identity changed".into());
    }
    let files_turn = record.files_turn.as_deref().unwrap_or(&turn.id);
    let expected = record
        .attachments
        .iter()
        .enumerate()
        .map(|(index, item)| descriptor(&session.id, files_turn, format!("input-{index}"), item))
        .collect::<Vec<_>>();
    if refs != expected {
        return Err("Pilot input references differ from frozen bytes".into());
    }
    let files = if let Some(origin) = record.files_turn.as_deref() {
        let original = sessions
            .get_turn(owner, &session.id, origin)
            .map_err(|e| e.to_string())?;
        if record.files_created.as_deref() != Some(original.created_at.to_rfc3339().as_str()) {
            return Err("Pilot input origin identity changed".into());
        }
        let origin_target = directory(data, owner, session, &original)?.join("input");
        let original_record = load_record(&origin_target, owner, session, &original, "input")?;
        if original_record.source_turn.is_some()
            || original_record.attachments != record.attachments
        {
            return Err("Pilot input origin differs from the authorized source".into());
        }
        origin_target.join("files")
    } else {
        target.join("files")
    };
    Ok(Some((record, refs, files)))
}

pub(crate) fn source_inputs(
    data: &Path,
    sessions: &SessionStore,
    owner: &str,
    session: &Session,
    source: &Turn,
) -> Result<(PreparedAttachments, Option<InputSource>), String> {
    let Some((record, refs, files)) = checked_inputs(data, sessions, owner, session, source)?
    else {
        return Ok((PreparedAttachments::default(), None));
    };
    let prepared = PreparedAttachments(
        record
            .attachments
            .iter()
            .map(|item| attachments::read_bytes(&files, item).map(|bytes| (item.clone(), bytes)))
            .collect::<Result<_, _>>()?,
    );
    let source = InputSource {
        turn: source.id.clone(),
        files_turn: record.files_turn.unwrap_or_else(|| source.id.clone()),
        files_created: record
            .files_created
            .unwrap_or_else(|| source.created_at.to_rfc3339()),
        references: refs,
    };
    Ok((prepared, Some(source)))
}

pub(crate) fn prepared_inputs(
    data: &Path,
    sessions: &SessionStore,
    owner: &str,
    session: &Session,
    turn: &Turn,
) -> Result<PreparedAttachments, String> {
    let Some((record, _, files)) = checked_inputs(data, sessions, owner, session, turn)? else {
        return Ok(PreparedAttachments::default());
    };
    Ok(PreparedAttachments(
        record
            .attachments
            .into_iter()
            .map(|item| attachments::read_bytes(&files, &item).map(|bytes| (item, bytes)))
            .collect::<Result<_, _>>()?,
    ))
}

pub(crate) fn readonly_inputs(
    data: &Path,
    sessions: &SessionStore,
    owner: &str,
    session: &Session,
    turn: &Turn,
) -> Result<Vec<ReadOnlyInput>, String> {
    let Some((record, _, files)) = checked_inputs(data, sessions, owner, session, turn)? else {
        return Ok(Vec::new());
    };
    for item in &record.attachments {
        attachments::read_bytes(&files, item)?;
    }
    Ok(vec![ReadOnlyInput {
        source: files,
        destination: "/in/pilot".into(),
    }])
}

pub(crate) fn project_turn(
    sessions: &SessionStore,
    owner: &str,
    turn: &Turn,
) -> Result<Value, String> {
    let mut value = json!(turn);
    let refs = input_references(sessions, owner, &turn.session, &turn.id)?;
    if !refs.is_empty() {
        value["attachments"] = json!(refs);
    }
    Ok(value)
}

pub(crate) fn read(
    data: &Path,
    sessions: &SessionStore,
    owner: &str,
    session: &Session,
    turn: &Turn,
    id: &str,
) -> Result<(Attachment, Vec<u8>), String> {
    let refs = input_references(sessions, owner, &session.id, &turn.id)?;
    let (reference, direction, target) =
        if let Some(reference) = refs.into_iter().find(|item| item.id == id) {
            (
                reference,
                "input",
                directory(data, owner, session, turn)?.join("input"),
            )
        } else {
            let events = sessions
                .turn_events(owner, &session.id, &turn.id, 0)
                .map_err(|e| e.to_string())?;
            let reference = events
                .into_iter()
                .filter(|event| event.data["type"] == "attachment")
                .filter_map(|event| {
                    serde_json::from_value::<Attachment>(event.data["attachment"].clone()).ok()
                })
                .find(|item| item.id == id)
                .ok_or("Pilot attachment not found")?;
            if !anchor_runtime::identifier::is_name(id) {
                return Err("invalid Pilot attachment id".into());
            }
            (
                reference,
                "output",
                directory(data, owner, session, turn)?
                    .join("output")
                    .join(id),
            )
        };
    if direction == "input" {
        let (record, _, files) = checked_inputs(data, sessions, owner, session, turn)?
            .ok_or("Pilot input attachment not found")?;
        let owner_turn = record.files_turn.as_deref().unwrap_or(&turn.id);
        let item = record
            .attachments
            .iter()
            .find(|item| descriptor(&session.id, owner_turn, id.into(), item) == reference)
            .ok_or("Pilot attachment reference changed")?;
        return Ok((reference, attachments::read_bytes(&files, item)?));
    }
    let record = load_record(&target, owner, session, turn, direction)?;
    let item = record
        .attachments
        .iter()
        .find(|item| descriptor(&session.id, &turn.id, id.into(), item) == reference)
        .ok_or("Pilot attachment reference changed")?;
    let bytes = attachments::read_bytes(&target.join("files"), item)?;
    Ok((reference, bytes))
}

pub(crate) fn publish(
    data: &Path,
    sessions: &SessionStore,
    owner: &str,
    session_id: &str,
    turn_id: &str,
    prepared: PreparedAttachments,
) -> Result<Attachment, String> {
    let _guard = PUBLICATION_GATE
        .lock()
        .map_err(|_| "Pilot attachment publication unavailable")?;
    let session = sessions.get(owner, session_id).map_err(|e| e.to_string())?;
    let turn = sessions
        .get_turn(owner, session_id, turn_id)
        .map_err(|e| e.to_string())?;
    if turn.status != TurnStatus::Running
        || !session.graph.is_empty()
        || !session.channel.is_empty()
    {
        return Err("Pilot Turn is not active".into());
    }
    let manifest = prepared
        .manifest()
        .into_iter()
        .next()
        .ok_or("publish_file requires a file")?;
    if prepared.0.len() != 1 {
        return Err("publish_file requires exactly one file".into());
    }
    let id = format!(
        "output-{:x}",
        Sha256::digest(serde_json::to_vec(&manifest).map_err(|e| e.to_string())?)
    );
    let reference = descriptor(session_id, turn_id, id.clone(), &manifest);
    let mut sizes = attachments::Sizes::default();
    for event in sessions
        .turn_events(owner, session_id, turn_id, 0)
        .map_err(|e| e.to_string())?
    {
        if event.data["type"] != "attachment" {
            continue;
        }
        let retained: Attachment = serde_json::from_value(event.data["attachment"].clone())
            .map_err(|_| "Pilot output reference changed")?;
        if retained.id == id {
            read(data, sessions, owner, &session, &turn, &id)?;
            return Ok(retained);
        }
        sizes.add(retained.size, retained.media_type.as_deref())?;
    }
    sizes.add(manifest.size, manifest.media_type.as_deref())?;
    let target = directory(data, owner, &session, &turn)?
        .join("output")
        .join(&id);
    let record = frozen(
        owner,
        &session,
        &turn,
        "output",
        None,
        vec![manifest.clone()],
    );
    if target.exists() {
        if load_record(&target, owner, &session, &turn, "output")? != record {
            return Err("Pilot publication identity changed".into());
        }
        attachments::read_bytes(&target.join("files"), &manifest)?;
    } else {
        attachments::freeze_directory(
            &target,
            "attachment.json",
            &serde_json::to_vec(&record).map_err(|e| e.to_string())?,
            &prepared,
        )?;
    }
    sessions
        .append_turn_event(
            owner,
            session_id,
            turn_id,
            json!({"type":"attachment","attachment":reference}),
        )
        .map_err(|e| e.to_string())?;
    Ok(reference)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn publish_workspace_file(
    data: &Path,
    sessions: &SessionStore,
    owner: &str,
    session: &str,
    turn: &str,
    workspace: &Path,
    path: &str,
    name: Option<&str>,
) -> Result<Attachment, String> {
    let mut file = crate::resource_read::open_resource(workspace, path).map_err(
        |_| "published file must be a regular non-symlink file inside the Pilot workspace",
    )?;
    if file.metadata().map_err(|e| e.to_string())?.len() > attachments::MAX_FILE_BYTES as u64 {
        return Err("published file exceeds the 20 MiB file limit".into());
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(attachments::MAX_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let name = name
        .map(str::to_owned)
        .or_else(|| {
            Path::new(path)
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
        })
        .ok_or("published file name unavailable")?;
    publish(
        data,
        sessions,
        owner,
        session,
        turn,
        attachments::from_bytes(name, bytes, None)?,
    )
}

#[cfg(test)]
mod tests;
