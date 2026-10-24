//! One Host-owned binding for the existing WeCom supervisor. Persisted values
//! are configuration facts, not a new channel runtime or environment asset.
use super::{ChannelSupervisor, ManagedChannelLaunch, random_token};
use crate::application::{
    ApplicationError,
    channel_management::{ChannelBinding, ChannelManagementPort, ChannelRuntimeState},
};
use anchor_graph_host::{
    ChannelDefinition, FileGraphBundleLoader, FilePluginCatalog, PluginMetadata,
};
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::RwLock,
};
use tokio::sync::Mutex;

pub(crate) struct ChannelHostConfiguration {
    pub(crate) data_root: PathBuf,
    pub(crate) catalog_root: PathBuf,
    pub(crate) library_root: PathBuf,
    pub(crate) listen: String,
    pub(crate) deployment_owner: String,
    deployment_binding: Option<ChannelBinding>,
    deployment_account: Option<String>,
}

impl ChannelHostConfiguration {
    pub(crate) fn from_env(
        data_root: PathBuf,
        catalog_root: PathBuf,
        library_root: PathBuf,
        listen: String,
        deployment_owner: String,
    ) -> Self {
        let externally_managed = [
            "ANCHOR_WECOM_GRAPH",
            "ANCHOR_WECOM_REPLY_NODE",
            "ANCHOR_WECOM_USERS",
            "ANCHOR_WECOM_SEND_USERS",
        ]
        .iter()
        .any(|field| env::var_os(field).is_some());
        let users = parse_users(&env::var("ANCHOR_WECOM_USERS").unwrap_or_default());
        let send_users = env::var("ANCHOR_WECOM_SEND_USERS")
            .map(|users| parse_users(&users))
            .unwrap_or_else(|_| users.clone());
        Self {
            data_root,
            catalog_root,
            library_root,
            listen,
            deployment_owner,
            deployment_binding: externally_managed.then(|| ChannelBinding {
                graph: env::var("ANCHOR_WECOM_GRAPH")
                    .unwrap_or_default()
                    .trim()
                    .into(),
                reply_node: env::var("ANCHOR_WECOM_REPLY_NODE")
                    .unwrap_or_else(|_| "assistant".into())
                    .trim()
                    .into(),
                plugin: String::new(),
                users,
                send_users,
            }),
            deployment_account: env::var("WECOM_BOT_ID")
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty()),
        }
    }
}

