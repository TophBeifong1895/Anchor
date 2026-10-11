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

fn needs_view_repair(
    already_installed: bool,
    dependencies: &anchor_graph_host::PluginDependencies,
    library_root: &std::path::Path,
    id: &str,
) -> bool {
    already_installed
        && !dependencies.is_empty()
        && crate::plugin_view::view_needs_repair(library_root, id)
}

fn install_message(precondition: &PluginInstallPrecondition) -> String {
    let review = &precondition.review;
    let mut message = format!(
        "确认把插件「{}」安装进 Anchor 插件库？\n内容指纹：{}\n",
        review.id, review.digest
    );
    message.push_str(
        if precondition.already_installed() && precondition.dependencies.is_some() {
            "该 id 已安装且内容指纹相同；本次不会改写插件，只补建它声明的依赖视图。\n"
        } else if precondition.replacing() {
            "该 id 已安装，本次会覆盖现有版本。\n"
        } else {
            "这是新建，不会覆盖任何现有插件。\n"
        },
    );
    // Which Graphs bind the Plugin is still worth showing; what it does to Runs
    // is not a question any more. Upgrading takes effect from each Run's next node
    // call and strands nothing, so there is nothing to confirm and nothing that
    // will refuse to continue.
    let impact = &precondition.impact;
    if impact.graphs.is_empty() {
        message.push_str("当前没有 Graph 绑定这个插件。\n");
    } else {
        message.push_str(&format!(
            "当前依赖这个插件：{} 张图（{}）\n",
            impact.graphs.len(),
            impact.graphs.join(", ")
        ));
    }
    if precondition.replacing() && !precondition.already_installed() {
        message.push_str(
            "升级从每个正在跑的 Run 的**下一次节点调用**起生效；已完成的步骤不回滚、不重放。\n",
        );
    }
    // Outside the branch on purpose: an empty count beside a partial scan would
    // otherwise read as "nothing depends on it" when a Graph could not be read.
    render_coverage(&mut message, &impact.coverage);
    // A view placeholder stands for a deployment path, and the human is being
    // asked to approve that path: show the interpreter the server will run on,
    // and the packages the declaration resolves to.
    let view = precondition
        .dependencies
        .as_ref()
        .map(|plan| plan.view.clone());
    let shown = |value: &str| match view.as_ref() {
        Some(view) => value.replace("${ANCHOR_PLUGIN_VIEW}", &view.to_string_lossy()),
        None => value.to_owned(),
    };
    if let Some(plan) = &precondition.dependencies {
        message.push_str(&format!(
            "该插件声明了 Python 依赖，宿主按部署授权的来源解析并安装：\n解释器：{}（{}）\n来源：{}\n视图：{}\n",
            plan.interpreter,
            plan.interpreter_path.display(),
            if plan.sources.is_empty() {
                "部署未列出来源".to_owned()
            } else {
                plan.sources.join(", ")
            },
            plan.view.display(),
        ));
        for package in &plan.packages {
            message.push_str(&format!("  - {}=={}\n", package.name, package.version));
        }
    }
    for server in &review.servers {
        match (&server.command, &server.url) {
            (Some(command), _) if server.args.is_empty() => {
                message.push_str(&format!("会运行：{}（{}）\n", shown(command), server.name));
            }
            (Some(command), _) => {
                message.push_str(&format!(
                    "会运行：{} {}（{}）\n",
                    shown(command),
                    server
                        .args
                        .iter()
                        .map(|argument| shown(argument))
                        .collect::<Vec<_>>()
                        .join(" "),
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
    if bundle.plugins.iter().any(|plugin| plugin == &id) {
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
    // An already-published tree normally means there is nothing to do. The one
    // exception is a dependency view that is missing or unusable: that is exactly
    // the state a repair has to go through, so it is not reported as "up to date".
    let repairing = needs_view_repair(
        precondition.already_installed(),
        &precondition.review.dependencies,
        &tools.library_root,
        &precondition.review.id,
    );
    if precondition.already_installed() && !repairing {
        return Ok(vec![ToolResultContent::text(
            json!({"installed":false,"already_installed":true,"id":precondition.review.id,"digest":precondition.review.digest})
                .to_string(),
        )]);
    }
    // Repairing a view rewrites no Plugin bytes, so it is not a replacement and
    // must not be confirmed as one.
    let replacing = precondition.replacing() && !precondition.already_installed();
    let mut properties = serde_json::Map::new();
    properties.insert(
        "confirm".into(),
        json!({"type":"boolean","title":"确认安装这个插件","default":false}),
    );
    if replacing {
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
        && (!replacing || approved("replace_existing"))
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
    // The answer names what the install produced, including the view and the
    // packages it landed, so an operator can see the solved set without reading
    // the host's filesystem.
    let mut result = json!({
        "installed":true,
        "id":outcome.id,
        "digest":outcome.digest,
        "replaced":precondition.replacing()
    });
    if let Some(plan) = &precondition.dependencies {
        result["dependencies"] = json!({
            "view": plan.view,
            "declaration_digest": plan.declaration_digest,
            "interpreter": plan.interpreter,
            "lock_digest": plan.lock_digest,
            "packages": plan
                .packages
                .iter()
                .map(|package| json!({"name":package.name,"version":package.version}))
                .collect::<Vec<_>>(),
        });
    }
    Ok(vec![ToolResultContent::text(result.to_string())])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The form shows what will actually run: a dependency view is a real path,
    /// not the placeholder the manifest declares, and the packages the
    /// declaration resolved to are named before anybody approves them.
    #[test]
    fn the_install_form_expands_the_dependency_view_for_the_human() {
        use crate::application::graphs::ImpactCoverage;
        use crate::application::plugins::{PluginDependencyPlan, PluginImpact};
        use crate::dependency_provision::LockedPackage;

        let root = tempfile::tempdir().unwrap();
        let candidate = root.path().join("candidate");
        std::fs::create_dir_all(&candidate).unwrap();
        std::fs::write(
            candidate.join("plugin.json"),
            json!({
                "name": "Probe",
                "dependencies": {"python": ["idna==3.6"]},
                "mcpServers": {
                    "probe": {"command": "${ANCHOR_PLUGIN_VIEW}/bin/python", "args": ["server.py"]}
                }
            })
            .to_string(),
        )
        .unwrap();
        let review = anchor_library::Library::new(root.path().join("library"))
            .review_directory("probe", &candidate)
            .unwrap();
        let view = root.path().join("library/dependencies/views/probe/abc123");
        let precondition = PluginInstallPrecondition {
            review,
            directory: candidate,
            existing_digest: None,
            impact: PluginImpact {
                graphs: Vec::new(),
                coverage: ImpactCoverage::Complete,
            },
            dependencies: Some(PluginDependencyPlan {
                sources: vec!["directory:/opt/wheels".into()],
                interpreter: "3.12.13".into(),
                interpreter_path: std::path::PathBuf::from("/usr/bin/python3.12"),
                interpreter_digest: "f".repeat(64),
                lock_digest: "a".repeat(64),
                packages: vec![LockedPackage {
                    name: "idna".into(),
                    version: "3.6".into(),
                    hashes: vec!["b".repeat(64)],
                }],
                view: view.clone(),
                declaration_digest: "c".repeat(64),
            }),
            resolved: None,
        };

        let message = install_message(&precondition);
        assert!(
            message.contains(&view.join("bin/python").to_string_lossy().into_owned()),
            "the form must show the interpreter the server will run on: {message}"
        );
        assert!(
            !message.contains("ANCHOR_PLUGIN_VIEW"),
            "the placeholder must not reach the human: {message}"
        );
        assert!(message.contains("idna==3.6"), "{message}");
        assert!(message.contains("directory:/opt/wheels"), "{message}");
    }

    /// An already-installed Plugin whose view is gone is not "up to date": the
    /// confirmation gate is the only installation entry point, so it is the one
    /// that has to offer the repair.
    #[test]
    fn an_already_installed_plugin_without_its_view_is_repair_work() {
        let root = tempfile::tempdir().unwrap();
        let library = root.path().join("library");
        let plugin = library.join("plugins/probe");
        std::fs::create_dir_all(&plugin).unwrap();
        std::fs::write(
            plugin.join("plugin.json"),
            json!({
                "name": "Probe",
                "dependencies": {"python": ["idna==3.6"]},
                "mcpServers": {"probe": {"command": "${ANCHOR_PLUGIN_VIEW}/bin/python"}}
            })
            .to_string(),
        )
        .unwrap();
        let dependencies = crate::plugin_view::prepare_for_plugin(&library, "probe");
        assert!(
            dependencies.is_err(),
            "no view exists yet: {dependencies:?}"
        );
        assert!(needs_view_repair(
            true,
            &anchor_graph_host::PluginDependencies {
                python: vec!["idna==3.6".to_owned()]
            },
            &library,
            "probe"
        ));

        // A Plugin that declares nothing needs no repair, and neither does one
        // whose view is complete.
        assert!(!needs_view_repair(
            true,
            &anchor_graph_host::PluginDependencies::default(),
            &library,
            "probe"
        ));
        assert!(!needs_view_repair(
            false,
            &Default::default(),
            &library,
            "probe"
        ));

        // A complete view: the file the standard requires, an interpreter, and a
        // pointer into the shared store.
        let store = anchor_graph_host::store_root(&library.canonicalize().unwrap());
        std::fs::create_dir_all(store.join("idna/3.6/idna")).unwrap();
        let view = anchor_graph_host::plugin_view_path(
            &library.canonicalize().unwrap(),
            "probe",
            &anchor_graph_host::PluginDependencies {
                python: vec!["idna==3.6".to_owned()],
            }
            .declaration_digest(),
        );
        let site = view.join("lib/python3.12/site-packages");
        std::fs::create_dir_all(&site).unwrap();
        std::fs::create_dir_all(view.join("bin")).unwrap();
        std::fs::write(view.join("pyvenv.cfg"), "home = /usr/bin").unwrap();
        std::os::unix::fs::symlink("/bin/sh", view.join("bin/python")).unwrap();
        std::os::unix::fs::symlink(store.join("idna/3.6/idna"), site.join("idna")).unwrap();
        assert!(!needs_view_repair(
            true,
            &anchor_graph_host::PluginDependencies {
                python: vec!["idna==3.6".to_owned()]
            },
            &library,
            "probe"
        ));
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
}
