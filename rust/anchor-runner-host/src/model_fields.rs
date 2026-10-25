//! Typed public model fields shared by settings and execution.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

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
    pub(crate) fn provider_id(&self) -> String {
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