fn parse_users(users: &str) -> Vec<String> {
    users
        .split(',')
        .map(str::trim)
        .filter(|user| !user.is_empty())
        .map(str::to_owned)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ChannelFact {
    format: u32,
    owner: String,
    binding: ChannelBinding,
    enabled: bool,
    authorization_digest: String,
    callback_key: String,
    #[serde(default)]
    account: Option<String>,
}

pub(crate) struct ChannelManagementAdapter {
    configuration: ChannelHostConfiguration,
    root: PathBuf,
    current: RwLock<Option<ChannelFact>>,
    supervisor: Mutex<Option<ChannelSupervisor>>,
    external: bool,
    external_descriptors: BTreeMap<String, PathBuf>,
    external_callback_keys: BTreeMap<String, String>,
    _owner_lease: anchor_file_lease::FileLease,
}

impl ChannelManagementAdapter {
    pub(crate) fn open(
        mut configuration: ChannelHostConfiguration,
        supervisor: Option<ChannelSupervisor>,
    ) -> Result<Self, String> {
        let root = configuration.data_root.join("platform/channels");
        crate::goose_acp::pilot::directory(&root)?;
        let metadata = fs::metadata(&root).map_err(|_| "channel configuration root unavailable")?;
        if metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.permissions().mode() & 0o777 != 0o700
        {
            return Err(
                "channel configuration directory requires Host ownership and mode 700".into(),
            );
        }
        let owner_lease = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(
                (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32,
            )
            .open(root.join("configuration.lock"))
            .map_err(|_| "channel configuration lease unavailable")?;
        if !owner_lease
            .metadata()
            .map_err(|_| "channel configuration lease unavailable")?
            .is_file()
        {
            return Err("channel configuration lease must be a regular file".into());
        }
        let owner_lease = anchor_file_lease::FileLease::acquire(
            owner_lease,
            rustix::fs::FlockOperation::NonBlockingLockExclusive,
        )
        .map_err(|_| "channel configuration already has a Host owner")?;
        let current = read_fact(&root)?;
        let external = configuration.deployment_binding.is_some() || supervisor.is_some();
        let external_callback_keys = supervisor
            .as_ref()
            .map(|supervisor| supervisor.callback_keys.clone())
            .unwrap_or_default();
        let external_descriptors = supervisor
            .as_ref()
            .map(ChannelSupervisor::descriptors)
            .unwrap_or_default();
        if let Some(binding) = configuration.deployment_binding.as_mut() {
            if let Some(discovered) = supervisor
                .as_ref()
                .and_then(|supervisor| supervisor.definitions.get("wecom"))
            {
                binding.plugin = discovered.plugin.clone();
            } else if !binding.graph.is_empty() {
                // Resolve only installed channel declarations referenced by the
                // deployment's graph; never assume the Plugin ID is "wecom".
                if let Ok(bundle) =
                    FileGraphBundleLoader::new(configuration.catalog_root.join(&binding.graph))
                        .with_library_root(&configuration.library_root)
                        .load()
                {
                    for plugin in bundle.plugins {
                        if FilePluginCatalog::new(&configuration.library_root)
                            .channels(&plugin)
                            .is_ok_and(|channels| {
                                channels.iter().any(|channel| channel.platform == "wecom")
                            })
                        {
                            binding.plugin = plugin;
                            break;
                        }
                    }
                }
            }
        }
        if let Some(identity) = configuration.deployment_account.as_deref() {
            let account =
                register_account(&configuration, &configuration.deployment_owner, identity)?;
            configuration.deployment_account = Some(account);
        }
        Ok(Self {
            configuration,
            root,
            current: RwLock::new(current),
            supervisor: Mutex::new(supervisor),
            external,
            external_descriptors,
            external_callback_keys,
            _owner_lease: owner_lease,
        })
    }

    pub(crate) async fn resume(&self) -> Result<(), ApplicationError> {
        if self.external {
            return Ok(());
        }
        let mut supervisor = self.supervisor.lock().await;
        let Some(mut fact) = self.fact()? else {
            return Ok(());
        };
        if fact.enabled && supervisor.is_none() {
            if let Ok((definition, channel)) = self.definition(&fact.binding)
                && let Ok(credentials) = self.credentials(&fact, &definition, &channel)
            {
                let account = self.register_account(&fact.owner, &credentials)?;
                if fact.account != account {
                    fact.account = account;
                    self.publish(fact.clone())?;
                }
            }
            match self.start(&fact).await {
                Ok(started) => *supervisor = Some(started),
                // A saved binding to removed resources must not prevent the
                // otherwise healthy platform and Pilot from starting.
                Err(_) => eprintln!("anchor channel wecom: saved binding is currently unavailable"),
            }
        }
        Ok(())
    }

    pub(crate) async fn shutdown(&self) {
        let mut supervisor = self.supervisor.lock().await;
        if let Some(mut active) = supervisor.take() {
            active.shutdown().await;
        }
    }

    fn fact(&self) -> Result<Option<ChannelFact>, ApplicationError> {
        self.current
            .read()
            .map(|fact| fact.clone())
            .map_err(|_| storage("channel configuration unavailable"))
    }

    fn register_account(
        &self,
        owner: &str,
        credentials: &BTreeMap<String, String>,
    ) -> Result<Option<String>, ApplicationError> {
        credentials
            .get("WECOM_BOT_ID")
            .map(|identity| register_account(&self.configuration, owner, identity).map_err(storage))
            .transpose()
    }

    fn assert_managed_owner(&self, owner: &str) -> Result<(), ApplicationError> {
        if self.external {
            return Err(ApplicationError::Conflict(
                "this channel binding is managed externally by the deployment".into(),
            ));
        }
        if owner.is_empty() || owner.len() > 500 || owner.chars().any(char::is_control) {
            return Err(ApplicationError::Invalid("invalid channel owner".into()));
        }
        if self.fact()?.is_some_and(|fact| fact.owner != owner) {
            return Err(ApplicationError::Conflict(
                "the platform's single wecom gateway belongs to another owner".into(),
            ));
        }
        Ok(())
    }

    fn publish(&self, fact: ChannelFact) -> Result<(), ApplicationError> {
        crate::goose_acp::pilot::directory(&self.root).map_err(storage)?;
        let mut temporary = tempfile::NamedTempFile::new_in(&self.root)
            .map_err(|_| storage("channel configuration staging unavailable"))?;
        fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o600))
            .map_err(|_| storage("channel configuration permissions unavailable"))?;
        temporary
            .write_all(
                &serde_json::to_vec(&fact)
                    .map_err(|_| storage("channel configuration encoding failed"))?,
            )
            .and_then(|_| temporary.as_file().sync_all())
            .map_err(|_| storage("channel configuration persistence failed"))?;
        temporary
            .persist(self.root.join("wecom.json"))
            .map_err(|_| storage("channel configuration publication failed"))?;
        *self
            .current
            .write()
            .map_err(|_| storage("channel configuration unavailable"))? = Some(fact);
        if fs::File::open(&self.root)
            .and_then(|directory| directory.sync_all())
            .is_err()
        {
            // The rename has already published; retain the matching in-memory
            // fact rather than roll back to a snapshot no longer on disk.
            eprintln!("channel configuration published; directory sync failed");
        }
        Ok(())
    }

    fn definition(
        &self,
        binding: &ChannelBinding,
    ) -> Result<(PluginMetadata, ChannelDefinition), ApplicationError> {
        let definition = FilePluginCatalog::new(&self.configuration.library_root)
            .definition(&binding.plugin)
            .map_err(|_| ApplicationError::Missing)?;
        Self::channel(definition.metadata())
    }

    fn metadata(
        &self,
        binding: &ChannelBinding,
    ) -> Result<(PluginMetadata, ChannelDefinition), ApplicationError> {
        let definition = FilePluginCatalog::new(&self.configuration.library_root)
            .metadata(&binding.plugin)
            .map_err(|_| ApplicationError::Missing)?;
        Self::channel(definition)
    }

    fn channel(
        definition: PluginMetadata,
    ) -> Result<(PluginMetadata, ChannelDefinition), ApplicationError> {
        let channel = definition
            .channels
            .iter()
            .find(|channel| channel.platform == "wecom")
            .cloned()
            .ok_or_else(|| {
                ApplicationError::Invalid(
                    "the installed Plugin has no wecom channel declaration".into(),
                )
            })?;
        Ok((definition, channel))
    }

    fn credentials(
        &self,
        fact: &ChannelFact,
        definition: &PluginMetadata,
        channel: &ChannelDefinition,
    ) -> Result<BTreeMap<String, String>, ApplicationError> {
        if crate::credential_store::channel_authorization(definition, "wecom")
            .map_err(storage)?
            .1
            != fact.authorization_digest
        {
            return Err(ApplicationError::Conflict(
                "channel declarations changed; disable and configure the binding again".into(),
            ));
        }
        let mut values = crate::credential_store::channel_environment_values(
            &self.configuration.data_root,
            &crate::credential_store::binding_owner(&fact.owner),
            definition,
            "channel_wecom",
        )
        .map_err(storage)?;
        for field in &channel.required_environment {
            if let Some(value) = env::var_os(field) {
                let value = value.into_string().map_err(|_| {
                    ApplicationError::Invalid(format!(
                        "deployment channel credential {field} must be UTF-8"
                    ))
                })?;
                values.insert(field.clone(), value);
            }
        }
        Ok(values)
    }

    fn admission(&self, fact: &ChannelFact) -> Result<(), ApplicationError> {
        let bundle =
            FileGraphBundleLoader::new(self.configuration.catalog_root.join(&fact.binding.graph))
                .with_library_root(&self.configuration.library_root)
                .load()
                .map_err(|_| ApplicationError::Missing)?;
        if !bundle
            .snapshot
            .nodes
            .iter()
            .any(|node| node.id == fact.binding.reply_node)
            || !bundle.snapshot.nodes.iter().any(|node| {
                node.plugins
                    .iter()
                    .any(|plugin| plugin.id == fact.binding.plugin)
            })
        {
            return Err(ApplicationError::Invalid(
                "the channel Graph no longer contains the bound reply node and Plugin".into(),
            ));
        }
        crate::assistant::AssistantPlan::from_snapshot(&bundle.snapshot, &fact.binding.reply_node)
            .map_err(ApplicationError::Invalid)?
            .ok_or_else(|| {
                ApplicationError::Invalid(
                    crate::application::channel_management::NOT_AN_ASSISTANT.into(),
                )
            })?;
        Ok(())
    }

    async fn start(&self, fact: &ChannelFact) -> Result<ChannelSupervisor, ApplicationError> {
        self.admission(fact)?;
        let (definition, channel) = self.definition(&fact.binding)?;
        let credentials = self.credentials(fact, &definition, &channel)?;
        for field in &channel.required_environment {
            if credentials
                .get(field)
                .is_none_or(|value| value.trim().is_empty() || value.chars().any(char::is_control))
            {
                return Err(ApplicationError::Invalid(format!(
                    "channel credential {field} is not configured; use credential_request with server channel_wecom"
                )));
            }
        }
        ChannelSupervisor::start_managed(
            &self.configuration.catalog_root.join(&fact.binding.graph),
            &self.configuration.library_root,
            &self.configuration.data_root,
            &self.configuration.listen,
            ManagedChannelLaunch {
                plugin: fact.binding.plugin.clone(),
                platform: "wecom".into(),
                credentials,
                users: fact.binding.users.clone(),
                send_users: fact.binding.send_users.clone(),
                callback_key: fact.callback_key.clone(),
            },
        )
        .await
        .map_err(|_| storage("channel gateway could not start"))
    }

    async fn public_status(&self, owner: &str) -> Result<Value, ApplicationError> {
        if self.external {
            if owner != self.configuration.deployment_owner {
                return Ok(
                    json!({"platform":"wecom","configuration_owner":"deployment","configured":true,"owner_matches":false,"mutable":false}),
                );
            }
            let descriptor = self.external_descriptors.get("wecom");
            let listener = match descriptor {
                Some(descriptor) => serde_json::to_value(super::status::read(descriptor).await)
                    .map_err(|_| storage("channel status encoding failed"))?,
                None => json!("unavailable"),
            };
            return Ok(
                json!({"platform":"wecom","configuration_owner":"deployment","configured":self.configuration.deployment_binding.as_ref().is_some_and(|binding|!binding.graph.is_empty()),"binding":self.configuration.deployment_binding,"enabled":descriptor.is_some(),"listener":listener,"owner_matches":true,"mutable":false}),
            );
        }
        let Some(fact) = self.fact()? else {
            return Ok(
                json!({"platform":"wecom","configuration_owner":"host","configured":false,"enabled":false,"listener":"stopped","mutable":true}),
            );
        };
        if fact.owner != owner {
            return Ok(
                json!({"platform":"wecom","configuration_owner":"host","configured":true,"owner_matches":false,"mutable":false}),
            );
        }
        let admitted = self.admission(&fact).is_ok();
        let (available, credentials, account) = match self.metadata(&fact.binding) {
            Ok((definition, channel)) => {
                let values = self.credentials(&fact, &definition, &channel);
                let fields = channel.required_environment.iter().map(|field| json!({"field":field,"configured":values.as_ref().is_ok_and(|values|values.get(field).is_some_and(|value|!value.trim().is_empty())),"configuration_owner":if env::var_os(field).is_some(){"deployment"}else{"host"}})).collect::<Vec<_>>();
                let account = values.as_ref().ok().and_then(account_identity);
                (admitted && values.is_ok(), fields, account)
            }
            Err(_) => (false, Vec::new(), None),
        };
        let listener = if !fact.enabled {
            json!("stopped")
        } else if !available {
            json!("unavailable")
        } else {
            serde_json::to_value(
                super::status::read(
                    &self
                        .configuration
                        .data_root
                        .join("channels/wecom/control.json"),
                )
                .await,
            )
            .map_err(|_| storage("channel status encoding failed"))?
        };
        Ok(
            json!({"platform":"wecom","configuration_owner":"host","configured":true,"owner_matches":true,"mutable":true,
            "enabled":fact.enabled,"available":available,"binding":fact.binding,"credentials":credentials,"listener":listener,
            "account":account,"restart_required":fact.enabled && fact.account != account}),
        )
    }
}

