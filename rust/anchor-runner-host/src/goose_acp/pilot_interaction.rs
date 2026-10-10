use crate::application::environments::EnvironmentPublishPrecondition;
use crate::application::plugins::PluginInstallPrecondition;
use crate::pilot_tools::{PilotTools, identifier, workspace_directory};
use anchor_runtime::{Cancellation, ToolDefinition, ToolError, ToolResultContent};
use rmcp::{
    Peer, RoleServer,
    model::{ElicitRequestParams, ElicitationAction},
};
use serde_json::{Value, json};
use std::{sync::atomic::Ordering, time::Duration};

pub(crate) fn definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "ask_user".into(),
            description: "Ask the user for necessary information using a native form; the current Turn waits for their answer. Decline/cancel is not consent.".into(),
            parameters: json!({"type":"object","properties":{"message":{"type":"string"},"requested_schema":{"type":"object"}},"required":["message","requested_schema"],"additionalProperties":false}),
        },
        ToolDefinition {
            name: "graph_delete".into(),
            description: "Delete the saved Graph and its retained Runs/workspaces only after a fresh native user confirmation of its exact current contents. Never infer confirmation from a chat message.".into(),
            parameters: json!({"type":"object","properties":{"graph":{"type":"string"}},"required":["graph"],"additionalProperties":false}),
        },
        ToolDefinition {
            name: "environment_publish".into(),
            description: "Publish a dependency environment you prepared inside your own workspace and point a deployment name at it. The user approves the exact content in a native form; approval is never a tool argument. Prepare the directory first, then ask.".into(),
            parameters: json!({"type":"object","properties":{"directory":{"type":"string"},"name":{"type":"string"}},"required":["directory","name"],"additionalProperties":false}),
        },
        ToolDefinition {
            name: "plugin_install".into(),
            description: "Install a Plugin candidate you prepared inside your own workspace into the Anchor Library. The user approves the exact reviewed content in a native form; approval is never a tool argument. Fetch, adapt, build and self-test the candidate first, then review it.".into(),
            parameters: json!({"type":"object","properties":{"directory":{"type":"string"},"id":{"type":"string"}},"required":["directory"],"additionalProperties":false}),
        },
    ]
}

fn failure(message: impl Into<String>) -> ToolError {
    ToolError::Failed(message.into())
}

/// The assistant needs the concrete reason (for example a catalog field it has
/// to fix), so surface the application's own message rather than a Debug form.
fn application_reason(error: crate::application::ApplicationError) -> String {
    use crate::application::ApplicationError;
    match error {
        ApplicationError::Missing => "not found".into(),
        ApplicationError::Conflict(reason)
        | ApplicationError::Invalid(reason)
        | ApplicationError::Storage(reason) => reason,
    }
}

fn active(tools: &PilotTools, cancellation: &Cancellation) -> Result<(), ToolError> {
    if cancellation.load(Ordering::Acquire)
        || tools
            .sessions
            .get_turn(&tools.owner, &tools.session, &tools.turn)
            .map_err(|_| failure("Pilot Turn is unavailable"))?
            .status
            != anchor_platform_session::TurnStatus::Running
    {
        return Err(failure("Pilot Turn is not active"));
    }
    Ok(())
}

fn native_schema(schema: Value) -> Result<rmcp::model::ElicitationSchema, ToolError> {
    anchor_platform_session::validate_question_schema(&schema)
        .map_err(|error| failure(error.to_string()))?;
    let requested_schema: rmcp::model::ElicitationSchema =
        serde_json::from_value(schema.clone())
            .map_err(|_| failure("unsupported native form schema"))?;
    let serialized = serde_json::to_value(&requested_schema)
        .map_err(|_| failure("native form schema serialization failed"))?;
    let mut expected = schema;
    expected
        .as_object_mut()
        .unwrap()
        .remove("additionalProperties");
    if serialized != expected {
        return Err(failure(
            "native MCP form schema cannot preserve the requested constraints",
        ));
    }
    Ok(requested_schema)
}

