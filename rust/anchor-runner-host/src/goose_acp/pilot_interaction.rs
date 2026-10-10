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

/// Why a plan holds no answer at all: the form never reached anyone.
const FORM_UNREACHED: &str = "the native form could not be delivered, so nobody was asked";
/// Why a plan holds no answer at all: the Pilot stopped with the form open.
const FORM_ABANDONED: &str = "the Pilot was cancelled while the form was open";
/// Why an approved act never ran: the Turn ended after the human approved it.
const TURN_ENDS_BEFORE_APPROVED_ACT: &str =
    "the human approved, but the Turn ended before the act could run";

/// Record that nobody answered a plan's form, so the record never stays a
/// request a human can no longer fulfil (§D2B).
fn cancel_environment_plan(
    tools: &PilotTools,
    plan: &crate::application::environments::EnvironmentPlan,
    reason: &str,
) {
    let mut cancelled = plan.clone();
    cancelled.status = crate::application::environments::EnvironmentPlanStatus::Cancelled {
        reason: reason.to_owned(),
    };
    if tools
        .application
        .write_environment_plan(&cancelled)
        .is_err()
    {
        eprintln!(
            "environment plan {} could not be marked cancelled",
            plan.name
        );
    }
}

/// Finish a plan whose Turn ended between the human's answer and the act.
///
/// The answer is already known here, so the record keeps it: an approval that
/// never ran is `Aborted`, anything else is `Refused` — the same terminal states
/// the ordinary paths write. It is never `Cancelled`, which means *nobody
/// answered* (I2), and it never stays `Requested`, which would report a live
/// question nobody can answer any more (I1). The act itself is not carried out.
fn finish_environment_plan_when_turn_is_gone(
    tools: &PilotTools,
    plan: &crate::application::environments::EnvironmentPlan,
    accepted: bool,
) {
    let mut finished = plan.clone();
    finished.status = if accepted {
        crate::application::environments::EnvironmentPlanStatus::Aborted {
            reason: TURN_ENDS_BEFORE_APPROVED_ACT.to_owned(),
        }
    } else {
        crate::application::environments::EnvironmentPlanStatus::Refused
    };
    if tools.application.write_environment_plan(&finished).is_err() {
        eprintln!(
            "environment plan {} could not be finished after its Turn ended",
            plan.name
        );
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
                "影响范围只能部分确认（已确认 {considered}/{total} 个图）：{}。下面这个数字不是全部。\n",
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
        // The name already points at exactly this content, so the publication is
        // a fact: the record ends as one instead of staying a request nobody
        // answered (§D3A). Nothing was changed, so a failed record update is
        // reported rather than raised.
        let plan_recorded = tools
            .application
            .record_observed_publication(&precondition)
            .is_ok();
        return Ok(vec![ToolResultContent::text(
            json!({"published":false,"already_published":true,"name":precondition.plan.name,"id":precondition.plan.digest,"plan_recorded":plan_recorded})
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
            // The form never got through: nobody was asked, so the record must
            // not read as a request still waiting for an answer.
            Err(_) => {
                cancel_environment_plan(tools, &precondition.plan, FORM_UNREACHED);
                return Err(failure("native user confirmation could not complete"));
            }
        },
        _ = async {
            while !cancellation.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        } => {
            cancel_environment_plan(tools, &precondition.plan, FORM_ABANDONED);
            return Err(failure("Pilot was cancelled while waiting for the user"));
        }
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
    // The Turn can end between the human's answer and this act. The answer is
    // known by now, so the record keeps it instead of staying a live request
    // nobody can answer (I1/I2); the act itself still does not run.
    if let Err(error) = active(tools, cancellation) {
        finish_environment_plan_when_turn_is_gone(tools, &precondition.plan, accepted);
        return Err(error);
    }
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
/// and every pinned package with its digest — not a request like "install what
/// the assistant asked for".
fn provision_message(
    precondition: &crate::application::environments::EnvironmentProvisionPrecondition,
) -> String {
    let plan = &precondition.plan;
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
            "  {}=={} sha256:{}…\n",
            package.name,
            package.version,
            &package.hashes[0][..package.hashes[0].len().min(16)]
        ));
    }
    if plan.packages.len() > 12 {
        message.push_str(&format!("  …其余 {} 个\n", plan.packages.len() - 12));
    }
    message
        .push_str("安装：只安装上述 digest 的 wheel，在无网络的沙箱内离线安装，不编译源码包。\n");
    render_impact(&mut message, plan.references.as_ref());
    message.push_str(if precondition.replacing() {
        "该名字当前指向别的编号，本次会切换它（旧内容保留）。\n"
    } else {
        "这是新名字，不会替换任何现有引用。\n"
    });
    message.push_str("发布后内容不可修改；引用它的节点/插件会在下一次接纳时使用这个编号。\n");
    message
}

fn abort_environment_provision_plan(
    tools: &PilotTools,
    plan: &crate::application::environments::EnvironmentProvisionPlan,
    reason: &str,
) {
    let mut aborted = plan.clone();
    aborted.status = crate::application::environments::EnvironmentProvisionStatus::Aborted {
        reason: reason.to_owned(),
    };
    if tools
        .application
        .write_environment_provision_plan(&aborted)
        .is_err()
    {
        eprintln!(
            "environment provision plan {} could not be marked aborted",
            plan.name
        );
    }
}

/// Record that nobody answered a build's form, so the record never stays a
/// request a human can no longer fulfil (§D2B).
fn cancel_environment_provision_plan(
    tools: &PilotTools,
    plan: &crate::application::environments::EnvironmentProvisionPlan,
    reason: &str,
) {
    let mut cancelled = plan.clone();
    cancelled.status = crate::application::environments::EnvironmentProvisionStatus::Cancelled {
        reason: reason.to_owned(),
    };
    if tools
        .application
        .write_environment_provision_plan(&cancelled)
        .is_err()
    {
        eprintln!(
            "environment provision plan {} could not be marked cancelled",
            plan.name
        );
    }
}

/// Finish a build whose Turn ended between the human's answer and the act; see
/// [`finish_environment_plan_when_turn_is_gone`].
fn finish_environment_provision_plan_when_turn_is_gone(
    tools: &PilotTools,
    plan: &crate::application::environments::EnvironmentProvisionPlan,
    accepted: bool,
) {
    let mut finished = plan.clone();
    finished.status = if accepted {
        crate::application::environments::EnvironmentProvisionStatus::Aborted {
            reason: TURN_ENDS_BEFORE_APPROVED_ACT.to_owned(),
        }
    } else {
        crate::application::environments::EnvironmentProvisionStatus::Refused
    };
    if tools
        .application
        .write_environment_provision_plan(&finished)
        .is_err()
    {
        eprintln!(
            "environment provision plan {} could not be finished after its Turn ended",
            plan.name
        );
    }
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
    let requester = crate::application::environments::EnvironmentPlanRequester {
        owner: tools.owner.clone(),
        session: tools.session.clone(),
        turn: tools.turn.clone(),
    };
    let precondition = tools
        .application
        .environment_provision_precondition(tools.library_root.clone(), name.clone(), requester)
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
        message: provision_message(&precondition),
        requested_schema,
    });
    tokio::pin!(elicitation);
    let result = tokio::select! {
        result = &mut elicitation => match result {
            Ok(result) => result,
            Err(_) => {
                cancel_environment_provision_plan(tools, &precondition.plan, FORM_UNREACHED);
                return Err(failure("native user confirmation could not complete"));
            }
        },
        _ = async {
            while !cancellation.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        } => {
            cancel_environment_provision_plan(tools, &precondition.plan, FORM_ABANDONED);
            return Err(failure("Pilot was cancelled while waiting for the user"));
        }
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
    // path above for why the record keeps the answer it already has.
    if let Err(error) = active(tools, cancellation) {
        finish_environment_provision_plan_when_turn_is_gone(tools, &precondition.plan, accepted);
        return Err(error);
    }
    if !accepted {
        // The resolved plan stays durable as a refusal; it is never replayed.
        let mut refused = precondition.plan.clone();
        refused.status = crate::application::environments::EnvironmentProvisionStatus::Refused;
        let recorded = tools
            .application
            .write_environment_provision_plan(&refused)
            .is_ok();
        return Ok(vec![ToolResultContent::text(
            json!({
                "provisioned":false,
                "name":precondition.plan.name,
                "action":result.action,
                "plan_recorded":recorded
            })
            .to_string(),
        )]);
    }
    let outcome = match tools
        .application
        .provision_environment_if_unchanged(tools.library_root.clone(), precondition.clone())
        .await
    {
        Ok(outcome) => outcome,
        Err(error) => {
            let reason = application_reason(error);
            abort_environment_provision_plan(tools, &precondition.plan, &reason);
            return Err(failure(format!("environment preparation failed: {reason}")));
        }
    };
    let mut provisioned = precondition.plan.clone();
    provisioned.status =
        crate::application::environments::EnvironmentProvisionStatus::Provisioned {
            id: outcome.id.clone(),
            name_version: outcome.name_version,
            files: outcome.files,
            bytes: outcome.bytes,
        };
    let plan_recorded = match tools
        .application
        .write_environment_provision_plan(&provisioned)
    {
        Ok(()) => true,
        Err(error) => {
            eprintln!(
                "environment provision plan {} could not be finalized: {error:?}",
                provisioned.name
            );
            false
        }
    };
    Ok(vec![ToolResultContent::text(
        json!({
            "provisioned":true,
            "name":precondition.plan.name,
            "id":outcome.id,
            "name_version":outcome.name_version,
            "files":outcome.files,
            "bytes":outcome.bytes,
            "packages":precondition.plan.packages.len(),
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

/// The approval lifecycle, driven through the real tool call over an in-process
/// MCP pairing: the assistant's confirmed tool asks a native form and the record
/// it leaves behind is read the way the operator read model reads it.
#[cfg(test)]
mod plan_lifecycle_tests {
    use super::*;
    use crate::application::RunApplication;
    use crate::application::environments::{
        EnvironmentPlan, EnvironmentPlanStatus, list_environment_plans,
    };
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

    /// How the native form behaves in one test.
    enum Form {
        /// The human never answers it.
        Silent,
        /// It answers with this content.
        Accept(Value),
        /// It answers, but the Turn is already over by the time the answer
        /// arrives: the client ends the Turn before handing the answer back,
        /// which is the race the answer-arrives-first branch handles.
        AcceptAfterTurnEnded(Value),
        /// It cannot be delivered at all.
        Unreachable,
    }

    /// The MCP client of the pairing: it answers (or never answers) the native
    /// forms the tool asks, and hands each ask to the test.
    struct FormClient {
        form: Form,
        asks: mpsc::UnboundedSender<ElicitRequestParams>,
        /// The Session the form belongs to, so a test can end its Turn while the
        /// answer is in flight.
        sessions: SessionStore,
        owner: String,
        session: String,
        turn: String,
    }

    impl ClientHandler for FormClient {
        async fn create_elicitation(
            &self,
            request: ElicitRequestParams,
            _context: RequestContext<RoleClient>,
        ) -> Result<ElicitResult, ErrorData> {
            let _ = self.asks.send(request);
            let accept = |content: &Value| {
                ElicitResult::new(ElicitationAction::Accept).with_content(content.clone())
            };
            match &self.form {
                Form::Silent => std::future::pending::<Result<ElicitResult, ErrorData>>().await,
                Form::Accept(content) => Ok(accept(content)),
                Form::AcceptAfterTurnEnded(content) => {
                    self.sessions
                        .finish_turn(
                            &self.owner,
                            &self.session,
                            &self.turn,
                            anchor_platform_session::TurnStatus::Stopped,
                            None,
                        )
                        .expect("the fixture Turn ends");
                    Ok(accept(content))
                }
                Form::Unreachable => Err(ErrorData::internal_error(
                    "the native form was lost in transit",
                    None,
                )),
            }
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
        cancellation: Cancellation,
        client: RunningService<RoleClient, FormClient>,
        asks: mpsc::UnboundedReceiver<ElicitRequestParams>,
    }

    impl Fixture {
        async fn start(form: Form) -> Self {
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
                cancellation: Arc::clone(&cancellation),
            };
            tokio::spawn(async move {
                if let Ok(running) = handler.serve(server_io).await {
                    let _ = running.waiting().await;
                }
            });
            let client = FormClient {
                form,
                asks: asks_tx,
                sessions: tools.sessions.clone(),
                owner: tools.owner.clone(),
                session: tools.session.clone(),
                turn: tools.turn.clone(),
            }
            .serve(client_io)
            .await
            .expect("the in-process MCP client handshakes");

            Self {
                _root: root,
                tools,
                cancellation,
                client,
                asks,
            }
        }

        /// Call one confirmed tool; the answer is its text payload.
        async fn call(&self, name: &str, arguments: Value) -> (bool, Value) {
            call_tool(&self.client, name, arguments).await
        }

        /// The one durable plan this deployment recorded.
        fn only_plan(&self) -> EnvironmentPlan {
            let listed = list_environment_plans(&self.tools.data_root).unwrap();
            assert_eq!(listed.records.len(), 1, "{:?}", listed.records);
            listed.records.into_iter().next().unwrap()
        }
    }

    /// Call one confirmed tool on a client whose owner may still be borrowed
    /// elsewhere (the T1 case waits for the form while the call is in flight).
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

    /// T2 (§D3A): a second call that finds the name already pointing at exactly
    /// this content records that publication instead of leaving the plan as a
    /// request nobody answered.
    #[tokio::test]
    async fn a_repeat_publish_records_the_publication_it_found() {
        let mut fixture = Fixture::start(Form::Silent).await;
        let directory = fixture.tools.workspace_root.join(CANDIDATE);
        let store = EnvironmentStore::new(&fixture.tools.library_root);
        let published = store.publish(&directory).unwrap();
        store.set_name(NAME, &published.id, None).unwrap();

        let (failed, result) = fixture
            .call(
                "environment_publish",
                json!({"directory":CANDIDATE,"name":NAME}),
            )
            .await;
        assert!(!failed, "{result}");
        assert_eq!(result["already_published"], json!(true), "{result}");
        assert_eq!(
            result["plan_recorded"],
            json!(true),
            "the observed publication is recorded: {result}"
        );
        assert!(
            fixture.asks.try_recv().is_err(),
            "no form is shown for content that is already published"
        );
        assert_eq!(
            fixture.only_plan().status,
            EnvironmentPlanStatus::Published {
                id: published.id,
                name_version: 1,
            }
        );
    }

    /// T1 (§D2B): a Pilot cancelled while the form is open leaves a `Cancelled`
    /// record — never a `requested` one a human can no longer answer.
    #[tokio::test]
    async fn a_cancelled_pilot_leaves_a_cancelled_record_not_a_live_request() {
        let mut fixture = Fixture::start(Form::Silent).await;
        let call = call_tool(
            &fixture.client,
            "environment_publish",
            json!({"directory":CANDIDATE,"name":NAME}),
        );
        tokio::pin!(call);
        // The tool is now waiting for the native form to be answered.
        tokio::select! {
            _ = &mut call => panic!("the tool answered before the form was answered"),
            ask = fixture.asks.recv() => assert!(ask.is_some(), "the native form was asked"),
        }
        fixture.cancellation.store(true, Ordering::SeqCst);
        let (failed, result) = call.await;
        assert!(failed, "a cancelled Pilot is a tool failure: {result}");
        assert_eq!(
            fixture.only_plan().status,
            EnvironmentPlanStatus::Cancelled {
                reason: FORM_ABANDONED.to_owned(),
            }
        );
    }

    /// T3 (§D2B): a form that cannot be transported asked nobody, so the record
    /// is `Cancelled` too — and never stays `requested`.
    #[tokio::test]
    async fn an_undeliverable_form_leaves_a_cancelled_record() {
        let mut fixture = Fixture::start(Form::Unreachable).await;
        let (failed, result) = fixture
            .call(
                "environment_publish",
                json!({"directory":CANDIDATE,"name":NAME}),
            )
            .await;
        assert!(failed, "{result}");
        assert!(
            result
                .as_str()
                .is_some_and(|reason| reason.contains("could not complete")),
            "{result}"
        );
        assert!(
            fixture.asks.try_recv().is_ok(),
            "the form really was attempted"
        );
        assert_eq!(
            fixture.only_plan().status,
            EnvironmentPlanStatus::Cancelled {
                reason: FORM_UNREACHED.to_owned(),
            }
        );
    }

    /// The build path has the same shape as the publish path: a form nobody
    /// answered leaves a `Cancelled` build record, never a live request.
    #[tokio::test]
    async fn a_cancelled_build_form_leaves_a_cancelled_record() {
        use crate::application::environments::{
            EnvironmentProvisionPlan, EnvironmentProvisionStatus, EnvironmentRequirementAsk,
            list_environment_provision_plans,
        };
        use crate::environment_provision::LockedPackage;

        let fixture = Fixture::start(Form::Silent).await;
        let requester = crate::application::environments::EnvironmentPlanRequester {
            owner: "local".into(),
            session: "pilot".into(),
            turn: "turn-1".into(),
        };
        let plan = EnvironmentProvisionPlan {
            requester: requester.clone(),
            name: NAME.into(),
            ask: EnvironmentRequirementAsk {
                requester,
                purpose: "analyse documents".into(),
                interpreter: Some("3.12".into()),
                packages: vec!["pandas==2.2.0".into()],
                notes: None,
            },
            sources: vec!["index:https://pypi.org/simple".into()],
            interpreter: "3.12.13".into(),
            interpreter_path: std::path::PathBuf::from("/usr/bin/python3.12"),
            interpreter_prefix: std::path::PathBuf::from("/usr"),
            interpreter_digest: "a".repeat(64),
            lock_digest: "b".repeat(64),
            lock: String::new(),
            packages: vec![LockedPackage {
                name: "pandas".into(),
                version: "2.2.0".into(),
                hashes: vec!["c".repeat(64)],
            }],
            references: None,
            observed_id: None,
            observed_present: false,
            observed_name_version: None,
            status: EnvironmentProvisionStatus::Requested,
            created: 0,
        };
        // The precondition writes it before the human is asked.
        fixture
            .tools
            .application
            .write_environment_provision_plan(&plan)
            .unwrap();

        cancel_environment_provision_plan(&fixture.tools, &plan, FORM_ABANDONED);

        let listed = list_environment_provision_plans(&fixture.tools.data_root).unwrap();
        assert_eq!(listed.records.len(), 1, "{:?}", listed.records);
        assert_eq!(
            listed.records[0].status,
            EnvironmentProvisionStatus::Cancelled {
                reason: FORM_ABANDONED.to_owned(),
            }
        );
    }

    /// One durable plan record for a test, as the approval paths write it.
    fn requested_plan(turn: &str, digest: &str) -> EnvironmentPlan {
        EnvironmentPlan {
            requester: crate::application::environments::EnvironmentPlanRequester {
                owner: "local".into(),
                session: "pilot".into(),
                turn: turn.into(),
            },
            directory: std::path::PathBuf::from("/prepared"),
            name: NAME.into(),
            digest: digest.into(),
            bytes: 1,
            files: 1,
            observed_id: None,
            observed_present: false,
            observed_name_version: None,
            references: None,
            status: EnvironmentPlanStatus::Requested,
            created: 0,
        }
    }

    /// T9: the Turn can end between the human's answer and the act. The answer
    /// is already known, so the record keeps it — `Aborted` for an approval that
    /// never ran, `Refused` for a "no" — and never stays a live request.
    ///
    /// The race itself (a form answer and a Pilot cancellation becoming ready in
    /// the same `select!`) is not deterministically reproducible; this covers the
    /// handler that race calls.
    #[tokio::test]
    async fn a_turn_that_ends_after_the_answer_keeps_the_answer() {
        // Two candidates, so the two records do not collapse into one another.
        let approved = requested_plan("turn-approved", &"a".repeat(64));
        let refused = requested_plan("turn-refused", &"b".repeat(64));
        let fixture = Fixture::start(Form::Silent).await;
        for plan in [&approved, &refused] {
            fixture
                .tools
                .application
                .write_environment_plan(plan)
                .unwrap();
        }

        finish_environment_plan_when_turn_is_gone(&fixture.tools, &approved, true);
        finish_environment_plan_when_turn_is_gone(&fixture.tools, &refused, false);

        let listed = list_environment_plans(&fixture.tools.data_root).unwrap();
        assert_eq!(listed.records.len(), 2, "{:?}", listed.records);
        let recorded = |turn: &str| {
            listed
                .records
                .iter()
                .find(|plan| plan.requester.turn == turn)
                .unwrap()
        };
        assert_eq!(
            recorded("turn-approved").status,
            EnvironmentPlanStatus::Aborted {
                reason: TURN_ENDS_BEFORE_APPROVED_ACT.to_owned(),
            },
            "an approval that never ran is not a refusal and not a cancellation"
        );
        assert_eq!(
            recorded("turn-refused").status,
            EnvironmentPlanStatus::Refused
        );
        // The operator read model renders `"requested"` exactly when the status
        // is `Requested`, so a terminal status here is what "no longer shows
        // requested" means (the HTTP projection for both of these terminal
        // states is asserted in the api tests).
        assert!(
            listed
                .records
                .iter()
                .all(|plan| !matches!(plan.status, EnvironmentPlanStatus::Requested)),
            "nobody can answer these any more: {:?}",
            listed.records
        );
    }

    /// T9 end to end: the human answers, but the Turn is over by the time the
    /// answer is handed back. The answer is already known, so it is still
    /// recorded, and the plan never stays a live request.
    async fn answer_after_the_turn_ended(content: Value, expected: EnvironmentPlanStatus) {
        let fixture = Fixture::start(Form::AcceptAfterTurnEnded(content)).await;
        let (failed, result) = fixture
            .call(
                "environment_publish",
                json!({"directory":CANDIDATE,"name":NAME}),
            )
            .await;
        assert!(
            failed,
            "the Turn is gone, so the act does not run: {result}"
        );
        assert_eq!(fixture.only_plan().status, expected);
    }

    #[tokio::test]
    async fn an_approved_answer_that_arrives_after_its_turn_ended_is_recorded() {
        answer_after_the_turn_ended(
            json!({"confirm": true}),
            EnvironmentPlanStatus::Aborted {
                reason: TURN_ENDS_BEFORE_APPROVED_ACT.to_owned(),
            },
        )
        .await;
    }

    #[tokio::test]
    async fn a_refused_answer_that_arrives_after_its_turn_ended_is_recorded() {
        answer_after_the_turn_ended(json!({"confirm": false}), EnvironmentPlanStatus::Refused)
            .await;
    }

    /// T9 for the build family: the same handler, the same two terminal states.
    #[tokio::test]
    async fn a_build_turn_that_ends_after_the_answer_keeps_the_answer() {
        use crate::application::environments::{
            EnvironmentProvisionPlan, EnvironmentProvisionStatus, EnvironmentRequirementAsk,
            list_environment_provision_plans,
        };
        use crate::environment_provision::LockedPackage;

        let fixture = Fixture::start(Form::Silent).await;
        let requester = crate::application::environments::EnvironmentPlanRequester {
            owner: "local".into(),
            session: "pilot".into(),
            turn: "turn-1".into(),
        };
        let plan = |lock: &str, status: EnvironmentProvisionStatus| EnvironmentProvisionPlan {
            requester: requester.clone(),
            name: NAME.into(),
            ask: EnvironmentRequirementAsk {
                requester: requester.clone(),
                purpose: "analyse documents".into(),
                interpreter: Some("3.12".into()),
                packages: vec!["pandas==2.2.0".into()],
                notes: None,
            },
            sources: vec!["index:https://pypi.org/simple".into()],
            interpreter: "3.12.13".into(),
            interpreter_path: std::path::PathBuf::from("/usr/bin/python3.12"),
            interpreter_prefix: std::path::PathBuf::from("/usr"),
            interpreter_digest: "a".repeat(64),
            lock_digest: lock.into(),
            lock: String::new(),
            packages: vec![LockedPackage {
                name: "pandas".into(),
                version: "2.2.0".into(),
                hashes: vec!["c".repeat(64)],
            }],
            references: None,
            observed_id: None,
            observed_present: false,
            observed_name_version: None,
            status,
            created: 0,
        };
        let approved = plan(&"1".repeat(64), EnvironmentProvisionStatus::Requested);
        let refused = plan(&"2".repeat(64), EnvironmentProvisionStatus::Requested);
        for record in [&approved, &refused] {
            fixture
                .tools
                .application
                .write_environment_provision_plan(record)
                .unwrap();
        }

        finish_environment_provision_plan_when_turn_is_gone(&fixture.tools, &approved, true);
        finish_environment_provision_plan_when_turn_is_gone(&fixture.tools, &refused, false);

        let listed = list_environment_provision_plans(&fixture.tools.data_root).unwrap();
        assert_eq!(listed.records.len(), 2, "{:?}", listed.records);
        let recorded = |lock: &str| {
            listed
                .records
                .iter()
                .find(|plan| plan.lock_digest == lock)
                .unwrap()
        };
        assert_eq!(
            recorded(&"1".repeat(64)).status,
            EnvironmentProvisionStatus::Aborted {
                reason: TURN_ENDS_BEFORE_APPROVED_ACT.to_owned(),
            }
        );
        assert_eq!(
            recorded(&"2".repeat(64)).status,
            EnvironmentProvisionStatus::Refused
        );
        assert!(
            listed
                .records
                .iter()
                .all(|plan| !matches!(plan.status, EnvironmentProvisionStatus::Requested)),
            "{:?}",
            listed.records
        );
    }

    /// A human who answers "no" in the form is recorded as a refusal, not as a
    /// cancellation: the three terminal states may not be mixed up (I2).
    #[tokio::test]
    async fn a_declined_form_is_recorded_as_a_refusal() {
        let fixture = Fixture::start(Form::Accept(json!({"confirm": false}))).await;
        let (failed, result) = fixture
            .call(
                "environment_publish",
                json!({"directory":CANDIDATE,"name":NAME}),
            )
            .await;
        assert!(!failed, "{result}");
        assert_eq!(result["plan_recorded"], json!(true), "{result}");
        assert_eq!(fixture.only_plan().status, EnvironmentPlanStatus::Refused);
    }
}