impl ChannelManagementPort for ChannelManagementAdapter {
    fn status<'a>(
        &'a self,
        owner: &'a str,
        platform: &'a str,
    ) -> BoxFuture<'a, Result<Value, ApplicationError>> {
        Box::pin(async move {
            supported(platform)?;
            self.public_status(owner).await
        })
    }

    fn configure<'a>(
        &'a self,
        owner: &'a str,
        platform: &'a str,
        binding: ChannelBinding,
    ) -> BoxFuture<'a, Result<Value, ApplicationError>> {
        Box::pin(async move {
            supported(platform)?;
            let _supervisor = self.supervisor.lock().await;
            self.assert_managed_owner(owner)?;
            binding.validate()?;
            if self.fact()?.is_some_and(|fact| fact.enabled) {
                return Err(ApplicationError::Conflict(
                    "stop the channel before changing its binding".into(),
                ));
            }
            let (definition, _) = self.definition(&binding)?;
            let previous = self.fact()?;
            let digest = crate::credential_store::channel_authorization(&definition, "wecom")
                .map_err(storage)?
                .1;
            let changed = previous
                .as_ref()
                .is_none_or(|fact| fact.binding != binding || fact.authorization_digest != digest);
            if changed {
                let fact = ChannelFact {
                    format: 1,
                    owner: owner.into(),
                    binding,
                    enabled: false,
                    authorization_digest: digest,
                    callback_key: previous.map(|fact| fact.callback_key).unwrap_or(
                        random_token("anchor-channel-api")
                            .map_err(|_| storage("channel callback identity unavailable"))?,
                    ),
                    account: None,
                };
                self.admission(&fact)?;
                self.publish(fact)?;
            }
            let mut result = self.public_status(owner).await?;
            result["changed"] = json!(changed);
            Ok(result)
        })
    }

    fn set_enabled<'a>(
        &'a self,
        owner: &'a str,
        platform: &'a str,
        enabled: bool,
    ) -> BoxFuture<'a, Result<Value, ApplicationError>> {
        Box::pin(async move {
            supported(platform)?;
            let mut supervisor = self.supervisor.lock().await;
            self.assert_managed_owner(owner)?;
            let mut fact = self.fact()?.ok_or(ApplicationError::Missing)?;
            let changed = fact.enabled != enabled;
            if enabled {
                // Validate all credentials before saving an enabled intent.
                self.admission(&fact)?;
                let (definition, channel) = self.definition(&fact.binding)?;
                let credentials = self.credentials(&fact, &definition, &channel)?;
                if channel.required_environment.iter().any(|field| {
                    credentials.get(field).is_none_or(|value| {
                        value.trim().is_empty() || value.chars().any(char::is_control)
                    })
                }) {
                    return Err(ApplicationError::Invalid("channel credentials are missing; use the dedicated credential input before enabling".into()));
                }
                if changed {
                    fact.enabled = true;
                    fact.callback_key = random_token("anchor-channel-api")
                        .map_err(|_| storage("channel callback identity unavailable"))?;
                    fact.account = self.register_account(&fact.owner, &credentials)?;
                    self.publish(fact.clone())?;
                }
                if supervisor.is_none() {
                    *supervisor = Some(self.start(&fact).await?);
                }
            } else {
                if changed {
                    fact.enabled = false;
                    self.publish(fact.clone())?;
                }
                if let Some(mut active) = supervisor.take() {
                    active.shutdown().await;
                }
            }
            let mut result = self.public_status(owner).await?;
            result["changed"] = json!(changed);
            Ok(result)
        })
    }

    fn snapshot(&self, platform: &str) -> Result<Option<ChannelRuntimeState>, ApplicationError> {
        supported(platform)?;
        if self.external {
            return Ok(self
                .configuration
                .deployment_binding
                .as_ref()
                .map(|binding| ChannelRuntimeState {
                    owner: self.configuration.deployment_owner.clone(),
                    binding: binding.clone(),
                    enabled: self.external_descriptors.contains_key("wecom"),
                    account: self.configuration.deployment_account.clone(),
                    descriptor: self.external_descriptors.get("wecom").cloned(),
                }));
        }
        self.fact()?
            .map(|fact| {
                let enabled = fact.enabled
                    && self.admission(&fact).is_ok()
                    && self
                        .metadata(&fact.binding)
                        .is_ok_and(|(definition, channel)| {
                            self.credentials(&fact, &definition, &channel)
                                .is_ok_and(|values| {
                                    account_identity(&values) == fact.account
                                        && channel.required_environment.iter().all(|field| {
                                            values.get(field).is_some_and(|value| {
                                                !value.trim().is_empty()
                                                    && !value.chars().any(char::is_control)
                                            })
                                        })
                                })
                        });
                Ok(ChannelRuntimeState {
                    owner: fact.owner,
                    binding: fact.binding,
                    enabled,
                    account: fact.account,
                    descriptor: fact.enabled.then(|| {
                        self.configuration
                            .data_root
                            .join("channels/wecom/control.json")
                    }),
                })
            })
            .transpose()
    }

    fn callback_owner(&self, platform: &str, key: &str) -> Option<String> {
        if platform != "wecom" {
            return None;
        }
        if self.external {
            let expected = self.external_callback_keys.get(platform)?;
            // An explicitly supplied deployment API key retains its existing
            // API authority. Only Host-generated callback keys are narrowed.
            if !expected.starts_with("anchor-channel-api-") {
                return None;
            }
            return token_matches(expected, key)
                .then(|| self.configuration.deployment_owner.clone());
        }
        let fact = self.fact().ok()??;
        if !fact.enabled {
            return None;
        }
        token_matches(&fact.callback_key, key).then_some(fact.owner)
    }
}

