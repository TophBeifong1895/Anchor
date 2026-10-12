//! The Pilot describes non-sensitive channel settings. Mutations are executed
//! only by the existing native confirmation adapter, never by model consent.
use super::*;
use crate::application::channel_management::ChannelBinding;

pub(crate) fn configuration_arguments(
    arguments: &Value,
) -> Result<(String, ChannelBinding), ToolError> {
    let object = arguments
        .as_object()
        .ok_or_else(|| rejected("channel arguments must be an object"))?;
    if object.keys().any(|key| {
        ![
            "platform",
            "graph",
            "reply_node",
            "plugin",
            "users",
            "send_users",
        ]
        .contains(&key.as_str())
    }) {
        return Err(rejected(
            "provide only the channel binding; credentials and approval are not model arguments",
        ));
    }
    let platform = parameter(arguments, "platform")?;
    let binding = ChannelBinding {
        graph: parameter(arguments, "graph")?,
        reply_node: parameter(arguments, "reply_node")?,
        plugin: parameter(arguments, "plugin")?,
        users: serde_json::from_value(
            arguments
                .get("users")
                .cloned()
                .ok_or_else(|| rejected("users must be explicitly provided"))?,
        )
        .map_err(|_| rejected("users must be a string array"))?,
        send_users: serde_json::from_value(
            arguments
                .get("send_users")
                .cloned()
                .ok_or_else(|| rejected("send_users must be explicitly provided"))?,
        )
        .map_err(|_| rejected("send_users must be a string array"))?,
    };
    if platform != "wecom" {
        return Err(rejected("the supported channel is wecom"));
    }
    binding.validate().map_err(channel_error)?;
    Ok((platform, binding.normalized()))
}

pub(crate) fn enable_arguments(arguments: &Value) -> Result<(String, bool), ToolError> {
    let object = arguments
        .as_object()
        .ok_or_else(|| rejected("channel arguments must be an object"))?;
    if object
        .keys()
        .any(|key| !["platform", "enabled"].contains(&key.as_str()))
    {
        return Err(rejected(
            "provide only platform and enabled; approval is not a model argument",
        ));
    }
    let platform = parameter(arguments, "platform")?;
    if platform != "wecom" {
        return Err(rejected("the supported channel is wecom"));
    }
    let enabled = arguments
        .get("enabled")
        .and_then(Value::as_bool)
        .ok_or_else(|| rejected("enabled must be a boolean"))?;
    Ok((platform, enabled))
}

impl PilotTools {
    pub(super) async fn channel_status(&self, arguments: &Value) -> Result<Value, ToolError> {
        let platform = parameter(arguments, "platform")?;
        if arguments
            .as_object()
            .is_none_or(|object| object.keys().any(|key| key != "platform"))
        {
            return Err(rejected("provide platform only"));
        }
        self.application
            .channel_status(&self.owner, &platform)
            .await
            .map_err(channel_error)
    }

    pub(crate) async fn configure_channel_approved(
        &self,
        platform: &str,
        binding: ChannelBinding,
    ) -> Result<Value, ToolError> {
        self.require_active_turn()?;
        self.application
            .configure_channel(&self.owner, platform, binding)
            .await
            .map_err(channel_error)
    }

    pub(crate) async fn enable_channel_approved(
        &self,
        platform: &str,
        enabled: bool,
        expected: &ChannelBinding,
    ) -> Result<Value, ToolError> {
        self.require_active_turn()?;
        self.application
            .set_channel_enabled_checked(&self.owner, platform, enabled, expected)
            .await
            .map_err(channel_error)
    }
}

fn channel_error(error: crate::application::ApplicationError) -> ToolError {
    match error {
        crate::application::ApplicationError::Missing => {
            rejected("channel Graph, Plugin, or binding is unavailable")
        }
        crate::application::ApplicationError::Invalid(reason)
        | crate::application::ApplicationError::Conflict(reason)
        | crate::application::ApplicationError::Storage(reason) => rejected(reason),
    }
}

#[cfg(test)]
mod tests;