pub(super) async fn call(
    tools: &PilotTools,
    name: &str,
    arguments: Value,
    peer: Peer<RoleServer>,
    cancellation: &Cancellation,
) -> Result<Vec<ToolResultContent>, ToolError> {
    active(tools, cancellation)?;
    let arguments = arguments
        .as_object()
        .ok_or_else(|| failure("tool arguments must be an object"))?;
    if name == "plugin_install" {
        return install(tools, arguments, peer, cancellation).await;
    }
    if name == "environment_publish" {
        return publish_environment(tools, arguments, peer, cancellation).await;
    }
    if !matches!(name, "ask_user" | "graph_delete") {
        // Falling through to ask_user would show a generic form the model fills
        // in, which is exactly what a confirmed tool must never do.
        return Err(failure(format!(
            "`{name}` is declared as a confirmed tool but has no native confirmation implementation"
        )));
    }
    let (message, schema, deletion) = if name == "graph_delete" {
        if arguments.len() != 1 {
            return Err(failure(
                "graph_delete accepts only graph; confirmation cannot be supplied by the model",
            ));
        }
        let graph = arguments
            .get("graph")
            .and_then(Value::as_str)
            .ok_or_else(|| failure("graph must be a string"))?;
        // The assistant may not delete the Graph that defines it.
        tools.refuse_self_graph(graph)?;
        let expected = tools
            .application
            .graph_delete_precondition(graph)
            .await
            .map_err(|error| failure(format!("Graph deletion rejected: {error}")))?;
        (
            format!(
                "确认删除 Graph「{graph}」及其保留的 Run、workspace 和产物？此操作不可撤销。确认只适用于当前内容：{expected}"
            ),
            json!({"type":"object","properties":{"confirm":{"type":"boolean","title":"确认删除此 Graph 及其运行数据","default":false}},"required":["confirm"],"additionalProperties":false}),
            Some((graph.to_owned(), expected)),
        )
    } else {
        if arguments.len() != 2 {
            return Err(failure(
                "ask_user accepts only message and requested_schema",
            ));
        }
        let message = arguments
            .get("message")
            .and_then(Value::as_str)
            .filter(|message| !message.trim().is_empty() && message.len() <= 65536)
            .ok_or_else(|| failure("message must be nonblank and bounded"))?;
        let schema = arguments
            .get("requested_schema")
            .ok_or_else(|| failure("requested_schema is required"))?;
        (message.to_owned(), schema.clone(), None)
    };
    let requested_schema = native_schema(schema)?;
    let elicitation = peer.create_elicitation(ElicitRequestParams::FormElicitationParams {
        meta: None,
        message,
        requested_schema,
    });
    tokio::pin!(elicitation);
    let result = tokio::select! {
        result = &mut elicitation => result.map_err(|_| failure("native user question could not complete"))?,
        _ = async {
            while !cancellation.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        } => return Err(failure("Pilot was cancelled while waiting for the user")),
    };
    active(tools, cancellation)?;
    let value = if let Some((graph, expected)) = deletion {
        if result.action != ElicitationAction::Accept
            || result
                .content
                .as_ref()
                .and_then(|content| content.get("confirm"))
                != Some(&Value::Bool(true))
        {
            json!({"deleted":false,"graph":graph,"action":result.action})
        } else {
            let removed = tools
                .application
                .remove_graph_if_unchanged_guarded(&graph, &tools.workspace_root, &expected, || {
                    active(tools, cancellation).map_err(|error| {
                        crate::application::graphs::GraphManagementError::Conflict(
                            error.to_string(),
                        )
                    })
                })
                .await
                .map_err(|error| {
                    failure(format!(
                        "Graph deletion rejected; request a fresh confirmation: {error}"
                    ))
                })?;
            json!({"deleted":true,"graph":graph,"removed_runs":removed,"precondition":expected})
        }
    } else {
        serde_json::to_value(result).map_err(|_| failure("native answer serialization failed"))?
    };
    Ok(vec![ToolResultContent::text(value.to_string())])
}

/// Only the candidate location is a model argument: approval is not.
fn install_arguments(
    arguments: &serde_json::Map<String, Value>,
) -> Result<(String, Option<String>), ToolError> {
    if let Some(key) = arguments
        .keys()
        .find(|key| !matches!(key.as_str(), "directory" | "id"))
    {
        return Err(failure(format!(
            "plugin_install does not accept `{key}`; the user approves the installation, not the model"
        )));
    }
    let directory = arguments
        .get("directory")
        .and_then(Value::as_str)
        .filter(|directory| !directory.trim().is_empty())
        .ok_or_else(|| failure("directory must be a nonempty workspace-relative path"))?
        .to_owned();
    let id = match arguments.get("id").and_then(Value::as_str) {
        Some(id) if identifier(id) => Some(id.to_owned()),
        Some(_) => return Err(failure("id must be a Plugin identifier")),
        None => None,
    };
    Ok((directory, id))
}