fn supported(platform: &str) -> Result<(), ApplicationError> {
    if platform == "wecom" {
        Ok(())
    } else {
        Err(ApplicationError::Invalid(
            "the supported channel is wecom".into(),
        ))
    }
}
fn storage(reason: impl Into<String>) -> ApplicationError {
    ApplicationError::Storage(reason.into())
}
fn account_identity(credentials: &BTreeMap<String, String>) -> Option<String> {
    credentials
        .get("WECOM_BOT_ID")
        .map(|identity| wecom_account_identity(identity))
}

pub(crate) fn wecom_account_identity(identity: &str) -> String {
    format!("wecom:{:x}", Sha256::digest(identity.trim().as_bytes()))
}

fn register_account(
    configuration: &ChannelHostConfiguration,
    owner: &str,
    identity: &str,
) -> Result<String, String> {
    let account = wecom_account_identity(identity);
    crate::goose_acp::pilot::directory(&configuration.data_root.join("platform"))?;
    anchor_platform_session::SessionStore::open(
        configuration.data_root.join("platform/sessions.sqlite"),
    )
    .and_then(|sessions| {
        sessions.register_channel_account(
            owner,
            "wecom",
            &account,
            &[
                identity.to_owned(),
                identity.trim().to_owned(),
                format!("wecom:{:x}", Sha256::digest(identity.as_bytes())),
            ],
        )
    })
    .map_err(|_| "channel account identity registration failed".to_owned())?;
    Ok(account)
}

