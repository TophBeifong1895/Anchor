//! Host-owned channel configuration; the existing Session/Turn admission and
//! ChannelSupervisor retain execution and transport semantics.
use super::{ApplicationError, RunApplication};
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeSet, path::PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChannelBinding {
    pub(crate) graph: String,
    pub(crate) reply_node: String,
    pub(crate) plugin: String,
    pub(crate) users: Vec<String>,
    pub(crate) send_users: Vec<String>,
}

impl ChannelBinding {
    pub(crate) fn validate(&self) -> Result<(), ApplicationError> {
        if !identifier(&self.graph)
            || !identifier(&self.plugin)
            || self.reply_node.trim().is_empty()
            || self.reply_node.len() > 200
            || self.reply_node.chars().any(char::is_control)
        {
            return Err(ApplicationError::Invalid("invalid channel binding".into()));
        }
        for users in [&self.users, &self.send_users] {
            if users.len() > 1000
                || users.iter().any(|user| {
                    user.trim().is_empty()
                        || user.len() > 200
                        || user.contains(',')
                        || user.chars().any(char::is_control)
                })
            {
                return Err(ApplicationError::Invalid(
                    "invalid channel user authorization".into(),
                ));
            }
        }
        // GatewayConfig's empty outbound list inherits inbound users. Make that
        // existing behavior explicit in the approved binding rather than save
        // an empty list that misleadingly appears to prohibit outbound replies.
        if self.send_users.is_empty() && !self.users.is_empty() {
            return Err(ApplicationError::Invalid(
                "send_users must explicitly authorize the intended reply users".into(),
            ));
        }
        Ok(())
    }

    pub(crate) fn normalized(mut self) -> Self {
        self.users = self
            .users
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        self.send_users = self
            .send_users
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        self
    }
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

/// Internal routing data. Credentials and callback tokens are deliberately not
/// part of this public projection; the adapter retains them privately.
#[derive(Clone)]
pub(crate) struct ChannelRuntimeState {
    pub(crate) owner: String,
    pub(crate) binding: ChannelBinding,
    pub(crate) enabled: bool,
    pub(crate) account: Option<String>,
    pub(crate) descriptor: Option<PathBuf>,
}

pub(crate) trait ChannelManagementPort: Send + Sync {
    fn status<'a>(
        &'a self,
        owner: &'a str,
        platform: &'a str,
    ) -> BoxFuture<'a, Result<Value, ApplicationError>>;
    fn configure<'a>(
        &'a self,
        owner: &'a str,
        platform: &'a str,
        binding: ChannelBinding,
    ) -> BoxFuture<'a, Result<Value, ApplicationError>>;
    fn set_enabled<'a>(
        &'a self,
        owner: &'a str,
        platform: &'a str,
        enabled: bool,
    ) -> BoxFuture<'a, Result<Value, ApplicationError>>;
    fn snapshot(&self, platform: &str) -> Result<Option<ChannelRuntimeState>, ApplicationError>;
    fn callback_owner(&self, platform: &str, key: &str) -> Option<String>;
}

fn platform_supported(platform: &str) -> Result<(), ApplicationError> {
    if platform != "wecom" {
        return Err(ApplicationError::Invalid(
            "this Host supports the wecom channel".into(),
        ));
    }
    Ok(())
}

impl RunApplication {
    pub(crate) fn ensure_channel_session_eligible(
        &self,
        owner: &str,
        session: &str,
    ) -> Result<(), ApplicationError> {
        let sessions = anchor_platform_session::SessionStore::open(
            self.data_root.join("platform/sessions.sqlite"),
        )
        .map_err(super::assistants::assistant_session_error)?;
        if !sessions
            .channel_session_is_selected(owner, session)
            .map_err(super::assistants::assistant_session_error)?
        {
            return Err(ApplicationError::Conflict(
                "this channel Session is superseded by its canonical account Session".into(),
            ));
        }
        let Some(port) = self.channels.as_deref() else {
            return Ok(());
        };
        let saved = sessions
            .get(owner, session)
            .map_err(super::assistants::assistant_session_error)?;
        let source = saved.channel.get("source").ok_or_else(|| {
            ApplicationError::Conflict("channel Session has no platform identity".into())
        })?;
        let current = port.snapshot(source)?.ok_or_else(|| {
            ApplicationError::Conflict("channel Session no longer has a deployment binding".into())
        })?;
        let canonical = |account: &str| {
            sessions
                .canonical_channel_account(owner, source, account)
                .map_err(super::assistants::assistant_session_error)
        };
        let saved_account = canonical(saved.channel.get("account").map_or("", String::as_str))?;
        let current_account = canonical(current.account.as_deref().unwrap_or(""))?;
        if current.owner != owner
            || !current.enabled
            || saved.graph != current.binding.graph
            || saved.reply_node != current.binding.reply_node
            || saved_account != current_account
        {
            return Err(ApplicationError::Conflict(
                "channel Session does not match an enabled deployment binding".into(),
            ));
        }
        Ok(())
    }