/// The form the operator reads. It names what will run and never carries a
/// credential value: only environment variable names are listed.
fn environment_arguments(
    arguments: &serde_json::Map<String, Value>,
) -> Result<(String, String), ToolError> {
    if let Some(key) = arguments
        .keys()
        .find(|key| !matches!(key.as_str(), "directory" | "name"))
    {
        return Err(failure(format!(
            "environment_publish does not accept `{key}`; the user approves the publication, not the model"
        )));
    }
    let directory = arguments
        .get("directory")
        .and_then(Value::as_str)
        .filter(|directory| !directory.trim().is_empty())
        .ok_or_else(|| failure("directory must be a nonempty workspace-relative path"))?
        .to_owned();
    let name = arguments
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .ok_or_else(|| failure("name must be a nonempty deployment name"))?
        .to_owned();
    if !identifier(&name) {
        return Err(failure(
            "name must use only letters, digits, dot, dash and underscore",
        ));
    }
    Ok((directory, name))
}

/// Record that an approved plan did not complete, so the operator read model
/// never shows an answered request as pending.
fn abort_environment_plan(
    tools: &PilotTools,
    plan: &crate::application::environments::EnvironmentPlan,
    reason: &str,
) {
    let mut aborted = plan.clone();
    aborted.status = crate::application::environments::EnvironmentPlanStatus::Aborted {
        reason: reason.to_owned(),
    };
    if tools.application.write_environment_plan(&aborted).is_err() {
        eprintln!("environment plan {} could not be marked aborted", plan.name);
    }
}

fn environment_message(precondition: &EnvironmentPublishPrecondition) -> String {
    let plan = &precondition.plan;
    let mut message = format!(
        "确认把这份内容发布为依赖环境「{}」？\n内容指纹：{}\n内容规模：{} 个文件 / {} 字节\n目录：{}\n",
        plan.name,
        plan.digest,
        plan.files,
        plan.bytes,
        plan.directory.display()
    );
    // Show the commands the environment would provide: the human is approving
    // content, not a hash.
    if let Ok(entries) = std::fs::read_dir(plan.directory.join("bin")) {
        let mut names = entries
            .flatten()
            .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
            .collect::<Vec<_>>();
        names.sort();
        let truncated = names.len() > 12;
        names.truncate(12);
        if !names.is_empty() {
            message.push_str(&format!(
                "可执行（前 12 个{}）：{}\n",
                if truncated { "，已截断" } else { "" },
                names.join(", ")
            ));
        }
    }
    message.push_str(if precondition.replacing() {
        "该名字当前指向别的编号，本次会切换它（旧内容保留）。\n"
    } else {
        "这是新名字，不会替换任何现有引用。\n"
    });
    // The impact range the human is being asked about, as it was recorded.
    match plan
        .references
        .as_ref()
        .map(|references| &references.coverage)
    {
        Some(crate::application::environments::EnvironmentCoverage::Complete) => {
            let references = plan
                .references
                .as_ref()
                .expect("coverage implies references");
            let mut users = references
                .nodes
                .iter()
                .map(|node| format!("{}::{}", node.graph, node.node))
                .chain(references.servers.iter().map(|server| {
                    format!(
                        "{}::{}/{}(server)",
                        server.graph, server.node, server.plugin
                    )
                }))
                .collect::<Vec<_>>();
            users.sort();
            users.truncate(8);
            message.push_str(&format!(
                "当前引用这个环境名字的位置：{} 个{}\n",
                references.nodes.len() + references.servers.len(),
                if users.is_empty() {
                    String::new()
                } else {
                    format!("（{}）", users.join(", "))
                }
            ));
        }
        Some(crate::application::environments::EnvironmentCoverage::Partial {
            scanned,
            total,
            ..
        }) => message.push_str(&format!(
            "影响范围只能部分确认（已扫描 {scanned}/{total} 个图），下面这个数字不是全部。\n"
        )),
        Some(crate::application::environments::EnvironmentCoverage::Unknown { reason }) => {
            message.push_str(&format!("影响范围未知：{reason}（不能读成 0 个引用）。\n"))
        }
        None => message.push_str("影响范围未记录。\n"),
    }
    message.push_str("发布后内容不可修改；引用它的节点/插件会在下一次接纳时使用这个编号。\n");
    message
}

