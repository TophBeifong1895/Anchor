//! Host-owned, owner-bound default Plugin connections. Logical fields have one
//! atomic authority even when several MCP servers consume the same secret.
use anchor_graph_host::{FilePluginCatalog, PluginDefinition};
use anchor_platform_session::SessionStore;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[cfg(test)]
mod tests;

const EXTERNAL_CONFIGURATION: &str = "ANCHOR_PLUGIN_EXTERNAL_CONFIGURATION";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredField {
    authorization_digest: String,
    value: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigurationFact {
    format: u32,
    owner: String,
    plugin: String,
    revision: u64,
    fields: BTreeMap<String, StoredField>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConfigurationUpdate {
    pub(crate) authorization_digest: String,
    pub(crate) revision: u64,
    pub(crate) fields: BTreeMap<String, FieldUpdate>,
}
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum FieldUpdate {
    Set { value: String },
    Clear,
}

/// Private execution material. Never serialize or debug-print this snapshot.
pub(crate) struct ConfigurationSnapshot {
    pub(crate) revision: u64,
    /// Non-secret recovery identity. Secret values and their hashes are excluded,
    /// so rotating a Key preserves the connection reference within an invocation.
    pub(crate) binding_digest: String,
    pub(crate) environments: BTreeMap<String, BTreeMap<String, String>>,
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConfigurationVersion {
    pub(crate) revision: u64,
    pub(crate) binding_digest: String,
}

fn definition(library: &Path, plugin: &str) -> Result<PluginDefinition, String> {
    if !crate::pilot_tools::identifier(plugin) {
        return Err("invalid Plugin configuration ID".into());
    }
    FilePluginCatalog::new(library)
        .definition(plugin)
        .map_err(|_| "Plugin configuration target unavailable".into())
}
fn path(data: &Path, owner: &str, plugin: &str) -> PathBuf {
    data.join("platform/credentials")
        .join(crate::credential_store::hash(owner.as_bytes()))
        .join(plugin)
        .join("configuration.json")
}
fn read(
    data: &Path,
    owner: &str,
    definition: &PluginDefinition,
) -> Result<ConfigurationFact, String> {
    let fact =
        crate::credential_store::load::<ConfigurationFact>(&path(data, owner, &definition.id))?
            .unwrap_or_else(|| ConfigurationFact {
                format: 1,
                owner: owner.into(),
                plugin: definition.id.clone(),
                revision: 0,
                fields: BTreeMap::new(),
            });
    if fact.format != 1 || fact.owner != owner || fact.plugin != definition.id {
        return Err("stored Plugin configuration identity is invalid".into());
    }
    Ok(fact)
}

/// Deployment ownership is explicit. Merely inheriting an environment variable
/// never grants an MCP access to that value. Legacy targets use `server.ENV`.
fn external_declarations() -> Result<BTreeMap<String, BTreeMap<String, String>>, String> {
    let Some(value) = std::env::var_os(EXTERNAL_CONFIGURATION) else {
        return Ok(BTreeMap::new());
    };
    serde_json::from_str(
        value
            .to_str()
            .ok_or("external Plugin configuration is invalid")?,
    )
    .map_err(|_| "external Plugin configuration is invalid".into())
}
fn external_fields(definition: &PluginDefinition) -> Result<BTreeMap<String, String>, String> {
    let declared = external_declarations()?
        .remove(&definition.id)
        .unwrap_or_default();
    for (field, environment) in &declared {
        let valid = if definition.configuration.fields.contains_key(field) {
            definition
                .configuration
                .environment
                .values()
                .any(|mapping| mapping.get(environment) == Some(field))
        } else if let Some((target, environment_field)) = field.rsplit_once('.') {
            environment == environment_field
                && definition.mcp_servers.iter().any(|server| {
                    server.name == target
                        && crate::credential_store::declared_fields(&server.config)
                            .contains(environment)
                })
                && logical_field(definition, target, environment).is_none()
        } else {
            false
        };
        if !valid {
            return Err(
                "external Plugin configuration must reference a declared environment binding"
                    .into(),
            );
        }
    }
    Ok(declared)
}
pub(crate) fn legacy_external_environment(
    definition: &PluginDefinition,
    target: &str,
    field: &str,
) -> Result<Option<String>, String> {
    Ok(external_fields(definition)?.remove(&format!("{target}.{field}")))
}
pub(crate) fn logical_field<'a>(
    definition: &'a PluginDefinition,
    target: &str,
    environment: &str,
) -> Option<&'a str> {
    definition
        .configuration
        .environment_for(target)?
        .get(environment)
        .map(String::as_str)
}
pub(crate) fn is_external(
    definition: &PluginDefinition,
    target: &str,
    field: &str,
) -> Result<bool, String> {
    if target.starts_with("channel_") {
        // A bot's credentials belong to its Graph binding; the process
        // environment never supplies them.
        return Ok(false);
    }
    let key = logical_field(definition, target, field)
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{target}.{field}"));
    Ok(external_fields(definition)?.contains_key(&key))
}

/// Normalize only a root origin's optional trailing slash. Do not rewrite
/// application paths, queries, duplicate slashes, escapes, ports or casing.
/// The same effective value is used for projection, injection and comparison.
fn normalized_value(
    declaration: &anchor_graph_host::PluginConfigurationField,
    value: &str,
) -> String {
    if declaration.kind == anchor_graph_host::PluginConfigurationFieldType::Url
        && value.ends_with('/')
        && value.split_once("://").is_some_and(|(_, rest)| {
            rest.strip_suffix('/')
                .is_some_and(|authority| !authority.contains('/'))
        })
        && url::Url::parse(value)
            .is_ok_and(|url| url.path() == "/" && url.query().is_none() && url.fragment().is_none())
    {
        value[..value.len() - 1].to_owned()
    } else {
        value.to_owned()
    }
}

struct ResolvedField {
    state: &'static str,
    external: bool,
    value: Option<String>,
}
fn resolve_fields(
    definition: &PluginDefinition,
    fact: &ConfigurationFact,
) -> Result<BTreeMap<String, ResolvedField>, String> {
    let external = external_fields(definition)?;
    let mut fields = BTreeMap::new();
    for (id, declaration) in &definition.configuration.fields {
        let (state, value, external) = if let Some(environment) = external.get(id) {
            let value = std::env::var(environment).ok();
            if let Some(value) = &value {
                declaration
                    .validate_value(value)
                    .map_err(|_| "external Plugin configuration value is invalid")?;
            }
            (
                if value.is_some() {
                    "deployment"
                } else {
                    "missing"
                },
                value,
                true,
            )
        } else if let Some(stored) = fact.fields.get(id) {
            if stored.authorization_digest.as_str() != definition.authorization_digest.as_ref() {
                ("stale", None, false)
            } else {
                if let Some(value) = &stored.value {
                    declaration
                        .validate_value(value)
                        .map_err(|_| "stored Plugin configuration value is invalid")?;
                }
                (
                    if stored.value.is_some() {
                        "configured"
                    } else {
                        "missing"
                    },
                    stored.value.clone(),
                    false,
                )
            }
        } else {
            (
                if declaration.default.is_some() {
                    "configured"
                } else {
                    "missing"
                },
                declaration.default.clone(),
                false,
            )
        };
        fields.insert(
            id.clone(),
            ResolvedField {
                state,
                external,
                value: value.map(|value| normalized_value(declaration, &value)),
            },
        );
    }
    Ok(fields)
}
fn project(data: &Path, owner: &str, definition: &PluginDefinition) -> Result<Value, String> {
    let fact = read(data, owner, definition)?;
    let resolved = resolve_fields(definition, &fact)?;
    let fields = definition
        .configuration
        .fields
        .iter()
        .map(|(id, declaration)| {
            let field = &resolved[id];
            let mut record = json!({"id":id,"label":declaration.label,"type":declaration.kind,
            "required":declaration.required,"state":field.state,
            "source":if field.external {"external"} else {"managed"},"editable":!field.external});
            if !declaration.is_secret()
                && let Some(value) = &field.value
            {
                record["value"] = json!(value);
            }
            record
        })
        .collect::<Vec<_>>();
    let targets = definition.configuration.environment.keys().filter_map(|target| {
        definition.mcp_servers.iter().find(|server| &server.name == target).map(|server|
            json!({"target":target,"transport":if server.config.get("command").is_some(){"stdio"}else{server.config.get("type").and_then(Value::as_str).unwrap_or("http")}}))
    }).collect::<Vec<_>>();
    Ok(
        json!({"plugin":definition.id,"authorization_digest":definition.authorization_digest,
        "revision":fact.revision,"fields":fields,"targets":targets}),
    )
}
pub(crate) fn configuration(
    data: &Path,
    library: &Path,
    owner: &str,
    plugin: &str,
) -> Result<Value, String> {
    let definition = definition(library, plugin)?;
    let _lease = crate::credential_store::lease(data)?;
    project(
        data,
        &crate::credential_store::binding_owner(owner),
        &definition,
    )
}

/// Caller holds the credentials lease. Owner is the runtime binding owner.
pub(crate) fn snapshot_locked(
    data: &Path,
    owner: &str,
    definition: &PluginDefinition,
) -> Result<ConfigurationSnapshot, crate::tool_host::PreparationError> {
    use crate::tool_host::PreparationError;
    let fact = read(data, owner, definition)?;
    let fields = resolve_fields(definition, &fact)?;
    for (id, field) in &fields {
        if field.state == "stale" {
            return Err(PreparationError::KnownFailure(
                "Plugin configuration authorization changed; configure it again".into(),
            ));
        }
        if definition.configuration.fields[id].required && field.value.is_none() {
            return Err(PreparationError::KnownFailure(
                "required Plugin configuration is missing; configure the Plugin in the Web UI"
                    .into(),
            ));
        }
    }
    let environments = definition
        .configuration
        .environment
        .iter()
        .map(|(server, mapping)| {
            let values = mapping
                .iter()
                .filter_map(|(environment, field)| {
                    fields[field]
                        .value
                        .as_ref()
                        .map(|value| (environment.clone(), value.clone()))
                })
                .collect();
            (server.clone(), values)
        })
        .collect();
    Ok(ConfigurationSnapshot {
        revision: fact.revision,
        binding_digest: binding_digest(owner, definition, &fields)?,
        environments,
    })
}
/// Bind recovery to the connection identity rather than a credential version.
/// Only the Host owner, declaration authority, explicit source selectors and
/// non-secret values may influence this durable observation.
fn binding_digest(
    owner: &str,
    definition: &PluginDefinition,
    resolved: &BTreeMap<String, ResolvedField>,
) -> Result<String, String> {
    let external = external_fields(definition)?;
    let fields = definition.configuration.fields.iter().map(|(id, declaration)| {
        let mut record = json!({"source": if external.contains_key(id) {"external"} else {"managed"},
            "selector":external.get(id)});
        if !declaration.is_secret() { record["value"] = json!(resolved[id].value); }
        (id.clone(), record)
    }).collect::<BTreeMap<_, _>>();
    let legacy = definition.mcp_servers.iter().map(|server| {
        let fields = crate::credential_store::declared_fields(&server.config).into_iter()
            .filter(|field| logical_field(definition, &server.name, field).is_none())
            .map(|field| {
                let selector = external.get(&format!("{}.{}", server.name, field));
                (field, json!({"source":if selector.is_some(){"external"}else{"managed"},"selector":selector}))
            }).collect::<BTreeMap<_, _>>();
        (server.name.clone(), fields)
    }).collect::<BTreeMap<_, _>>();
    let bytes = serde_json::to_vec(&json!({"format":1,"owner":owner,"plugin":definition.id,
        "authorization_digest":definition.authorization_digest,"fields":fields,"legacy":legacy}))
    .map_err(|_| "Plugin configuration binding unavailable")?;
    Ok(crate::credential_store::hash(&bytes))
}

pub(crate) fn request_version_locked(
    data: &Path,
    owner: &str,
    definition: &PluginDefinition,
) -> Result<ConfigurationVersion, String> {
    let fact = read(data, owner, definition)?;
    let resolved = resolve_fields(definition, &fact)?;
    Ok(ConfigurationVersion {
        revision: fact.revision,
        binding_digest: binding_digest(owner, definition, &resolved)?,
    })
}
#[cfg(test)]
pub(crate) fn snapshot(
    data: &Path,
    owner: &str,
    definition: &PluginDefinition,
) -> Result<ConfigurationSnapshot, String> {
    let _lease = crate::credential_store::lease(data)?;
    snapshot_locked(data, owner, definition).map_err(|error| error.to_string())
}

fn write_locked(
    data: &Path,
    owner: &str,
    definition: &PluginDefinition,
    update: &ConfigurationUpdate,
) -> Result<u64, String> {
    if update.authorization_digest.as_str() != definition.authorization_digest.as_ref() {
        return Err("Plugin authorization changed; reload configuration before saving".into());
    }
    let mut fact = read(data, owner, definition)?;
    if update.revision != fact.revision {
        return Err("Plugin configuration changed; reload before saving".into());
    }
    let external = external_fields(definition)?;
    for (id, change) in &update.fields {
        let declaration = definition
            .configuration
            .fields
            .get(id)
            .ok_or("Plugin configuration field is not declared")?;
        if external.contains_key(id) {
            return Err(
                "Plugin configuration field is managed externally by the deployment".into(),
            );
        }
        if let FieldUpdate::Set { value } = change {
            declaration
                .validate_value(value)
                .map_err(|_| "Plugin configuration value is invalid")?;
        }
    }
    if update.fields.is_empty() {
        return Ok(fact.revision);
    }
    let resolved = resolve_fields(definition, &fact)?;
    let normalized_updates = update
        .fields
        .iter()
        .map(|(id, change)| {
            let value = match change {
                FieldUpdate::Set { value } => Some(normalized_value(
                    &definition.configuration.fields[id],
                    value,
                )),
                FieldUpdate::Clear => None,
            };
            (id.clone(), value)
        })
        .collect::<BTreeMap<_, _>>();
    let identity_changed = normalized_updates.iter().any(|(id, new_value)| {
        !definition.configuration.fields[id].is_secret() && &resolved[id].value != new_value
    });
    if identity_changed
        && definition
            .configuration
            .fields
            .iter()
            .any(|(id, field)| field.is_secret() && external.contains_key(id))
    {
        return Err("Plugin connection changes require deployment-managed credentials to be reauthorized externally".into());
    }
    if identity_changed
        && crate::credential_store::has_legacy_mcp_values_locked(data, owner, definition)?
    {
        return Err(
            "clear legacy MCP credentials before changing Plugin connection configuration".into(),
        );
    }
    for (id, value) in normalized_updates {
        fact.fields.insert(
            id,
            StoredField {
                authorization_digest: definition.authorization_digest.to_string(),
                value,
            },
        );
    }
    if identity_changed {
        for (id, declaration) in &definition.configuration.fields {
            if declaration.is_secret()
                && !matches!(update.fields.get(id), Some(FieldUpdate::Set { .. }))
            {
                fact.fields.insert(
                    id.clone(),
                    StoredField {
                        authorization_digest: definition.authorization_digest.to_string(),
                        value: None,
                    },
                );
            }
        }
    }
    fact.revision = fact
        .revision
        .checked_add(1)
        .ok_or("Plugin configuration revision exhausted")?;
    save_fact(data, owner, definition, &fact)?;
    Ok(fact.revision)
}
fn save_fact(
    data: &Path,
    owner: &str,
    definition: &PluginDefinition,
    fact: &ConfigurationFact,
) -> Result<(), String> {
    if serde_json::to_vec(fact)
        .map_err(|_| "Plugin configuration encoding failed")?
        .len()
        > 128 * 1024
    {
        return Err("Plugin configuration exceeds the private storage limit".into());
    }
    crate::credential_store::save(&path(data, owner, &definition.id), fact)
}
/// Legacy MCP credentials participate in the same connection version even when
/// a Plugin has not declared logical fields. Invalidate before publishing the
/// secret: a failed write may conservatively advance the version, but must never
/// leave a changed credential associated with a previously tested version.
/// The caller holds the credentials lease throughout both private writes.
pub(crate) fn invalidate_legacy_locked(
    data: &Path,
    owner: &str,
    definition: &PluginDefinition,
) -> Result<(), String> {
    let mut fact = read(data, owner, definition)?;
    fact.revision = fact
        .revision
        .checked_add(1)
        .ok_or("Plugin configuration revision exhausted")?;
    save_fact(data, owner, definition, &fact)
}
/// Compatibility adapters call the same logical-field mutation under one lease.
pub(crate) fn set_legacy_locked(
    data: &Path,
    owner: &str,
    definition: &PluginDefinition,
    field: &str,
    value: Option<&str>,
) -> Result<(), String> {
    let fact = read(data, owner, definition)?;
    write_locked(
        data,
        owner,
        definition,
        &ConfigurationUpdate {
            authorization_digest: definition.authorization_digest.to_string(),
            revision: fact.revision,
            fields: BTreeMap::from([(
                field.into(),
                match value {
                    Some(value) => FieldUpdate::Set {
                        value: value.into(),
                    },
                    None => FieldUpdate::Clear,
                },
            )]),
        },
    )?;
    Ok(())
}
pub(crate) fn saved_value_locked(
    data: &Path,
    owner: &str,
    definition: &PluginDefinition,
    field: &str,
) -> Result<Option<String>, String> {
    let fact = read(data, owner, definition)?;
    Ok(fact
        .fields
        .get(field)
        .filter(|stored| {
            stored.authorization_digest.as_str() == definition.authorization_digest.as_ref()
        })
        .and_then(|stored| stored.value.clone()))
}
pub(crate) fn configure(
    data: &Path,
    library: &Path,
    owner: &str,
    plugin: &str,
    update: ConfigurationUpdate,
    sessions: &SessionStore,
) -> Result<Value, String> {
    let definition = definition(library, plugin)?;
    let _lease = crate::credential_store::lease(data)?;
    let bound_owner = crate::credential_store::binding_owner(owner);
    write_locked(data, &bound_owner, &definition, &update)?;
    let mut warning = false;
    for (target, mappings) in &definition.configuration.environment {
        for (environment, field) in mappings {
            if matches!(update.fields.get(field), Some(FieldUpdate::Set { .. })) {
                warning |= crate::credential_store::resolve_pending_locked(
                    data,
                    sessions,
                    owner,
                    &definition,
                    target,
                    environment,
                    &definition.authorization_digest,
                )
                .is_err();
            }
        }
    }
    let mut projection = project(data, &bound_owner, &definition)?;
    if warning {
        projection["warning"] =
            json!("配置已保存，但部分会话配置请求未能同步；请重新读取会话状态后核查。");
    }
    Ok(projection)
}
