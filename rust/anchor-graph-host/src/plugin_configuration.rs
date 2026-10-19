//! Plugin-owned configuration requirements and explicit MCP injection bindings.
//! Values belong to the Host; this declaration never contains secret defaults.

use crate::mcp::{
    McpServerDefinition, host_environment_references, host_path_environment_key,
    valid_environment_key,
};
use crate::resources::valid_reference;
use crate::{GraphError, invalid};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginConfigurationFieldType {
    String,
    Url,
    Secret,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginConfigurationField {
    #[serde(rename = "type")]
    pub kind: PluginConfigurationFieldType,
    #[serde(default)]
    pub label: String,
    #[serde(default = "required_by_default")]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

fn required_by_default() -> bool {
    true
}

impl PluginConfigurationField {
    pub fn is_secret(&self) -> bool {
        self.kind == PluginConfigurationFieldType::Secret
    }

    /// Shared validation for declaration defaults and Host configuration writes.
    /// Errors deliberately contain no supplied value.
    pub fn validate_value(&self, value: &str) -> Result<(), GraphError> {
        if value.len() > 65_536 || value.contains('\0') {
            return Err(invalid("Plugin configuration value is invalid".into()));
        }
        if value.trim().is_empty() {
            return if self.required {
                Err(invalid("Plugin configuration value is required".into()))
            } else {
                Ok(())
            };
        }
        if self.kind == PluginConfigurationFieldType::Url && !http_url_is_valid(value) {
            return Err(invalid(
                "Plugin configuration URL must be HTTP(S), without embedded credentials or fragment"
                    .into(),
            ));
        }
        Ok(())
    }
}

/// One owner-bound default connection per Plugin. Stable field identifiers are
/// independent of the environment names consumed by each MCP implementation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginConfiguration {
    #[serde(default)]
    pub fields: BTreeMap<String, PluginConfigurationField>,
    #[serde(default)]
    pub environment: BTreeMap<String, BTreeMap<String, String>>,
}

impl PluginConfiguration {
    pub fn environment_for(&self, server: &str) -> Option<&BTreeMap<String, String>> {
        self.environment.get(server)
    }

    pub fn is_empty(&self) -> bool {
        self.fields.is_empty() && self.environment.is_empty()
    }

    pub(crate) fn parse(
        id: &str,
        manifest: &Map<String, Value>,
        servers: &[McpServerDefinition],
    ) -> Result<Self, GraphError> {
        let Some(value) = manifest.get("configuration") else {
            return Ok(Self::default());
        };
        let mut declaration: Self = serde_json::from_value(value.clone())
            .map_err(|_| invalid(format!("Plugin {id}: invalid configuration declaration")))?;
        for (name, field) in &mut declaration.fields {
            if !valid_reference(name) || name.len() > 200 {
                return Err(invalid(format!(
                    "Plugin {id}: invalid configuration field identifier"
                )));
            }
            if field.label.is_empty() {
                field.label.clone_from(name);
            }
            if field.label.len() > 500 || field.label.chars().any(char::is_control) {
                return Err(invalid(format!(
                    "Plugin {id}: invalid configuration field label"
                )));
            }
            if let Some(value) = &field.default {
                if field.is_secret() {
                    return Err(invalid(format!(
                        "Plugin {id}: secret configuration fields cannot declare defaults"
                    )));
                }
                field.validate_value(value)?;
            }
        }
        let mut used = BTreeSet::new();
        for (target, mappings) in &declaration.environment {
            let server = servers
                .iter()
                .find(|server| server.name == *target)
                .ok_or_else(|| {
                    invalid(format!(
                        "Plugin {id}: configuration references an unknown MCP target"
                    ))
                })?;
            let consumed = mcp_environment_fields(&server.config);
            for (environment, field) in mappings {
                if host_path_environment_key(environment) {
                    return Err(invalid(format!(
                        "Plugin {id}: configuration cannot bind Host-reserved environment variables"
                    )));
                }
                if !valid_environment_key(environment)
                    || !consumed.contains(environment)
                    || !declaration.fields.contains_key(field)
                {
                    return Err(invalid(format!(
                        "Plugin {id}: configuration contains an invalid environment mapping"
                    )));
                }
                if declaration.fields[field].is_secret()
                    && host_environment_references(&server.config).contains(environment)
                {
                    return Err(invalid(format!(
                        "Plugin {id}: secret configuration fields must use credential selectors without templates"
                    )));
                }
                used.insert(field);
            }
        }
        for server in servers {
            for environment in host_environment_references(&server.config) {
                if !declaration
                    .environment_for(&server.name)
                    .is_some_and(|mappings| mappings.contains_key(&environment))
                {
                    return Err(invalid(format!(
                        "Plugin {id}: MCP templates require explicit non-secret configuration bindings"
                    )));
                }
            }
        }
        if declaration.fields.keys().any(|field| !used.contains(field)) {
            return Err(invalid(format!(
                "Plugin {id}: configuration fields must have an explicit MCP environment binding"
            )));
        }
        Ok(declaration)
    }

    /// Bind configuration requirements and selectors to Host authority. Labels
    /// are presentation only, while default endpoints affect the destination.
    /// Preserve existing authorization identities for legacy Plugins.
    pub(crate) fn authorization_digest(&self, legacy: String) -> String {
        if self.is_empty() {
            return legacy;
        }
        let fields: BTreeMap<_, _> = self
            .fields
            .iter()
            .map(|(name, field)| {
                (
                    name,
                    serde_json::json!({
                        "type": field.kind,
                        "required": field.required,
                        "default": field.default,
                    }),
                )
            })
            .collect();
        let bytes = serde_json::to_vec(&serde_json::json!({
            "format": 1,
            "plugin_authority": legacy,
            "fields": fields,
            "environment": self.environment,
        }))
        .expect("configuration authority serialization is infallible");
        format!("{:x}", Sha256::digest(bytes))
    }
}

/// Host environment names consumed by templates, stdio injection and HTTP
/// authorization. Catalog validation and Host management share this discovery.
pub fn mcp_environment_fields(config: &Value) -> BTreeSet<String> {
    let mut fields = host_environment_references(config);
    for key in ["env_vars", "optional_env_vars"] {
        fields.extend(
            config
                .get(key)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned),
        );
    }
    if let Some(field) = config.get("bearer_token_env_var").and_then(Value::as_str) {
        fields.insert(field.into());
    }
    fields.extend(
        config
            .get("env_http_headers")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|values| values.values())
            .filter_map(Value::as_str)
            .map(str::to_owned),
    );
    fields
}

pub(crate) fn http_url_is_valid(value: &str) -> bool {
    let Ok(url) = url::Url::parse(value) else {
        return false;
    };
    let authority = value
        .split_once("://")
        .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or_default());
    matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && !authority.is_some_and(|authority| authority.contains('@'))
        && url.fragment().is_none()
        && !value
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
        && !value.contains('\\')
}

#[cfg(test)]
#[path = "plugin_configuration_tests.rs"]
mod tests;
