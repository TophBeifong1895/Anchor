//! Host-owned model settings. Public fields and provider credentials have separate
//! representations; credentials never enter status, history, or diagnostics.
use crate::goose_acp::configuration::ModelRegistry;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
mod connection_test;
mod execution;
mod providers;
pub(crate) use providers::{ProviderTest, ProviderUpdate};

use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

const ENV_FIELDS: [(&str, &str); 8] = [
    ("endpoint", "ANCHOR_MODEL_URL"),
    ("wire_api", "ANCHOR_MODEL_WIRE_API"),
    ("model", "ANCHOR_MODEL_NAME"),
    ("context_window", "ANCHOR_MODEL_CONTEXT_WINDOW"),
    ("max_tokens", "ANCHOR_MODEL_MAX_TOKENS"),
    ("aliases", "ANCHOR_MODEL_ALIASES"),
    ("api_key", "ANCHOR_MODEL_API_KEY"),
    ("direct", "ANCHOR_MODEL_DIRECT"),
];

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ModelFields {
    pub(crate) endpoint: String,
    pub(crate) wire_api: String,
    pub(crate) model: String,
    pub(crate) context_window: Option<u64>,
    pub(crate) max_tokens: Option<u64>,
    pub(crate) aliases: BTreeMap<String, String>,
}
impl Default for ModelFields {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            wire_api: "responses".into(),
            model: "default".into(),
            context_window: None,
            max_tokens: None,
            aliases: BTreeMap::new(),
        }
    }
}
impl ModelFields {
    fn values(&self) -> BTreeMap<String, String> {
        let mut values = BTreeMap::from([
            ("ANCHOR_MODEL_URL".into(), self.endpoint.clone()),
            ("ANCHOR_MODEL_WIRE_API".into(), self.wire_api.clone()),
            ("ANCHOR_MODEL_NAME".into(), self.model.clone()),
            (
                "ANCHOR_MODEL_ALIASES".into(),
                serde_json::to_string(&self.aliases).expect("aliases serialize"),
            ),
        ]);
        for (name, value) in [
            ("ANCHOR_MODEL_CONTEXT_WINDOW", self.context_window),
            ("ANCHOR_MODEL_MAX_TOKENS", self.max_tokens),
        ] {
            if let Some(value) = value {
                values.insert(name.into(), value.to_string());
            }
        }
        values
    }
    fn provider_id(&self) -> String {
        // ModelRegistry normalizes and validates the URL before this is used.
        let endpoint = reqwest::Url::parse(&self.endpoint)
            .map(|url| url.to_string().trim_end_matches('/').to_owned())
            .unwrap_or_default();
        format!(
            "{:x}",
            Sha256::digest(format!("{}\n{endpoint}", self.wire_api))
        )
    }
}