async fn publish_environment(
    tools: &PilotTools,
    arguments: &serde_json::Map<String, Value>,
    peer: Peer<RoleServer>,
    cancellation: &Cancellation,
) -> Result<Vec<ToolResultContent>, ToolError> {
    let (directory, name) = environment_arguments(arguments)?;
    let candidate = workspace_directory(&tools.workspace_root, &directory)?;
    // An environment that provides this assistant's own dependencies would let it
    // change itself; the operator interface owns that. This follows the Graphs it
    // calls, because it can author those too.
    let own = tools
        .application
        .assistant_uses_environment_name(&tools.graph_name, &name)
        .map_err(|error| match error {
            crate::application::ApplicationError::Invalid(reason) => failure(reason),
            _ => failure("the assistant Graph is unavailable"),
        })?;
    if own {
        return Err(failure(format!(
            "`{name}` provides this assistant's own dependencies; ask the operator to publish it through the operator interface"
        )));
    }
    let requester = crate::application::environments::EnvironmentPlanRequester {
        owner: tools.owner.clone(),
        session: tools.session.clone(),
        turn: tools.turn.clone(),
    };
    let precondition = tools
        .application
        .environment_publish_precondition(
            tools.library_root.clone(),
            candidate,
            name.clone(),
            requester,
        )
        .await
        .map_err(|error| {
            failure(format!(
                "environment candidate was rejected: {}",
                application_reason(error)
            ))
        })?;
    if precondition.already_published() {
        return Ok(vec![ToolResultContent::text(
            json!({"published":false,"already_published":true,"name":precondition.plan.name,"id":precondition.plan.digest})
                .to_string(),
        )]);
    }
    let mut properties = serde_json::Map::new();
    properties.insert(
        "confirm".into(),
        json!({"type":"boolean","title":format!("确认发布为依赖环境「{}」", precondition.plan.name),"default":false}),
    );
    if precondition.replacing() {
        properties.insert(
            "replace_name".into(),
            json!({"type":"boolean","title":format!("确认切换名字「{}」到新编号", precondition.plan.name),"default":false}),
        );
    }
    let required = properties.keys().cloned().collect::<Vec<_>>();
    let schema = json!({
        "type":"object",
        "properties":Value::Object(properties),
        "required":required,
        "additionalProperties":false
    });
    let requested_schema = native_schema(schema)?;
    let elicitation = peer.create_elicitation(ElicitRequestParams::FormElicitationParams {
        meta: None,
        message: environment_message(&precondition),
        requested_schema,
    });
    tokio::pin!(elicitation);
    let result = tokio::select! {
        result = &mut elicitation => result.map_err(|_| failure("native user confirmation could not complete"))?,
        _ = async {
            while !cancellation.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        } => return Err(failure("Pilot was cancelled while waiting for the user")),
    };
    active(tools, cancellation)?;
    let approved = |field: &str| {
        result
            .content
            .as_ref()
            .and_then(|content| content.get(field))
            == Some(&Value::Bool(true))
    };
    let accepted = result.action == ElicitationAction::Accept
        && approved("confirm")
        && (!precondition.replacing() || approved("replace_name"));
    if !accepted {
        // The plan stays durable as a refusal; it is never replayed automatically.
        // A failure to record it is reported rather than swallowed, because a stale
        // `requested` record reads as an unanswered request.
        let mut refused = precondition.plan.clone();
        refused.status = crate::application::environments::EnvironmentPlanStatus::Refused;
        let recorded = tools.application.write_environment_plan(&refused).is_ok();
        return Ok(vec![ToolResultContent::text(
            json!({
                "published":false,
                "name":precondition.plan.name,
                "action":result.action,
                "plan_recorded":recorded
            })
            .to_string(),
        )]);
    }
    let (id, version) = match tools
        .application
        .publish_environment_if_unchanged(tools.library_root.clone(), precondition.clone())
        .await
    {
        Ok(outcome) => outcome,
        Err(error) => {
            let reason = application_reason(error);
            abort_environment_plan(tools, &precondition.plan, &reason);
            return Err(failure(format!(
                "environment publication rejected: {reason}"
            )));
        }
    };
    let mut published = precondition.plan.clone();
    published.status = crate::application::environments::EnvironmentPlanStatus::Published {
        id: id.clone(),
        name_version: version,
    };
    // The publication already happened; a failed record update must not be
    // reported as a failed publication, but it must be visible.
    let plan_recorded = match tools.application.write_environment_plan(&published) {
        Ok(()) => true,
        Err(error) => {
            eprintln!(
                "environment plan {} could not be finalized after publication: {error:?}",
                published.name
            );
            false
        }
    };
    Ok(vec![ToolResultContent::text(
        json!({
            "published":true,
            "name":precondition.plan.name,
            "id":id,
            "name_version":version,
            "replaced":precondition.replacing(),
            "plan_recorded":plan_recorded
        })
        .to_string(),
    )])
}

