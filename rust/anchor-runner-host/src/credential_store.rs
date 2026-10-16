//! Owner-bound credentials are Host configuration. Values never enter Pilot
//! history, question answers, workspace files, or public request metadata.
mod management;
pub(crate) use management::{configure, credentials};

use anchor_graph_host::{FilePluginCatalog, PluginDefinition};
use anchor_platform_session::{Session, SessionStore, TurnStatus};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CredentialRequest {
    pub(crate) id: String,
    pub(crate) session: String,
    pub(crate) turn: String,
    pub(crate) plugin: String,
    pub(crate) server: String,
    pub(crate) field: String,
    pub(crate) purpose: String,
    pub(crate) status: String,
    pub(crate) expires_at: i64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestFact {
    format: u32,
    owner: String,
    created_at: String,
    digest: String,
    authorization_digest: String,
    request: CredentialRequest,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Secret {
    format: u32,
    owner: String,
    plugin: String,
    server: String,
    field: String,
    authorization_digest: String,
    value: String,
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn requests_root(data: &Path, owner: &str, session: &Session) -> PathBuf {
    data.join("platform/credential-requests").join(
        crate::pilot_host::scope(data, owner, session)
            .file_name()
            .expect("scope hash"),
    )
}
fn secret_path(data: &Path, owner: &str, plugin: &str, server: &str, field: &str) -> PathBuf {
    data.join("platform/credentials")
        .join(hash(owner.as_bytes()))
        .join(plugin)
        .join(server)
        .join(format!("{field}.json"))
}

pub(crate) fn binding_owner(owner: &str) -> String {
    if owner == "local" {
        owner.into()
    } else {
        hash(owner.as_bytes())
    }
}
fn valid_field(field: &str) -> bool {
    !field.is_empty()
        && field.len() <= 200
        && field.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_uppercase() || byte == b'_' || (index > 0 && byte.is_ascii_digit())
        })
}

fn private_parent(path: &Path) -> Result<Option<rustix::fd::OwnedFd>, String> {
    use rustix::fs::{Mode, OFlags, fstat, open, openat};
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut directory =
        open("/", flags, Mode::empty()).map_err(|_| "private credential root unavailable")?;
    let parent = path
        .parent()
        .ok_or("private credential parent unavailable")?;
    if !parent.is_absolute() {
        return Err("private credential parent must be absolute".into());
    }
    let mut private_tree = false;
    for component in parent.components() {
        match component {
            std::path::Component::RootDir => {}
            std::path::Component::Normal(name) => {
                directory = match openat(&directory, name, flags, Mode::empty()) {
                    Ok(directory) => directory,
                    Err(error) if error == rustix::io::Errno::NOENT => return Ok(None),
                    Err(_) => {
                        return Err("private credential parent must not contain symlinks".into());
                    }
                };
                private_tree |= name == "credentials" || name == "credential-requests";
                if private_tree {
                    let metadata =
                        fstat(&directory).map_err(|_| "private credential parent unavailable")?;
                    if metadata.st_uid != rustix::process::geteuid().as_raw()
                        || metadata.st_mode & 0o777 != 0o700
                    {
                        return Err(
                            "private credential directories require Host ownership and mode 700"
                                .into(),
                        );
                    }
                }
            }
            _ => return Err("invalid private credential path".into()),
        }
    }
    Ok(Some(directory))
}
fn load<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>, String> {
    use rustix::fs::{Mode, OFlags, openat};
    let Some(directory) = private_parent(path)? else {
        return Ok(None);
    };
    let file: fs::File = match openat(
        &directory,
        path.file_name()
            .ok_or("private credential filename unavailable")?,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(file) => file.into(),
        Err(error) if error == rustix::io::Errno::NOENT => return Ok(None),
        Err(_) => return Err("private credential fact unavailable".into()),
    };
    let metadata = file
        .metadata()
        .map_err(|_| "private credential metadata unavailable")?;
    if !metadata.is_file() || metadata.len() > 128 * 1024 {
        return Err("private credential fact is not a bounded regular file".into());
    }
    if metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.permissions().mode() & 0o777 != 0o600
    {
        return Err("private credential facts require Host ownership and mode 600".into());
    }
    let mut bytes = Vec::new();
    file.take(128 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "private credential fact unavailable")?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| "private credential fact malformed".into())
}
fn save(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or("private credential parent unavailable")?;
    crate::goose_acp::pilot::directory(parent)?;
    private_parent(path)?.ok_or("private credential parent unavailable")?;
    let mut file = tempfile::NamedTempFile::new_in(parent)
        .map_err(|_| "private credential staging unavailable")?;
    fs::set_permissions(file.path(), fs::Permissions::from_mode(0o600))
        .map_err(|_| "private credential permissions unavailable")?;
    file.write_all(&serde_json::to_vec(value).map_err(|_| "private credential encoding failed")?)
        .and_then(|_| file.as_file().sync_all())
        .map_err(|_| "private credential persistence failed")?;
    file.persist(path)
        .map_err(|_| "private credential publication failed")?;
    fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| "private credential directory sync failed".into())
}
fn lease(data: &Path) -> Result<fs::File, String> {
    let root = data.join("platform");
    crate::goose_acp::pilot::directory(&root)?;
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32)
        .open(root.join("credentials.lock"))
        .map_err(|_| "credential lock unavailable")?;
    if !file
        .metadata()
        .map_err(|_| "credential lock unavailable")?
        .is_file()
    {
        return Err("credential lock is not a regular file".into());
    }
    let started = std::time::Instant::now();
    loop {
        match rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => break,
            Err(error)
                if error == rustix::io::Errno::WOULDBLOCK
                    && started.elapsed() < std::time::Duration::from_millis(200) =>
            {
                std::thread::sleep(std::time::Duration::from_millis(2))
            }
            Err(_) => return Err("credential state is busy; retry this operation".into()),
        }
    }
    Ok(file)
}

