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
            name: "environment_provision".into(),
            description: "Build the dependency environment the recorded requirement asks for from this deployment's authorized sources, then point its name at the result. The user approves the exact resolved package set in a native form; approval is never a tool argument.".into(),
            parameters: json!({"type":"object","properties":{"name":{"type":"string"}},"required":["name"],"additionalProperties":false}),
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
    if name == "environment_provision" {
        return provision_environment(tools, arguments, peer, cancellation).await;
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
    render_impact(&mut message, plan.references.as_ref());
    message.push_str("发布后内容不可修改；引用它的节点/插件会在下一次接纳时使用这个编号。\n");
    message
}

/// Print the impact range exactly as completely as it was measured.
///
/// The wording matters: "nobody uses this" may only be said when coverage is
/// `Complete`, so a partial or unknown report says so instead of showing a
/// smaller number.
fn render_impact(
    message: &mut String,
    references: Option<&crate::application::graphs::EnvironmentReferences>,
) {
    match references.map(|references| &references.coverage) {
        Some(crate::application::graphs::ImpactCoverage::Complete) => {
            let references = references.expect("coverage implies references");
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
        Some(crate::application::graphs::ImpactCoverage::Partial { .. })
        | Some(crate::application::graphs::ImpactCoverage::Unknown { .. }) => {}
        None => message.push_str("影响范围未记录。\n"),
    }
    if let Some(references) = references {
        render_coverage(message, &references.coverage);
    }
}

/// Say how complete an impact report was, whatever it was about.
///
/// A count may only be read as "this is everything" when the scan really saw
/// everything, so a partial or unknown scan says so instead of showing a
/// smaller number.
fn render_coverage(message: &mut String, coverage: &crate::application::graphs::ImpactCoverage) {
    use crate::application::graphs::ImpactCoverage;
    match coverage {
        ImpactCoverage::Complete => {}
        ImpactCoverage::Partial {
            considered,
            total,
            missing,
        } => {
            message.push_str(&format!(
                "影响范围只能部分确认（已考察 {considered}/{total} 个图）：{}。下面这个数字不是全部。\n",
                missing
                    .iter()
                    .map(|missing| format!("{}（{}）", missing.graph, missing.reason))
                    .collect::<Vec<_>>()
                    .join("、")
            ));
        }
        ImpactCoverage::Unknown { reason } => {
            message.push_str(&format!("影响范围未知：{reason}（不能读成 0 个引用）。\n"))
        }
    }
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
    let precondition = tools
        .application
        .environment_publish_precondition(tools.library_root.clone(), candidate, name.clone())
        .await
        .map_err(|error| {
            failure(format!(
                "environment candidate was rejected: {}",
                application_reason(error)
            ))
        })?;
    if precondition.already_published() {
        // The name already points at exactly this content, so the publication is
        // a fact the Library already holds; nothing is changed and nothing is
        // recorded (§D3A).
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
        result = &mut elicitation => match result {
            Ok(result) => result,
            // The form never got through: nobody was asked, so nothing runs.
            Err(_) => return Err(failure("native user confirmation could not complete")),
        },
        _ = async {
            while !cancellation.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        } => return Err(failure("Pilot was cancelled while waiting for the user")),
    };
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
    // The Turn can end between the human's answer and this act; the act still
    // does not run, and nothing is left behind to replay.
    active(tools, cancellation)?;
    if !accepted {
        return Ok(vec![ToolResultContent::text(
            json!({
                "published":false,
                "name":precondition.plan.name,
                "action":result.action
            })
            .to_string(),
        )]);
    }
    let (id, version) = tools
        .application
        .publish_environment_if_unchanged(tools.library_root.clone(), precondition.clone())
        .await
        .map_err(|error| {
            failure(format!(
                "environment publication rejected: {}",
                application_reason(error)
            ))
        })?;
    Ok(vec![ToolResultContent::text(
        json!({
            "published":true,
            "name":precondition.plan.name,
            "id":id,
            "name_version":version,
            "replaced":precondition.replacing()
        })
        .to_string(),
    )])
}

/// The one argument the model may supply when asking to prepare an environment.
fn provision_arguments(arguments: &serde_json::Map<String, Value>) -> Result<String, ToolError> {
    if let Some(key) = arguments.keys().find(|key| key.as_str() != "name") {
        return Err(failure(format!(
            "environment_provision does not accept `{key}`; the user approves the resolved plan, not the model"
        )));
    }
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
    Ok(name)
}

/// What the human is shown before approving a resolved dependency plan.
///
/// The point is that they approve the *content*: the interpreter, the sources,
/// and every pinned package with every digest the lock accepts — not a request
/// like "install what the assistant asked for".
///
/// Every candidate digest is listed, because the lock records hashes but not file
/// names: it does not say which of them is the wheel, and this form refuses to
/// guess. What gets installed is whichever candidate arrives as a wheel with a
/// matching digest from an authorized source, which is what the closing line
/// says.
fn provision_message(
    plan: &crate::application::environments::EnvironmentProvisionPlan,
    replacing: bool,
) -> String {
    let mut message = format!(
        "确认按下面这个已解析的锁定集合准备并发布依赖环境「{}」？\n解释器：Python {}（{}，sha256 {}…）\n",
        plan.name,
        plan.interpreter,
        plan.interpreter_path.display(),
        &plan.interpreter_digest[..plan.interpreter_digest.len().min(16)]
    );
    message.push_str(&format!("来源：{}\n", plan.sources.join(", ")));
    message.push_str(&format!(
        "锁定集合（{} 个包，共 {} 个候选产物）：\n",
        plan.packages.len(),
        plan.packages
            .iter()
            .map(|package| package.hashes.len())
            .sum::<usize>()
    ));
    for package in plan.packages.iter().take(12) {
        message.push_str(&format!(
            "  {}=={}（{} 个候选）\n",
            package.name,
            package.version,
            package.hashes.len()
        ));
        for hash in &package.hashes {
            message.push_str(&format!("    sha256:{}…\n", &hash[..hash.len().min(16)]));
        }
    }
    if plan.packages.len() > 12 {
        message.push_str(&format!("  …其余 {} 个\n", plan.packages.len() - 12));
    }
    message.push_str(
        "安装：宿主只安装候选里那个来自上述授权来源、且字节与候选摘要相符的 wheel；本表单不预判哪个候选是 wheel，锁里没有文件名。安装发生在无网络的沙箱内，不编译源码包。\n",
    );
    render_impact(&mut message, plan.references.as_ref());
    message.push_str(if replacing {
        "该名字当前指向别的编号，本次会切换它（旧内容保留）。\n"
    } else {
        "这是新名字，不会替换任何现有引用。\n"
    });
    message.push_str("发布后内容不可修改；引用它的节点/插件会在下一次接纳时使用这个编号。\n");
    message
}

/// Resolve, build and publish the environment one recorded requirement asks for.
async fn provision_environment(
    tools: &PilotTools,
    arguments: &serde_json::Map<String, Value>,
    peer: Peer<RoleServer>,
    cancellation: &Cancellation,
) -> Result<Vec<ToolResultContent>, ToolError> {
    let name = provision_arguments(arguments)?;
    // An environment that provides this assistant's own dependencies would let it
    // change itself; the operator interface owns that.
    let own = tools
        .application
        .assistant_uses_environment_name(&tools.graph_name, &name)
        .map_err(|error| match error {
            crate::application::ApplicationError::Invalid(reason) => failure(reason),
            _ => failure("the assistant Graph is unavailable"),
        })?;
    if own {
        return Err(failure(format!(
            "`{name}` provides this assistant's own dependencies; ask the operator to prepare it through the operator interface"
        )));
    }
    let precondition = tools
        .application
        .environment_provision_precondition(tools.library_root.clone(), name.clone())
        .await
        .map_err(|error| {
            failure(format!(
                "environment preparation was rejected: {}",
                application_reason(error)
            ))
        })?;

    let mut properties = serde_json::Map::new();
    properties.insert(
        "confirm".into(),
        json!({"type":"boolean","title":format!("确认按这个锁定集合准备并发布「{}」", precondition.plan.name),"default":false}),
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
        message: provision_message(&precondition.plan, precondition.replacing()),
        requested_schema,
    });
    tokio::pin!(elicitation);
    let result = tokio::select! {
        result = &mut elicitation => match result {
            Ok(result) => result,
            Err(_) => return Err(failure("native user confirmation could not complete")),
        },
        _ = async {
            while !cancellation.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        } => return Err(failure("Pilot was cancelled while waiting for the user")),
    };
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
    // The Turn can end between the human's answer and this act; see the publish
    // path above. Nothing is written down, so nothing can be replayed.
    active(tools, cancellation)?;
    if !accepted {
        return Ok(vec![ToolResultContent::text(
            json!({
                "provisioned":false,
                "name":precondition.plan.name,
                "action":result.action
            })
            .to_string(),
        )]);
    }
    let outcome = tools
        .application
        .provision_environment_if_unchanged(tools.library_root.clone(), precondition.clone())
        .await
        .map_err(|error| {
            failure(format!(
                "environment preparation failed: {}",
                application_reason(error)
            ))
        })?;
    Ok(vec![ToolResultContent::text(
        json!({
            "provisioned":true,
            "name":precondition.plan.name,
            "id":outcome.id,
            "name_version":outcome.name_version,
            "files":outcome.files,
            "bytes":outcome.bytes,
            "packages":precondition.plan.packages.len(),
            "replaced":precondition.replacing()
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
    // Changing a Plugin's manifest changes a binding its Runs already froze, so
    // the human is told exactly what the replacement reaches.
    let impact = &precondition.impact;
    if impact.graphs.is_empty() && impact.unfinished_runs.is_empty() {
        message.push_str("当前没有 Graph 或未结束的 Run 绑定这个插件。\n");
    } else {
        message.push_str(&format!(
            "当前依赖这个插件：{} 张图{}，{} 个未结束的 Run{}\n",
            impact.graphs.len(),
            if impact.graphs.is_empty() {
                String::new()
            } else {
                format!("（{}）", impact.graphs.join(", "))
            },
            impact.unfinished_runs.len(),
            if impact.unfinished_runs.is_empty() {
                String::new()
            } else {
                format!("（{}）", impact.unfinished_runs.join(", "))
            }
        ));
        if !impact.unfinished_runs.is_empty() {
            message.push_str("未结束的 Run 记录了它被接纳时的插件指纹；替换后它们会拒绝继续，需要先结束或放弃它们。\n");
        }
    }
    // Outside the branch on purpose: an empty count beside a partial scan would
    // otherwise read as "nothing depends on it" when a Graph could not be read.
    render_coverage(&mut message, &impact.coverage);
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
    if !precondition.impact.unfinished_runs.is_empty() {
        properties.insert(
            "replace_bound_runs".into(),
            json!({"type":"boolean","title":format!("我确认：{} 个未结束的 Run 仍绑定这个插件，替换后它们会拒绝继续", precondition.impact.unfinished_runs.len()),"default":false}),
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
        && (!precondition.review.binary_without_source || approved("accept_unreadable"))
        && (precondition.impact.unfinished_runs.is_empty() || approved("replace_bound_runs"));
    if !accepted {
        return Ok(vec![ToolResultContent::text(
            json!({"installed":false,"id":precondition.review.id,"action":result.action})
                .to_string(),
        )]);
    }
    let outcome = tools
        .application
        .install_plugin_if_unchanged(
            tools.library_root.clone(),
            precondition.clone(),
            approved("replace_bound_runs"),
        )
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

    /// The plan the form renders: two candidate digests for one package, the way
    /// a real lock records a wheel and an sdist.
    fn provision_plan() -> crate::application::environments::EnvironmentProvisionPlan {
        use crate::environment_provision::LockedPackage;
        crate::application::environments::EnvironmentProvisionPlan {
            name: "research".into(),
            sources: vec!["index:https://pypi.tuna.tsinghua.edu.cn/simple".into()],
            interpreter: "3.12.13".into(),
            interpreter_path: std::path::PathBuf::from("/usr/bin/python3.12"),
            interpreter_digest: "f".repeat(64),
            lock_digest: "a".repeat(64),
            packages: vec![LockedPackage {
                name: "pygments".into(),
                version: "2.20.0".into(),
                // What a real lock records: the sdist digest hashes smaller.
                hashes: vec![
                    "6757cd03768053ff99f3039c1a36d6c0aa0b263438fcab17520b30a303a82b5f".into(),
                    "81a9e26dd42fd28a23a2d169d86d7ac03b46e2f8b59ed4698fb4785f946d0176".into(),
                ],
            }],
            references: None,
            observed_id: None,
            observed_name_version: None,
        }
    }

    #[test]
    fn the_provision_form_lists_every_candidate_and_claims_no_wheel() {
        let plan = provision_plan();
        let message = provision_message(&plan, false);

        // Both candidate digests are shown, not just the first one.
        for hash in &plan.packages[0].hashes {
            assert!(
                message.contains(&format!("sha256:{}…", &hash[..16])),
                "{message}"
            );
        }
        assert!(
            message.contains("pygments==2.20.0（2 个候选）"),
            "{message}"
        );
        assert!(
            message.contains("锁定集合（1 个包，共 2 个候选产物）"),
            "{message}"
        );
        // And the form does not pretend the first digest is the installed one.
        assert!(
            message.contains("本表单不预判哪个候选是 wheel，锁里没有文件名"),
            "{message}"
        );
        assert!(
            !message.contains("pygments==2.20.0 sha256:"),
            "the old single-digest line implied the first candidate is the artifact: {message}"
        );
    }

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
    fn provision_arguments_are_only_a_name() {
        let arguments = |value: Value| {
            value
                .as_object()
                .cloned()
                .expect("test arguments are objects")
        };
        assert_eq!(
            provision_arguments(&arguments(json!({"name":"research"}))).unwrap(),
            "research".to_owned()
        );
        for refused in [
            // The locked set comes from the recorded requirement, and the
            // approval comes from the native form: neither is a model argument.
            json!({"name":"research","packages":["requests"]}),
            json!({"name":"research","interpreter":"3.12"}),
            json!({"name":"research","confirm":true}),
            json!({"name":"research","replace_name":true}),
            json!({"name":"research","directory":"env"}),
            json!({}),
            json!({"name":""}),
            json!({"name":"../escape"}),
            json!({"name":"re search"}),
        ] {
            assert!(
                provision_arguments(&arguments(refused.clone())).is_err(),
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

/// The confirmed-tool lifecycle, driven through the real tool call over an
/// in-process MCP pairing: the assistant's confirmed tool asks a native form,
/// publishes only on a real answer, and leaves no second ledger behind.
#[cfg(test)]
mod plan_lifecycle_tests {
    use super::*;
    use crate::application::RunApplication;
    use crate::pilot_tools::PilotTools;
    use anchor_library::EnvironmentStore;
    use anchor_platform_session::{CreateSession, SessionStore};
    use rmcp::{
        ClientHandler, RoleClient, ServerHandler, ServiceExt,
        model::{
            CallToolRequestParams, CallToolResult, ContentBlock, ElicitResult, ErrorData,
            ServerCapabilities, ServerInfo,
        },
        service::{RequestContext, RunningService},
    };
    use std::sync::Arc;
    use tokio::sync::mpsc;

    const CANDIDATE: &str = "candidate";
    const NAME: &str = "research";

    /// The MCP client of the pairing: it answers every native form with one fixed
    /// payload and hands each ask to the test.
    struct FormClient {
        answer: Value,
        asks: mpsc::UnboundedSender<ElicitRequestParams>,
    }

    impl ClientHandler for FormClient {
        async fn create_elicitation(
            &self,
            request: ElicitRequestParams,
            _context: RequestContext<RoleClient>,
        ) -> Result<ElicitResult, ErrorData> {
            let _ = self.asks.send(request);
            Ok(ElicitResult::new(ElicitationAction::Accept).with_content(self.answer.clone()))
        }
    }

    /// The MCP server of the pairing: exactly the host's confirmed-tool entry
    /// point, holding the peer the tool asks the human through.
    #[derive(Clone)]
    struct ConfirmedTools {
        tools: Arc<PilotTools>,
        cancellation: Cancellation,
    }

    impl ServerHandler for ConfirmedTools {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
        }

        async fn call_tool(
            &self,
            request: CallToolRequestParams,
            context: RequestContext<RoleServer>,
        ) -> Result<CallToolResult, ErrorData> {
            let arguments = Value::Object(request.arguments.unwrap_or_default());
            match call(
                &self.tools,
                &request.name,
                arguments,
                context.peer.clone(),
                &self.cancellation,
            )
            .await
            {
                Ok(contents) => Ok(CallToolResult::success(
                    contents
                        .into_iter()
                        .filter_map(|content| {
                            content.as_text().map(ContentBlock::text).or_else(|| {
                                content
                                    .as_json()
                                    .map(|json| ContentBlock::text(json.to_string()))
                            })
                        })
                        .collect(),
                )),
                Err(error) => Ok(CallToolResult::error(vec![ContentBlock::text(
                    error.to_string(),
                )])),
            }
        }
    }

    struct Fixture {
        _root: tempfile::TempDir,
        tools: Arc<PilotTools>,
        client: RunningService<RoleClient, FormClient>,
        asks: mpsc::UnboundedReceiver<ElicitRequestParams>,
    }

    impl Fixture {
        async fn start(answer: Value) -> Self {
            let root = tempfile::tempdir().unwrap();
            // The assistant Graph the confirmed tools load before they ask.
            std::fs::create_dir_all(root.path().join("fixture")).unwrap();
            std::fs::write(
                root.path().join("fixture/graph.json"),
                r#"{"entry":"idle","ops":{"idle":{"run":"true"}},"nodes":[{"id":"idle","op":"idle"}],"edges":[]}"#,
            )
            .unwrap();
            std::fs::write(
                root.path().join("fixture/manifest.json"),
                r#"{"format":1,"graph":"graph.json","plugins":[]}"#,
            )
            .unwrap();

            // A Pilot Session with one running Turn: a confirmed tool refuses to
            // ask anything once its Turn is over.
            let sessions = SessionStore::open(root.path().join("sessions.sqlite")).unwrap();
            sessions
                .create(
                    "local",
                    CreateSession {
                        id: Some("pilot".into()),
                        ..Default::default()
                    },
                )
                .unwrap();
            let (turn, _) = sessions
                .create_turn("local", "pilot", "request-1", Some("publish it"))
                .unwrap();

            // The candidate is a prepared directory inside the workspace.
            let workspace_root = root.path().join("work");
            let candidate = workspace_root.join(CANDIDATE);
            std::fs::create_dir_all(candidate.join("bin")).unwrap();
            std::fs::write(candidate.join("bin/probe"), "#!/bin/sh\n").unwrap();

            let data_root = root.path().join("state");
            let tools = Arc::new(PilotTools {
                application: RunApplication::new(data_root.clone(), root.path().to_path_buf()),
                graph_name: "fixture".into(),
                catalog_root: root.path().to_path_buf(),
                library_root: root.path().join("library"),
                data_root,
                workspace_root,
                sessions,
                owner: "local".into(),
                session: "pilot".into(),
                turn: turn.id,
            });

            let cancellation: Cancellation = Cancellation::default();
            let (asks_tx, asks) = mpsc::unbounded_channel();
            let (server_io, client_io) = tokio::io::duplex(16_384);
            let handler = ConfirmedTools {
                tools: Arc::clone(&tools),
                cancellation,
            };
            tokio::spawn(async move {
                if let Ok(running) = handler.serve(server_io).await {
                    let _ = running.waiting().await;
                }
            });
            let client = FormClient {
                answer,
                asks: asks_tx,
            }
            .serve(client_io)
            .await
            .expect("the in-process MCP client handshakes");

            Self {
                _root: root,
                tools,
                client,
                asks,
            }
        }

        /// Call one confirmed tool; the answer is its text payload.
        async fn call(&self, name: &str, arguments: Value) -> (bool, Value) {
            call_tool(&self.client, name, arguments).await
        }
    }

    /// Call one confirmed tool; the answer is its text payload.
    async fn call_tool(
        client: &RunningService<RoleClient, FormClient>,
        name: &str,
        arguments: Value,
    ) -> (bool, Value) {
        let mut request = CallToolRequestParams::new(name.to_owned());
        request.arguments = arguments.as_object().cloned();
        let result = client
            .call_tool(request)
            .await
            .expect("the confirmed tool answers");
        let text = result
            .content
            .iter()
            .find_map(|block| match block {
                ContentBlock::Text(text) => Some(text.text.clone()),
                _ => None,
            })
            .unwrap_or_default();
        (
            result.is_error == Some(true),
            serde_json::from_str(&text).unwrap_or(Value::String(text)),
        )
    }

    /// §5.5: neither confirmed environment act writes a plan ledger.
    fn assert_no_plan_directories(data_root: &std::path::Path) {
        assert!(
            !data_root.join("environment-plans").exists(),
            "no publication plan directory is written"
        );
        assert!(
            !data_root.join("environment-provision-plans").exists(),
            "no provision plan directory is written"
        );
    }

    /// §5.1/§5.5: an approved publish puts the content in the Library, points the
    /// name at the new identity, and writes nothing into `state/`.
    #[tokio::test]
    async fn an_approved_publish_points_the_name_and_keeps_no_ledger() {
        let mut fixture = Fixture::start(json!({"confirm": true})).await;
        let (failed, result) = fixture
            .call(
                "environment_publish",
                json!({"directory":CANDIDATE,"name":NAME}),
            )
            .await;
        assert!(!failed, "{result}");
        assert_eq!(result["published"], json!(true), "{result}");
        assert!(
            fixture.asks.try_recv().is_ok(),
            "the native form was asked before anything published"
        );

        let store = EnvironmentStore::new(&fixture.tools.library_root);
        let names = store.names().unwrap();
        assert_eq!(names.len(), 1, "{names:?}");
        assert_eq!(names[0].name, NAME);
        assert_eq!(names[0].id, result["id"].as_str().unwrap());
        assert!(
            store.show(&names[0].id).is_ok(),
            "the name points at published content"
        );
        assert_no_plan_directories(&fixture.tools.data_root);
    }

    /// §5.2: a form answered with "no" publishes nothing and moves no name.
    #[tokio::test]
    async fn a_form_answered_with_no_publishes_nothing_and_moves_no_name() {
        let fixture = Fixture::start(json!({"confirm": false})).await;
        let (failed, result) = fixture
            .call(
                "environment_publish",
                json!({"directory":CANDIDATE,"name":NAME}),
            )
            .await;
        assert!(!failed, "{result}");
        assert_eq!(result["published"], json!(false), "{result}");
        let store = EnvironmentStore::new(&fixture.tools.library_root);
        assert!(store.names().unwrap().is_empty(), "a refusal moves no name");
        assert!(
            store.list().unwrap().is_empty(),
            "a refusal publishes nothing"
        );
        assert_no_plan_directories(&fixture.tools.data_root);
    }
}