fn token_matches(expected: &str, key: &str) -> bool {
    let left = Sha256::digest(expected.as_bytes());
    let right = Sha256::digest(key.as_bytes());
    left.iter()
        .zip(right.iter())
        .fold(0u8, |difference, (left, right)| difference | (left ^ right))
        == 0
}

fn read_fact(root: &Path) -> Result<Option<ChannelFact>, String> {
    let file = match fs::OpenOptions::new()
        .read(true)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32)
        .open(root.join("wecom.json"))
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("channel configuration unavailable".into()),
    };
    let metadata = file
        .metadata()
        .map_err(|_| "channel configuration metadata unavailable")?;
    if !metadata.is_file()
        || metadata.len() > 512 * 1024
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.permissions().mode() & 0o777 != 0o600
    {
        return Err(
            "channel configuration requires a bounded Host-owned regular file with mode 600".into(),
        );
    }
    let mut bytes = Vec::new();
    file.take(512 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "channel configuration unavailable")?;
    if bytes.len() > 512 * 1024 {
        return Err("channel configuration exceeds the size limit".into());
    }
    let fact: ChannelFact =
        serde_json::from_slice(&bytes).map_err(|_| "channel configuration is invalid")?;
    fact.binding
        .validate()
        .map_err(|_| "channel configuration binding is invalid")?;
    if fact.format != 1
        || fact.owner.is_empty()
        || fact.owner.len() > 500
        || fact.owner.chars().any(char::is_control)
        || fact.authorization_digest.len() != 64
        || !fact
            .authorization_digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || !fact.callback_key.starts_with("anchor-channel-api-")
        || fact.callback_key.len() != 83
    {
        return Err("unsupported or invalid channel configuration identity".into());
    }
    Ok(Some(fact))
}

#[cfg(test)]
mod tests;