pub(crate) fn declared_fields(config: &Value) -> BTreeSet<String> {
    fn inspect(value: &Value, fields: &mut BTreeSet<String>) {
        match value {
            Value::String(text) => {
                let mut remaining = text.as_str();
                while let Some(index) = remaining.find("${") {
                    remaining = &remaining[index + 2..];
                    let Some(end) = remaining.find('}') else {
                        break;
                    };
                    let key = remaining[..end].split(":-").next().unwrap_or("");
                    if valid_field(key)
                        && !matches!(
                            key,
                            "PLUGIN_ROOT"
                                | "CODEX_PLUGIN_ROOT"
                                | "CLAUDE_PLUGIN_ROOT"
                                | "ANCHOR_PLUGIN_VIEW"
                        )
                    {
                        fields.insert(key.into());
                    }
                    remaining = &remaining[end + 1..];
                }
            }
            Value::Array(values) => {
                for value in values {
                    inspect(value, fields)
                }
            }
            Value::Object(values) => {
                for value in values.values() {
                    inspect(value, fields)
                }
            }
            _ => {}
        }
    }
    let mut fields = BTreeSet::new();
    inspect(config, &mut fields);
    for key in ["env_vars", "optional_env_vars"] {
        for value in config
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if valid_field(value) {
                fields.insert(value.into());
            }
        }
    }
    for value in config
        .get("env_http_headers")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|values| values.values())
        .filter_map(Value::as_str)
        .chain(config.get("bearer_token_env_var").and_then(Value::as_str))
    {
        if valid_field(value) {
            fields.insert(value.into());
        }
    }
    fields
}