#[derive(Default, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum KeyUpdate {
    #[default]
    Keep,
    Set {
        value: String,
    },
    Clear,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ModelUpdate {
    #[serde(flatten)]
    pub(crate) fields: ModelFields,
    #[serde(default)]
    pub(crate) api_key: KeyUpdate,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManagedModel {
    format: u32,
    current: ModelFields,
    #[serde(default)]
    history: Vec<ModelFields>,
    #[serde(default)]
    credentials: BTreeMap<String, String>,
    #[serde(default)]
    providers: providers::ProviderCatalog,
}
impl Default for ManagedModel {
    fn default() -> Self {
        Self {
            format: 3,
            current: ModelFields::default(),
            history: vec![],
            credentials: BTreeMap::new(),
            providers: providers::ProviderCatalog::default(),
        }
    }
}

pub(crate) struct ModelConfiguration {
    root: PathBuf,
    external: BTreeMap<String, String>,
}
impl ModelConfiguration {
    pub(crate) fn from_env(state: &Path) -> Result<Self, String> {
        let mut external = BTreeMap::new();
        for (_, name) in ENV_FIELDS {
            match std::env::var(name) {
                Ok(value) => {
                    external.insert(name.to_owned(), value);
                }
                Err(std::env::VarError::NotPresent) => {}
                Err(_) => return Err(format!("{name} must be valid Unicode")),
            }
        }
        Self::new(state, external)
    }
    fn new(state: &Path, external: BTreeMap<String, String>) -> Result<Self, String> {
        let root = state.join("managed");
        crate::goose_acp::pilot::directory(&root)?;
        Ok(Self { root, external })
    }
    fn load(&self) -> Result<ManagedModel, String> {
        let path = self.root.join("model-config.json");
        let mut file = match fs::OpenOptions::new()
            .read(true)
            .custom_flags(
                (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32,
            )
            .open(path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ManagedModel::default());
            }
            Err(_) => return Err("Host model configuration is unavailable".into()),
        };
        let metadata = file
            .metadata()
            .map_err(|_| "Host model configuration is unavailable")?;
        if !metadata.is_file()
            || metadata.len() > 1024 * 1024
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err("Host model configuration must be a private bounded regular file".into());
        }
        let mut bytes = vec![];
        file.read_to_end(&mut bytes)
            .map_err(|_| "Host model configuration is unavailable")?;
        let managed: ManagedModel =
            serde_json::from_slice(&bytes).map_err(|_| "Host model configuration is malformed")?;
        if !matches!(managed.format, 1..=3) {
            return Err("unsupported Host model configuration format".into());
        }
        Ok(managed)
    }
    fn lease(&self) -> Result<anchor_file_lease::FileLease, String> {
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(
                (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32,
            )
            .open(self.root.join("model-config.lock"))
            .map_err(|_| "model configuration lock is unavailable")?;
        if !file
            .metadata()
            .map_err(|_| "model configuration lock is unavailable")?
            .is_file()
        {
            return Err("model configuration lock must be a regular file".into());
        }
        let file =
            anchor_file_lease::FileLease::acquire(file, rustix::fs::FlockOperation::LockExclusive)
                .map_err(|_| "model configuration lock is unavailable")?;
        Ok(file)
    }
    fn publish(&self, managed: &ManagedModel) -> Result<(), String> {
        let mut managed = managed.clone();
        managed.format = 3;
        let mut file = tempfile::NamedTempFile::new_in(&self.root)
            .map_err(|_| "model configuration could not be saved")?;
        file.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|_| "model configuration could not be saved privately")?;
        let bytes = serde_json::to_vec(&managed)
            .map_err(|_| "model configuration could not be serialized")?;
        if bytes.len() > 1024 * 1024 {
            return Err("model configuration exceeds the Host storage limit".into());
        }
        file.write_all(&bytes)
            .and_then(|_| file.as_file().sync_all())
            .map_err(|_| "model configuration could not be saved")?;
        file.persist(self.root.join("model-config.json"))
            .map_err(|_| "model configuration could not be published")?;
        fs::File::open(&self.root)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| "model configuration could not be synchronized".to_owned())
    }
    fn effective(&self, fields: &ModelFields) -> Result<ModelFields, String> {
        let mut fields = fields.clone();
        for (name, value) in &self.external {
            match name.as_str() {
                "ANCHOR_MODEL_URL" => fields.endpoint = value.clone(),
                "ANCHOR_MODEL_WIRE_API" => fields.wire_api = value.clone(),
                "ANCHOR_MODEL_NAME" => fields.model = value.clone(),
                "ANCHOR_MODEL_CONTEXT_WINDOW" => {
                    fields.context_window =
                        Some(value.trim().parse().map_err(
                            |_| "ANCHOR_MODEL_CONTEXT_WINDOW must be a positive integer",
                        )?)
                }
                "ANCHOR_MODEL_MAX_TOKENS" => {
                    fields.max_tokens = Some(
                        value
                            .trim()
                            .parse()
                            .map_err(|_| "ANCHOR_MODEL_MAX_TOKENS must be a positive integer")?,
                    )
                }
                "ANCHOR_MODEL_ALIASES" => {
                    fields.aliases = serde_json::from_str(value).map_err(
                        |_| "ANCHOR_MODEL_ALIASES must be a JSON object of model aliases",
                    )?
                }
                _ => {}
            }
        }
        Ok(fields)
    }
    fn key<'a>(&'a self, managed: &'a ManagedModel, fields: &ModelFields) -> Option<&'a str> {
        let current = self.effective(&managed.current).ok()?;
        if current.provider_id() == fields.provider_id()
            && let Some(value) = self.external.get("ANCHOR_MODEL_API_KEY")
        {
            return Some(value);
        }
        managed
            .credentials
            .get(&fields.provider_id())
            .map(String::as_str)
    }
    fn registry(
        &self,
        managed: &ManagedModel,
        fields: &ModelFields,
        validation: bool,
    ) -> Result<ModelRegistry, String> {
        let mut values = fields.values();
        if let Some(key) = self
            .key(managed, fields)
            .or(validation.then_some("validation-placeholder"))
        {
            values.insert("ANCHOR_MODEL_API_KEY".into(), key.into());
        }
        if let Some(direct) = self.external.get("ANCHOR_MODEL_DIRECT") {
            values.insert("ANCHOR_MODEL_DIRECT".into(), direct.clone());
        }
        ModelRegistry::from_values(false, |name| values.get(name).cloned())
    }
    #[cfg(test)]
    pub(crate) fn snapshot(
        &self,
        binding: Option<&str>,
        reference: Option<&str>,
    ) -> Result<ModelRegistry, String> {
        let managed = self.load()?;
        self.legacy_snapshot(&managed, binding, reference)
    }
    fn legacy_snapshot(
        &self,
        managed: &ManagedModel,
        binding: Option<&str>,
        reference: Option<&str>,
    ) -> Result<ModelRegistry, String> {
        let current = self.effective(&managed.current)?;
        if let Some(binding) = binding {
            for fields in std::iter::once(&current).chain(&managed.history) {
                if let Ok(registry) = self.registry(managed, fields, false)
                    && registry
                        .resolve(reference)
                        .is_ok_and(|candidate| candidate.identity == binding)
                {
                    return Ok(registry);
                }
            }
            return Err("the retained model provider credential is unavailable; open Settings to restore its key or start a new Session".into());
        }
        let retained = managed
            .history
            .iter()
            .filter_map(|fields| self.registry(managed, fields, false).ok())
            .collect();
        self.registry(managed, &current, false)
            .map(|registry| registry.with_retained(retained))
    }
    pub(crate) fn status(&self) -> Result<Value, String> {
        let managed = self.load()?;
        self.project(&managed)
    }
    fn project(&self, managed: &ManagedModel) -> Result<Value, String> {
        let current = self.effective(&managed.current)?;
        let configured = self.registry(managed, &current, false).is_ok();
        let mut providers = BTreeMap::new();
        for fields in std::iter::once(&current).chain(&managed.history) {
            if fields.endpoint.is_empty() {
                continue;
            }
            let id = fields.provider_id();
            providers.entry(id.clone()).or_insert_with(|| json!({"id":id,"endpoint":fields.endpoint,"wire_api":fields.wire_api,"api_key_present":self.key(managed, fields).is_some(),"current":fields.provider_id() == current.provider_id(),"deployment":fields.provider_id() == current.provider_id() && self.external.contains_key("ANCHOR_MODEL_API_KEY")}));
        }
        self.project_slot_credentials(managed, &mut providers);
        let mut status = json!({"configured":configured,"fields":current,"api_key_present":self.key(managed, &current).is_some(),"deployment_fields":ENV_FIELDS.iter().filter(|(_,env)| self.external.contains_key(*env)).map(|(field,_)| *field).collect::<Vec<_>>(),"retained_providers":providers.into_values().collect::<Vec<_>>(),"notice":"新调用读取最新保存的模型；正在执行的调用保持原快照。同一次调用恢复时保留原模型和服务地址，可使用同一服务更新后的密钥。"});
        status["providers"] = self.provider_statuses(managed)?;
        status["execution"] = self.execution_status(managed)?;
        Ok(status)
    }
    fn draft(&self, managed: &mut ManagedModel, update: ModelUpdate) -> Result<(), String> {
        let before = self.effective(&managed.current)?;
        let draft = self.effective(&update.fields)?;
        let supplied =
            serde_json::to_value(&update.fields).map_err(|_| "invalid model settings")?;
        let effective = serde_json::to_value(&draft).map_err(|_| "invalid model settings")?;
        for (field, env) in ENV_FIELDS {
            if field != "api_key"
                && self.external.contains_key(env)
                && supplied.get(field) != effective.get(field)
            {
                return Err(format!("{field} is managed by this deployment"));
            }
        }
        if self.external.contains_key("ANCHOR_MODEL_API_KEY")
            && !matches!(update.api_key, KeyUpdate::Keep)
        {
            return Err("api_key is managed by this deployment".into());
        }
        self.registry(managed, &draft, true)?;
        if !before.endpoint.is_empty()
            && self.registry(managed, &before, true).is_ok()
            && before != draft
            && !managed.history.contains(&before)
        {
            managed.history.push(before);
        }
        let id = draft.provider_id();
        match update.api_key {
            KeyUpdate::Keep => {}
            KeyUpdate::Set { value } => {
                // Reuse the registry's credential validation; values are never formatted.
                let mut values = draft.values();
                values.insert("ANCHOR_MODEL_API_KEY".into(), value.clone());
                ModelRegistry::from_values(false, |name| values.get(name).cloned())?;
                managed.credentials.insert(id, value);
            }
            KeyUpdate::Clear => {
                managed.credentials.remove(&id);
            }
        }
        managed.current = update.fields;
        Ok(())
    }
    pub(crate) fn save(&self, update: ModelUpdate) -> Result<Value, String> {
        let _lease = self.lease()?;
        let mut managed = self.load()?;
        self.draft(&mut managed, update)?;
        let status = self.project(&managed)?;
        self.publish(&managed)?;
        Ok(status)
    }
    pub(crate) fn set_provider_credential(&self, id: &str, value: String) -> Result<Value, String> {
        let _lease = self.lease()?;
        let mut managed = self.load()?;
        if self.update_slot_credential(&mut managed, id, Some(&value))? {
            let status = self.project(&managed)?;
            self.publish(&managed)?;
            return Ok(status);
        }
        let current = self.effective(&managed.current)?;
        if id == current.provider_id() && self.external.contains_key("ANCHOR_MODEL_API_KEY") {
            return Err("api_key is managed by this deployment".into());
        }
        let fields = std::iter::once(&current)
            .chain(&managed.history)
            .find(|fields| fields.provider_id() == id)
            .ok_or("model provider not found")?;
        let mut values = fields.values();
        values.insert("ANCHOR_MODEL_API_KEY".into(), value.clone());
        ModelRegistry::from_values(false, |name| values.get(name).cloned())?;
        managed.credentials.insert(id.to_owned(), value);
        self.publish(&managed)?;
        self.project(&managed)
    }

    pub(crate) fn clear_provider_credential(&self, id: &str) -> Result<Value, String> {
        let _lease = self.lease()?;
        let mut managed = self.load()?;
        if self.update_slot_credential(&mut managed, id, None)? {
            let status = self.project(&managed)?;
            self.publish(&managed)?;
            return Ok(status);
        }
        let current = self.effective(&managed.current)?;
        if id == current.provider_id() && self.external.contains_key("ANCHOR_MODEL_API_KEY") {
            return Err("api_key is managed by this deployment".into());
        }
        if !std::iter::once(&current)
            .chain(&managed.history)
            .any(|fields| fields.provider_id() == id)
        {
            return Err("model provider not found".into());
        }
        managed.credentials.remove(id);
        self.publish(&managed)?;
        self.project(&managed)
    }
    pub(crate) async fn test(&self, update: ModelUpdate) -> Result<Value, String> {
        let mut managed = self.load()?;
        self.draft(&mut managed, update)?;
        let fields = self.effective(&managed.current)?;
        let registry = self.registry(&managed, &fields, false)?;
        connection_test::run(&registry, &fields).await
    }
}

#[cfg(test)]
mod tests;