    pub(crate) fn channel_management(
        &self,
    ) -> Result<&dyn ChannelManagementPort, ApplicationError> {
        self.channels.as_deref().ok_or_else(|| {
            ApplicationError::Invalid("this Host does not provide channel management".into())
        })
    }

    pub(crate) async fn channel_status(
        &self,
        owner: &str,
        platform: &str,
    ) -> Result<Value, ApplicationError> {
        platform_supported(platform)?;
        self.channel_management()?.status(owner, platform).await
    }

    pub(crate) async fn configure_channel(
        &self,
        owner: &str,
        platform: &str,
        binding: ChannelBinding,
    ) -> Result<Value, ApplicationError> {
        platform_supported(platform)?;
        binding.validate()?;
        let port = self.channel_management()?;
        let _catalog_guard = self.graph_catalog_mutation_guard().await;
        let _library_lease = self.plugin_reference_lease().await?;
        self.validate_channel_graph(&binding)?;
        port.configure(owner, platform, binding.normalized()).await
    }

    #[cfg(test)]
    pub(crate) async fn set_channel_enabled(
        &self,
        owner: &str,
        platform: &str,
        enabled: bool,
    ) -> Result<Value, ApplicationError> {
        self.set_channel_enabled_inner(owner, platform, enabled, None)
            .await
    }

    pub(crate) async fn set_channel_enabled_checked(
        &self,
        owner: &str,
        platform: &str,
        enabled: bool,
        expected: &ChannelBinding,
    ) -> Result<Value, ApplicationError> {
        self.set_channel_enabled_inner(owner, platform, enabled, Some(expected))
            .await
    }

    async fn set_channel_enabled_inner(
        &self,
        owner: &str,
        platform: &str,
        enabled: bool,
        expected: Option<&ChannelBinding>,
    ) -> Result<Value, ApplicationError> {
        platform_supported(platform)?;
        let port = self.channel_management()?;
        let _catalog_guard = self.graph_catalog_mutation_guard().await;
        let _library_lease = self.plugin_reference_lease().await?;
        if let Some(expected) = expected
            && port
                .snapshot(platform)?
                .is_none_or(|state| state.owner != owner || &state.binding != expected)
        {
            return Err(ApplicationError::Conflict(
                "channel binding changed while confirmation was pending; request a fresh form"
                    .into(),
            ));
        }
        if enabled {
            let state = port.snapshot(platform)?.ok_or(ApplicationError::Missing)?;
            self.validate_channel_graph(&state.binding)?;
        }
        // Disabling remains possible after a Graph or Plugin has disappeared.
        port.set_enabled(owner, platform, enabled).await
    }

    fn validate_channel_graph(&self, binding: &ChannelBinding) -> Result<(), ApplicationError> {
        let (_, graph) = self
            .load_graph(&binding.graph)
            .map_err(|_| ApplicationError::Missing)?;
        if !graph
            .snapshot
            .nodes
            .iter()
            .any(|node| node.id == binding.reply_node)
            || !graph.snapshot.nodes.iter().any(|node| {
                node.plugins
                    .iter()
                    .any(|reference| reference.id == binding.plugin)
            })
        {
            return Err(ApplicationError::Invalid(
                "channel Graph must contain its reply node and reference the channel Plugin".into(),
            ));
        }
        Ok(())
    }

    pub(crate) fn channel_snapshot(
        &self,
        platform: &str,
    ) -> Result<Option<ChannelRuntimeState>, ApplicationError> {
        platform_supported(platform)?;
        match self.channels.as_deref() {
            Some(port) => port.snapshot(platform),
            None => Ok(None),
        }
    }

    pub(crate) fn channel_callback_owner(&self, platform: &str, key: &str) -> Option<String> {
        self.channels.as_deref()?.callback_owner(platform, key)
    }
}

#[cfg(test)]
mod tests;