pub(crate) fn target_authorization(
    definition: &PluginDefinition,
    server: &str,
) -> Result<(BTreeSet<String>, String), String> {
    if let Some(platform) = server.strip_prefix("channel_") {
        if definition
            .mcp_servers
            .iter()
            .any(|declaration| declaration.name == server)
        {
            return Err("credential channel target conflicts with an MCP server name".into());
        }
        let channel = definition
            .channels
            .iter()
            .find(|channel| channel.platform == platform)
            .ok_or("credential target channel is not declared")?;
        let fields = channel
            .required_environment
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        if fields.iter().any(|field| !valid_field(field)) {
            return Err("channel credential field is invalid".into());
        }
        let digest = hash(
            &serde_json::to_vec(&(
                &channel.plugin,
                &channel.platform,
                &channel.transport,
                &channel.entrypoint,
                &channel.required_environment,
                &channel.description,
                &channel.sdk,
            ))
            .expect("channel authorization identity"),
        );
        Ok((fields, digest))
    } else {
        let declaration = definition
            .mcp_servers
            .iter()
            .find(|declaration| declaration.name == server)
            .ok_or("credential target MCP server is not declared")?;
        Ok((
            declared_fields(&declaration.config),
            definition.binding().authorization_digest.into(),
        ))
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn request(
    data: &Path,
    library: &Path,
    sessions: &SessionStore,
    owner: &str,
    session: &Session,
    turn: &str,
    plugin: &str,
    server: &str,
    field: &str,
    purpose: &str,
) -> Result<CredentialRequest, String> {
    if !crate::pilot_tools::identifier(plugin)
        || !crate::pilot_tools::identifier(server)
        || !valid_field(field)
        || purpose.len() > 2000
    {
        return Err("invalid credential request".into());
    }
    if std::env::var_os(field).is_some() {
        return Err("this credential field is managed externally by the deployment".into());
    }
    if sessions
        .get_turn(owner, &session.id, turn)
        .map_err(|error| error.to_string())?
        .status
        != TurnStatus::Running
    {
        return Err("credential request requires an active Pilot Turn".into());
    }
    let definition = FilePluginCatalog::new(library)
        .definition(plugin)
        .map_err(|_| "credential target Plugin is not installed")?;
    let (fields, authorization_digest) = target_authorization(&definition, server)?;
    if !fields.contains(field) {
        return Err("credential field is not declared by this Plugin target".into());
    }
    let _lease = lease(data)?;
    let id =
        hash(&serde_json::to_vec(&(turn, plugin, server, field)).expect("credential identity"));
    let path = requests_root(data, owner, session).join(format!("{id}.json"));
    if let Some(fact) = load::<RequestFact>(&path)? {
        check_fact(&fact, owner, session)?;
        return Ok(fact.request);
    }
    let request = CredentialRequest {
        id,
        session: session.id.clone(),
        turn: turn.into(),
        plugin: plugin.into(),
        server: server.into(),
        field: field.into(),
        purpose: purpose.into(),
        status: "pending".into(),
        expires_at: chrono::Utc::now().timestamp() + 1800,
    };
    save(
        &path,
        &RequestFact {
            format: 1,
            owner: owner.into(),
            created_at: session.created_at.to_rfc3339(),
            digest: definition.digest.clone(),
            authorization_digest,
            request: request.clone(),
        },
    )?;
    Ok(request)
}
fn check_fact(fact: &RequestFact, owner: &str, session: &Session) -> Result<(), String> {
    let id = hash(
        &serde_json::to_vec(&(
            &fact.request.turn,
            &fact.request.plugin,
            &fact.request.server,
            &fact.request.field,
        ))
        .expect("credential identity"),
    );
    if fact.format != 1
        || fact.owner != owner
        || fact.request.id != id
        || fact.request.session != session.id
        || fact.created_at != session.created_at.to_rfc3339()
        || !matches!(
            fact.request.status.as_str(),
            "pending" | "configured" | "cancelled" | "interrupted"
        )
        || !valid_field(&fact.request.field)
        || !crate::pilot_tools::identifier(&fact.request.plugin)
        || !crate::pilot_tools::identifier(&fact.request.server)
    {
        return Err("credential request ownership or status changed".into());
    }
    Ok(())
}
fn request_path(data: &Path, owner: &str, session: &Session, id: &str) -> Result<PathBuf, String> {
    if id.len() != 64 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("invalid credential request ID".into());
    }
    Ok(requests_root(data, owner, session).join(format!("{id}.json")))
}
pub(crate) fn list(
    data: &Path,
    sessions: &SessionStore,
    owner: &str,
    session: &Session,
) -> Result<Vec<CredentialRequest>, String> {
    let _lease = lease(data)?;
    let root = requests_root(data, owner, session);
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err("credential requests unavailable".into()),
    };
    let mut requests = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|_| "credential requests unavailable")?;
        let Some(mut fact) = load::<RequestFact>(&entry.path())? else {
            continue;
        };
        check_fact(&fact, owner, session)?;
        if fact.request.status == "pending"
            && (fact.request.expires_at < chrono::Utc::now().timestamp()
                || sessions
                    .get_turn(owner, &session.id, &fact.request.turn)
                    .map_err(|error| error.to_string())?
                    .status
                    != TurnStatus::Running)
        {
            fact.request.status = "interrupted".into();
            save(&entry.path(), &fact)?;
        }
        requests.push(fact.request);
    }
    requests.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(requests)
}
pub(crate) fn get(
    data: &Path,
    sessions: &SessionStore,
    owner: &str,
    session: &Session,
    id: &str,
) -> Result<CredentialRequest, String> {
    list(data, sessions, owner, session)?
        .into_iter()
        .find(|request| request.id == id)
        .ok_or_else(|| "credential request unavailable".into())
}
pub(crate) fn submit(
    data: &Path,
    library: &Path,
    sessions: &SessionStore,
    owner: &str,
    session: &Session,
    id: &str,
    value: &str,
) -> Result<CredentialRequest, String> {
    if value.is_empty() || value.len() > 65536 || value.contains('\0') {
        return Err("credential value must be nonempty and bounded".into());
    }
    let _lease = lease(data)?;
    let path = request_path(data, owner, session, id)?;
    let mut fact = load::<RequestFact>(&path)?.ok_or("credential request unavailable")?;
    check_fact(&fact, owner, session)?;
    if fact.request.status == "configured" {
        let binding_owner = binding_owner(owner);
        let previous = load::<Secret>(&secret_path(
            data,
            &binding_owner,
            &fact.request.plugin,
            &fact.request.server,
            &fact.request.field,
        ))?
        .ok_or("configured credential value unavailable")?;
        if previous.value != value {
            return Err("this request already configured a different value; request a fresh credential form".into());
        }
        return Ok(fact.request);
    }
    if fact.request.status != "pending"
        || fact.request.expires_at < chrono::Utc::now().timestamp()
        || sessions
            .get_turn(owner, &session.id, &fact.request.turn)
            .map_err(|error| error.to_string())?
            .status
            != TurnStatus::Running
    {
        return Err("credential request is no longer pending".into());
    }
    if std::env::var_os(&fact.request.field).is_some() {
        return Err("credential ownership changed to the external deployment".into());
    }
    let definition = FilePluginCatalog::new(library)
        .definition(&fact.request.plugin)
        .map_err(|_| "credential target no longer installed")?;
    if definition.digest != fact.digest
        || target_authorization(&definition, &fact.request.server)?.1 != fact.authorization_digest
    {
        return Err(
            "Plugin changed while credential input was pending; request a fresh form".into(),
        );
    }
    let _synchronization_warning = management::mutate_locked(
        data,
        owner,
        &definition,
        &fact.request.server,
        &fact.request.field,
        &fact.authorization_digest,
        Some(value),
        sessions,
    )?;
    fact.request.status = "configured".into();
    save(&path, &fact)?;
    Ok(fact.request)
}
pub(crate) fn cancel(
    data: &Path,
    owner: &str,
    session: &Session,
    id: &str,
    interrupted: bool,
) -> Result<CredentialRequest, String> {
    let _lease = lease(data)?;
    let path = request_path(data, owner, session, id)?;
    let mut fact = load::<RequestFact>(&path)?.ok_or("credential request unavailable")?;
    check_fact(&fact, owner, session)?;
    if fact.request.status == "pending" {
        fact.request.status = if interrupted {
            "interrupted"
        } else {
            "cancelled"
        }
        .into();
        save(&path, &fact)?;
    }
    Ok(fact.request)
}
pub(crate) fn environment_values(
    data: &Path,
    owner: &str,
    definition: &PluginDefinition,
    server: &str,
) -> Result<BTreeMap<String, String>, String> {
    let (fields, authorization_digest) = target_authorization(definition, server)?;
    let mut values = BTreeMap::new();
    for field in fields {
        if std::env::var_os(&field).is_some() {
            continue;
        }
        let Some(secret) =
            load::<Secret>(&secret_path(data, owner, &definition.id, server, &field))?
        else {
            continue;
        };
        if secret.format != 1
            || secret.owner != owner
            || secret.plugin != definition.id
            || secret.server != server
            || secret.field != field
            || secret.authorization_digest != authorization_digest
        {
            return Err(
                "stored credential authorization no longer matches this Plugin target".into(),
            );
        }
        values.insert(field, secret.value);
    }
    Ok(values)
}
pub(crate) fn delete_requests(data: &Path, owner: &str, session: &Session) -> Result<(), String> {
    let _lease = lease(data)?;
    let root = requests_root(data, owner, session);
    if root.exists() {
        crate::goose_acp::pilot::directory(&root)?;
        fs::remove_dir_all(root).map_err(|_| "credential request cleanup failed")?;
    }
    Ok(())
}
pub(crate) fn delete_plugin(data: &Path, plugin: &str) -> Result<(), String> {
    if !crate::pilot_tools::identifier(plugin) {
        return Err("invalid credential Plugin ID".into());
    }
    let _lease = lease(data)?;
    let root = data.join("platform/credentials");
    if private_parent(&root.join("probe"))?.is_some() {
        for entry in fs::read_dir(&root).map_err(|_| "credential owner directories unavailable")? {
            let owner = entry
                .map_err(|_| "credential owner directories unavailable")?
                .path();
            let directory = owner.join(plugin);
            if private_parent(&directory.join("probe"))?.is_some() {
                fs::remove_dir_all(directory)
                    .map_err(|_| "Plugin private credential cleanup failed")?;
            }
        }
    }
    let requests = data.join("platform/credential-requests");
    if private_parent(&requests.join("probe"))?.is_some() {
        for entry in fs::read_dir(&requests).map_err(|_| "credential request scopes unavailable")? {
            let directory = entry
                .map_err(|_| "credential request scopes unavailable")?
                .path();
            private_parent(&directory.join("probe"))?
                .ok_or("credential request scope unavailable")?;
            for entry in fs::read_dir(directory).map_err(|_| "credential requests unavailable")? {
                let path = entry.map_err(|_| "credential request unavailable")?.path();
                if load::<RequestFact>(&path)?.is_some_and(|fact| fact.request.plugin == plugin) {
                    fs::remove_file(path)
                        .map_err(|_| "Plugin credential request cleanup failed")?;
                }
            }
        }
    }
    Ok(())
}
pub(crate) fn public_result(request: &CredentialRequest) -> Value {
    json!({"request":request})
}