fn install_message(precondition: &PluginInstallPrecondition) -> String {
    let review = &precondition.review;
    let mut message = format!(
        "确认把插件「{}」安装进 Anchor 插件库？\n内容指纹：{}\n",
        review.id, review.digest
    );
    message.push_str(if precondition.replacing() {
        "该 id 已安装，本次会覆盖现有版本。\n"
    } else {
        "这是新建，不会覆盖任何现有插件。\n"
    });
    for server in &review.servers {
        match (&server.command, &server.url) {
            (Some(command), _) if server.args.is_empty() => {
                message.push_str(&format!("会运行：{command}（{}）\n", server.name));
            }
            (Some(command), _) => {
                message.push_str(&format!(
                    "会运行：{command} {}（{}）\n",
                    server.args.join(" "),
                    server.name
                ));
            }
            (_, Some(url)) => {
                message.push_str(&format!("会连接：{url}（{}）\n", server.name));
            }
            _ => message.push_str(&format!("声明了 server {}（无命令与地址）\n", server.name)),
        }
    }
    let mut names = review
        .servers
        .iter()
        .flat_map(|server| server.environment.iter().map(String::as_str))
        .collect::<Vec<_>>();
    names.sort_unstable();
    names.dedup();
    if !names.is_empty() {
        message.push_str(&format!(
            "需要的环境变量（只列名字，不含值）：{}\n",
            names.join(", ")
        ));
    }
    for warning in &review.warnings {
        message.push_str(&format!("注意：{warning}\n"));
    }
    if !review.unsupported.is_empty() {
        message.push_str(&format!(
            "清单还声明了不会被执行的：{}\n",
            review.unsupported.join(", ")
        ));
    }
    message.chars().take(16_000).collect()
}

