//! Native user approval for channel binding and transport lifecycle. Secret
//! values use credential requests; this form contains only explicit grants.
use super::*;

pub(super) async fn policy(
    tools: &PilotTools,
    name: &str,
    arguments: &serde_json::Map<String, Value>,
    peer: Peer<RoleServer>,
    cancellation: &Cancellation,
) -> Result<Vec<ToolResultContent>, ToolError> {
    active(tools, cancellation)?;
    let arguments = Value::Object(arguments.clone());
    let (message, binding, enable) = match name {
        "channel_configure" => {
            let (platform, binding) =
                crate::pilot_tools::channel_management::configuration_arguments(&arguments)?;
            // Load the graph before the question; the approved reference and
            // reply node are then checked again by the application use case.
            tools
                .application
                .load_graph(&binding.graph)
                .map_err(|error| failure(format!("channel Graph unavailable: {error}")))?;
            let message = format!(
                "配置 {platform} 渠道：Graph {}，回复节点 {}，已安装 Plugin {}。允许接收用户 [{}]，允许发送用户 [{}]。配置不会启用联网或挂载 Plugin；凭据通过专用输入控件提供。",
                binding.graph,
                binding.reply_node,
                binding.plugin,
                binding.users.join(", "),
                binding.send_users.join(", ")
            );
            (message, Some((platform, binding)), None)
        }
        "channel_enable" => {
            let (platform, enabled) =
                crate::pilot_tools::channel_management::enable_arguments(&arguments)?;
            let state = tools
                .application
                .channel_snapshot(&platform)
                .map_err(|error| failure(application_reason(error)))?
                .ok_or_else(|| failure("channel is not configured"))?;
            if state.owner != tools.owner {
                return Err(failure("the channel binding belongs to another owner"));
            }
            let message = format!(
                "{} {platform} 渠道接入，绑定 Graph {}、回复节点 {}。允许接收用户 [{}]，允许发送用户 [{}]。这只控制 Gateway 接入，现有 Run 的权限保持其准入授权。",
                if enabled { "启动" } else { "停止" },
                state.binding.graph,
                state.binding.reply_node,
                state.binding.users.join(", "),
                state.binding.send_users.join(", ")
            );
            (message, None, Some((platform, enabled, state.binding)))
        }
        _ => return Err(failure("unknown channel confirmation tool")),
    };
    let schema = native_schema(
        json!({"type":"object","properties":{"confirm":{"type":"boolean","title":"确认此渠道配置或接入操作","default":false}},"required":["confirm"],"additionalProperties":false}),
    )?;
    let request = peer.create_elicitation(ElicitRequestParams::FormElicitationParams {
        meta: None,
        message,
        requested_schema: schema,
    });
    tokio::pin!(request);
    let result = loop {
        tokio::select! {
            result = &mut request => break result.map_err(|_| failure("native channel confirmation unavailable"))?,
            _ = tokio::time::sleep(Duration::from_millis(50)) => active(tools, cancellation)?,
        }
    };
    active(tools, cancellation)?;
    if result.action != ElicitationAction::Accept
        || result
            .content
            .as_ref()
            .and_then(|content| content.get("confirm"))
            .and_then(Value::as_bool)
            != Some(true)
    {
        return Ok(vec![ToolResultContent::text(
            json!({"changed":false,"approved":false}).to_string(),
        )]);
    }
    let result = if let Some((platform, binding)) = binding {
        tools.configure_channel_approved(&platform, binding).await?
    } else if let Some((platform, enabled, expected)) = enable {
        if tools
            .application
            .channel_snapshot(&platform)
            .map_err(|error| failure(application_reason(error)))?
            .is_none_or(|state| state.binding != expected)
        {
            return Err(failure(
                "channel binding changed while confirmation was pending; request a fresh form",
            ));
        }
        tools
            .enable_channel_approved(&platform, enabled, &expected)
            .await?
    } else {
        return Err(failure("channel confirmation has no operation"));
    };
    Ok(vec![ToolResultContent::text(result.to_string())])
}