#[cfg(test)]
mod tests {
    use super::*;
    use anchor_platform_session::CreateSession;
    pub(super) fn fixture() -> (tempfile::TempDir, SessionStore, Session, String) {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("state");
        fs::create_dir_all(data.join("platform")).unwrap();
        fs::create_dir_all(root.path().join("plugins/demo")).unwrap();
        fs::write(root.path().join("plugins/demo/plugin.json"),json!({"name":"Demo","mcpServers":{"api":{"url":"https://example.com/mcp","bearer_token_env_var":"ANCHOR_TEST_PRIVATE_CREDENTIAL"}}}).to_string()).unwrap();
        let sessions = SessionStore::open(data.join("platform/sessions.sqlite")).unwrap();
        let session = sessions
            .create(
                "owner-a",
                CreateSession {
                    id: Some("pilot-a".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        let (turn, _) = sessions
            .create_turn("owner-a", &session.id, "test", Some("configure"))
            .unwrap();
        (root, sessions, session, turn.id)
    }
    #[test]
    fn private_input_is_owner_bound_and_never_public_and_conflicting_retry_is_rejected() {
        let (root, sessions, session, turn) = fixture();
        let data = root.path().join("state");
        let request = request(
            &data,
            root.path(),
            &sessions,
            "owner-a",
            &session,
            &turn,
            "demo",
            "api",
            "ANCHOR_TEST_PRIVATE_CREDENTIAL",
            "configure test",
        )
        .unwrap();
        assert!(
            submit(
                &data,
                root.path(),
                &sessions,
                "owner-b",
                &session,
                &request.id,
                "private-value"
            )
            .is_err()
        );
        let submitted = submit(
            &data,
            root.path(),
            &sessions,
            "owner-a",
            &session,
            &request.id,
            "private-value",
        )
        .unwrap();
        assert_eq!(submitted.status, "configured");
        assert!(
            !public_result(&submitted)
                .to_string()
                .contains("private-value")
        );
        assert!(
            submit(
                &data,
                root.path(),
                &sessions,
                "owner-a",
                &session,
                &request.id,
                "private-value"
            )
            .is_ok()
        );
        assert!(
            submit(
                &data,
                root.path(),
                &sessions,
                "owner-a",
                &session,
                &request.id,
                "changed-value"
            )
            .is_err()
        );
        let definition = FilePluginCatalog::new(root.path())
            .definition("demo")
            .unwrap();
        assert_eq!(
            environment_values(&data, &binding_owner("owner-a"), &definition, "api").unwrap()["ANCHOR_TEST_PRIVATE_CREDENTIAL"],
            "private-value"
        );
        assert!(
            environment_values(&data, &binding_owner("owner-b"), &definition, "api")
                .unwrap()
                .is_empty()
        );
        let configured = crate::tool_host::McpToolConfig::from_definitions_for_owner(
            root.path(),
            &[definition.binding()],
            &BTreeMap::from([("demo".into(), definition)]),
            Some((&data, &binding_owner("owner-a"))),
        );
        assert!(configured.is_ok());
        let events = sessions.events("owner-a", &session.id, 0).unwrap();
        assert!(
            !serde_json::to_string(&events)
                .unwrap()
                .contains("private-value")
        );
        assert_eq!(
            fs::metadata(secret_path(
                &data,
                &binding_owner("owner-a"),
                "demo",
                "api",
                "ANCHOR_TEST_PRIVATE_CREDENTIAL"
            ))
            .unwrap()
            .permissions()
            .mode()
                & 0o777,
            0o600
        );
    }
    #[test]
    fn cancellation_stopped_turn_expiry_and_changed_plugin_reject_late_secret() {
        let (root, sessions, session, turn) = fixture();
        let data = root.path().join("state");
        let first = request(
            &data,
            root.path(),
            &sessions,
            "owner-a",
            &session,
            &turn,
            "demo",
            "api",
            "ANCHOR_TEST_PRIVATE_CREDENTIAL",
            "configure",
        )
        .unwrap();
        cancel(&data, "owner-a", &session, &first.id, false).unwrap();
        assert!(
            submit(
                &data,
                root.path(),
                &sessions,
                "owner-a",
                &session,
                &first.id,
                "value"
            )
            .is_err()
        );
        sessions
            .finish_turn("owner-a", &session.id, &turn, TurnStatus::Completed, None)
            .unwrap();
        let (next, _) = sessions
            .create_turn("owner-a", &session.id, "next", Some("configure"))
            .unwrap();
        let request = request(
            &data,
            root.path(),
            &sessions,
            "owner-a",
            &session,
            &next.id,
            "demo",
            "api",
            "ANCHOR_TEST_PRIVATE_CREDENTIAL",
            "configure",
        )
        .unwrap();
        sessions
            .finish_turn("owner-a", &session.id, &next.id, TurnStatus::Stopped, None)
            .unwrap();
        assert_eq!(
            get(&data, &sessions, "owner-a", &session, &request.id)
                .unwrap()
                .status,
            "interrupted"
        );
        assert!(
            submit(
                &data,
                root.path(),
                &sessions,
                "owner-a",
                &session,
                &request.id,
                "value"
            )
            .is_err()
        );
        let (last, _) = sessions
            .create_turn("owner-a", &session.id, "last", Some("configure"))
            .unwrap();
        let request = self::request(
            &data,
            root.path(),
            &sessions,
            "owner-a",
            &session,
            &last.id,
            "demo",
            "api",
            "ANCHOR_TEST_PRIVATE_CREDENTIAL",
            "configure",
        )
        .unwrap();
        let path = request_path(&data, "owner-a", &session, &request.id).unwrap();
        let mut fact = load::<RequestFact>(&path).unwrap().unwrap();
        fact.request.expires_at = 0;
        save(&path, &fact).unwrap();
        assert_eq!(
            get(&data, &sessions, "owner-a", &session, &request.id)
                .unwrap()
                .status,
            "interrupted"
        );
        sessions
            .finish_turn(
                "owner-a",
                &session.id,
                &last.id,
                TurnStatus::Completed,
                None,
            )
            .unwrap();
        let (changed, _) = sessions
            .create_turn("owner-a", &session.id, "changed", Some("configure"))
            .unwrap();
        let request = self::request(
            &data,
            root.path(),
            &sessions,
            "owner-a",
            &session,
            &changed.id,
            "demo",
            "api",
            "ANCHOR_TEST_PRIVATE_CREDENTIAL",
            "configure",
        )
        .unwrap();
        fs::write(root.path().join("plugins/demo/changed.txt"), "changed").unwrap();
        assert!(
            submit(
                &data,
                root.path(),
                &sessions,
                "owner-a",
                &session,
                &request.id,
                "value"
            )
            .is_err()
        );
    }
    #[test]
    fn private_fact_reader_refuses_fifo_and_parent_symlink_without_blocking() {
        let root = tempfile::tempdir().unwrap();
        let fifo = root.path().join("fact.json");
        rustix::fs::mkfifoat(
            rustix::fs::CWD,
            &fifo,
            rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        )
        .unwrap();
        assert!(load::<Value>(&fifo).is_err());
        fs::create_dir(root.path().join("real")).unwrap();
        fs::write(root.path().join("real/fact.json"), "{}").unwrap();
        std::os::unix::fs::symlink(root.path().join("real"), root.path().join("alias")).unwrap();
        assert!(load::<Value>(&root.path().join("alias/fact.json")).is_err());
    }
    #[test]
    fn private_store_rejects_readable_secret_and_directory_and_uninstall_removes_all_owners() {
        let (root, sessions, session, turn) = fixture();
        let data = root.path().join("state");
        let request = request(
            &data,
            root.path(),
            &sessions,
            "owner-a",
            &session,
            &turn,
            "demo",
            "api",
            "ANCHOR_TEST_PRIVATE_CREDENTIAL",
            "configure",
        )
        .unwrap();
        submit(
            &data,
            root.path(),
            &sessions,
            "owner-a",
            &session,
            &request.id,
            "private-value",
        )
        .unwrap();
        let definition = FilePluginCatalog::new(root.path())
            .definition("demo")
            .unwrap();
        let path = secret_path(
            &data,
            &binding_owner("owner-a"),
            "demo",
            "api",
            "ANCHOR_TEST_PRIVATE_CREDENTIAL",
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(environment_values(&data, &binding_owner("owner-a"), &definition, "api").is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(environment_values(&data, &binding_owner("owner-a"), &definition, "api").is_err());
        fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
        delete_plugin(&data, "demo").unwrap();
        assert!(!path.exists());
        assert!(
            list(&data, &sessions, "owner-a", &session)
                .unwrap()
                .is_empty()
        );
    }
}