async fn install(
    tools: &PilotTools,
    arguments: &serde_json::Map<String, Value>,
    peer: Peer<RoleServer>,
    cancellation: &Cancellation,
) -> Result<Vec<ToolResultContent>, ToolError> {
    let (directory, id) = install_arguments(arguments)?;
    let candidate = workspace_directory(&tools.workspace_root, &directory)?;
    let id = match id {
        Some(id) => id,
        None => candidate
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| identifier(name))
            .ok_or_else(|| failure("the directory name is not a usable Plugin id; pass `id`"))?
            .to_owned(),
    };
    // Installing a Plugin that provides this assistant's own instructions would
    // let the assistant change itself; the operator interface owns that.
    let (_path, bundle) = tools
        .application
        .load_graph(&tools.graph_name)
        .map_err(|error| failure(format!("the assistant Graph is unavailable: {error:?}")))?;
    if bundle.plugins.iter().any(|binding| binding.id == id) {
        return Err(failure(format!(
            "`{id}` provides this assistant's own instructions; ask the operator to install it through the operator interface"
        )));
    }
    let precondition = tools
        .application
        .plugin_install_precondition(tools.library_root.clone(), candidate, id)
        .await
        .map_err(|error| {
            failure(format!(
                "Plugin candidate was rejected: {}",
                application_reason(error)
            ))
        })?;
    if precondition.already_installed() {
        return Ok(vec![ToolResultContent::text(
            json!({"installed":false,"already_installed":true,"id":precondition.review.id,"digest":precondition.review.digest})
                .to_string(),
        )]);
    }
    let mut properties = serde_json::Map::new();
    properties.insert(
        "confirm".into(),
        json!({"type":"boolean","title":"确认安装这个插件","default":false}),
    );
    if precondition.replacing() {
        properties.insert(
            "replace_existing".into(),
            json!({"type":"boolean","title":format!("确认覆盖已安装的 {}", precondition.review.id),"default":false}),
        );
    }
    if precondition.review.binary_without_source {
        properties.insert(
            "accept_unreadable".into(),
            json!({"type":"boolean","title":"我接受这些无法阅读源码的编译产物","default":false}),
        );
    }
    let required = properties.keys().cloned().collect::<Vec<_>>();
    let schema = json!({
        "type":"object",
        "properties":Value::Object(properties),
        "required":required,
        "additionalProperties":false
    });
    let requested_schema = native_schema(schema)?;
    let elicitation = peer.create_elicitation(ElicitRequestParams::FormElicitationParams {
        meta: None,
        message: install_message(&precondition),
        requested_schema,
    });
    tokio::pin!(elicitation);
    let result = tokio::select! {
        result = &mut elicitation => result.map_err(|_| failure("native user confirmation could not complete"))?,
        _ = async {
            while !cancellation.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        } => return Err(failure("Pilot was cancelled while waiting for the user")),
    };
    active(tools, cancellation)?;
    let approved = |field: &str| {
        result
            .content
            .as_ref()
            .and_then(|content| content.get(field))
            == Some(&Value::Bool(true))
    };
    let accepted = result.action == ElicitationAction::Accept
        && approved("confirm")
        && (!precondition.replacing() || approved("replace_existing"))
        && (!precondition.review.binary_without_source || approved("accept_unreadable"));
    if !accepted {
        return Ok(vec![ToolResultContent::text(
            json!({"installed":false,"id":precondition.review.id,"action":result.action})
                .to_string(),
        )]);
    }
    let outcome = tools
        .application
        .install_plugin_if_unchanged(tools.library_root.clone(), precondition.clone())
        .await
        .map_err(|error| {
            failure(format!(
                "Plugin installation rejected: {}",
                application_reason(error)
            ))
        })?;
    Ok(vec![ToolResultContent::text(
        json!({"installed":true,"id":outcome.id,"digest":outcome.digest,"replaced":precondition.replacing()})
            .to_string(),
    )])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn native_form_cannot_silently_drop_schema_constraints() {
        assert!(native_schema(json!({"type":"object","properties":{"confirm":{"type":"boolean","default":false}},"required":["confirm"],"additionalProperties":false})).is_ok());
        for schema in [
            json!({"type":"object","properties":{"value":{"type":"string","pattern":"^yes$"}}}),
            json!({"type":"object","properties":{"value":{"type":"boolean","enum":[true]}}}),
            json!({"type":"object","properties":{"value":{"type":"integer","multipleOf":2}}}),
            json!({"type":"object","properties":{"value":{"type":"string"}},"minProperties":1}),
        ] {
            assert!(native_schema(schema).is_err());
        }
    }

    #[test]
    fn install_arguments_refuse_any_model_supplied_approval() {
        let arguments = |value: Value| {
            value
                .as_object()
                .cloned()
                .expect("test arguments are objects")
        };
        assert_eq!(
            install_arguments(&arguments(json!({"directory":"candidate"}))).unwrap(),
            ("candidate".to_owned(), None)
        );
        assert_eq!(
            install_arguments(&arguments(json!({"directory":"candidate","id":"demo"}))).unwrap(),
            ("candidate".to_owned(), Some("demo".to_owned()))
        );
        for refused in [
            json!({"directory":"candidate","confirm":true}),
            json!({"directory":"candidate","replace_existing":true}),
            json!({"directory":"candidate","accept_unreadable":true}),
            json!({"id":"demo"}),
            json!({"directory":""}),
            json!({"directory":"candidate","id":"bad id"}),
        ] {
            assert!(
                install_arguments(&arguments(refused.clone())).is_err(),
                "{refused} must be refused"
            );
        }
    }

    #[test]
    fn environment_arguments_refuse_any_model_supplied_approval() {
        let arguments = |value: Value| {
            value
                .as_object()
                .cloned()
                .expect("test arguments are objects")
        };
        assert_eq!(
            environment_arguments(&arguments(json!({"directory":"env","name":"research"})))
                .unwrap(),
            ("env".to_owned(), "research".to_owned())
        );
        for refused in [
            json!({"directory":"env","name":"research","confirm":true}),
            json!({"directory":"env","name":"research","replace_name":true}),
            json!({"directory":"env","name":"research","id":"demo"}),
            json!({"directory":"env"}),
            json!({"name":"research"}),
            json!({"directory":"","name":"research"}),
            json!({"directory":"env","name":""}),
            json!({"directory":"env","name":"../escape"}),
            json!({"directory":"env","name":"re search"}),
        ] {
            assert!(
                environment_arguments(&arguments(refused.clone())).is_err(),
                "{refused} must be refused"
            );
        }
    }
}
