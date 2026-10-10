use std::{
    future::Future,
    io::Read,
    path::{Path, PathBuf},
    pin::Pin,
};

use anchor_graph_host::{FilePluginCatalog, PluginCatalog, PluginDefinition};
use anchor_platform_session::SessionStore;
use anchor_runtime::{
    ToolDefinition, ToolError, ToolPort, ToolResultContent, graph::GraphSnapshot,
};
use serde_json::{Value, json};

mod environments;

use crate::{
    application::{AdmissionOptions, RunApplication, RunTrigger, metadata::PilotRunSource},
    artifacts::HostArtifacts,
    resource_read::open_resource,
};

/// A tool that takes no arguments.
fn object() -> Value {
    json!({"type":"object","properties":{},"additionalProperties":false})
}

/// A tool that takes exactly one required string argument.
fn single(field: &str) -> Value {
    json!({"type":"object","properties":{field:{"type":"string"}},"required":[field],"additionalProperties":false})
}

/// How a Pilot tool may be used.
///
/// `Inspect` tools only read. `Mutate` tools change something and are dispatched
/// to `PilotTools` directly — today without a native form, which is recorded
/// debt for `graph_create`/`graph_update`/`graph_run`/`run_pause`/`run_resume`/
/// `run_stop`. `Confirm` tools are owned by the interaction layer, which shows a
/// native form the model cannot fill in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ToolAuthority {
    Inspect,
    Mutate,
    Confirm,
}

/// What calling a tool can touch.
///
/// `ToolAuthority` says *who may call it*; this says *what it can affect*, so
/// "only assets need a human click" is a property of the table rather than a
/// sentence in a document.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ToolEffect {
    /// Reads only.
    Read,
    /// Writes a host-side fact a human consumes (a request, a question).
    HostFact,
    /// Changes a saved asset: a Graph, a Plugin, a published environment.
    Asset,
    /// Creates an execution.
    Execution,
    /// Controls something already running.
    Control,
}

pub(crate) struct PilotToolSpec {
    pub(crate) name: &'static str,
    pub(crate) description: &'static str,
    pub(crate) parameters: fn() -> Value,
    pub(crate) authority: ToolAuthority,
    pub(crate) effect: ToolEffect,
}

/// The single source of truth for the Pilot tool surface: definitions, the
/// read-only flag and which layer dispatches a call all derive from this table,
/// so a new tool cannot be added to one list and forgotten in another.
pub(crate) const PILOT_TOOLS: &[PilotToolSpec] = &[
    PilotToolSpec {
        name: "graph_list",
        description: "List saved Graphs",
        parameters: || object(),
        authority: ToolAuthority::Inspect,
        effect: ToolEffect::Read,
    },
    PilotToolSpec {
        name: "graph_read",
        description: "Read one saved Graph definition",
        parameters: || single("graph"),
        authority: ToolAuthority::Inspect,
        effect: ToolEffect::Read,
    },
    PilotToolSpec {
        name: "graph_validate",
        description: "Validate a Graph definition without saving or running it",
        parameters: || json!({"type":"object","properties":{"definition":{"type":"object"}},"required":["definition"],"additionalProperties":false}),
        authority: ToolAuthority::Inspect,
        effect: ToolEffect::Read,
    },
    PilotToolSpec {
        name: "graph_create",
        description: "Create a saved Graph the user explicitly requested; this does not run it",
        parameters: || json!({"type":"object","properties":{"name":{"type":"string"},"definition":{"type":"object"}},"required":["name","definition"],"additionalProperties":false}),
        authority: ToolAuthority::Mutate,
        effect: ToolEffect::Asset,
    },
    PilotToolSpec {
        name: "graph_update",
        description: "Update a saved Graph the user explicitly requested; frozen Run resources are preserved",
        parameters: || json!({"type":"object","properties":{"graph":{"type":"string"},"definition":{"type":"object"}},"required":["graph","definition"],"additionalProperties":false}),
        authority: ToolAuthority::Mutate,
        effect: ToolEffect::Asset,
    },
    PilotToolSpec {
        name: "graph_run",
        description: "Start the Graph the user explicitly requested; returns an accepted Run ID, not completion",
        parameters: || json!({"type":"object","properties":{"graph":{"type":"string"},"objective":{"type":"string"},"input":{}},"required":["graph"],"additionalProperties":false}),
        authority: ToolAuthority::Mutate,
        effect: ToolEffect::Execution,
    },
    PilotToolSpec {
        name: "plugin_list",
        description: "List installed Plugins and their availability",
        parameters: || object(),
        authority: ToolAuthority::Inspect,
        effect: ToolEffect::Read,
    },
    PilotToolSpec {
        name: "plugin_read",
        description: "Read one Plugin's instructions without executing it",
        parameters: || single("plugin"),
        authority: ToolAuthority::Inspect,
        effect: ToolEffect::Read,
    },
    PilotToolSpec {
        name: "plugin_review",
        description: "Review a Plugin candidate directory inside your workspace before any activation: returns its digest, the MCP servers it would run, the environment variable names it needs, and warnings. It installs nothing.",
        parameters: || json!({"type":"object","properties":{"directory":{"type":"string"},"id":{"type":"string"}},"required":["directory"],"additionalProperties":false}),
        authority: ToolAuthority::Inspect,
        effect: ToolEffect::Read,
    },
    PilotToolSpec {
        name: "environment_list",
        description: "List published dependency environments and the names that point at them. It prepares nothing.",
        parameters: || object(),
        authority: ToolAuthority::Inspect,
        effect: ToolEffect::Read,
    },
    PilotToolSpec {
        name: "environment_request",
        description: "Record what you need a dependency environment to provide (interpreter, packages, why) under a deployment name, so the operator can provide it. It publishes nothing and installs nothing.",
        parameters: || json!({"type":"object","properties":{"name":{"type":"string"},"purpose":{"type":"string"},"interpreter":{"type":"string"},"packages":{"type":"array","items":{"type":"string"}},"notes":{"type":"string"}},"required":["name","purpose"],"additionalProperties":false}),
        authority: ToolAuthority::Mutate,
        effect: ToolEffect::HostFact,
    },
    PilotToolSpec {
        name: "environment_references",
        description: "Report where one dependency environment name is used right now: the Graph nodes that declare it and the Plugin servers whose manifest declares it. Derived from the Graph catalog, changes nothing.",
        parameters: || single("name"),
        authority: ToolAuthority::Inspect,
        effect: ToolEffect::Read,
    },
    PilotToolSpec {
        name: "environment_show",
        description: "Read one published dependency environment by `id` or by the name it is published under; the content digest is re-verified before it is described. It changes nothing.",
        parameters: || json!({"type":"object","properties":{"id":{"type":"string"},"name":{"type":"string"}},"additionalProperties":false}),
        authority: ToolAuthority::Inspect,
        effect: ToolEffect::Read,
    },
    PilotToolSpec {
        name: "run_list",
        description: "List durable Graph Runs",
        parameters: || object(),
        authority: ToolAuthority::Inspect,
        effect: ToolEffect::Read,
    },
    PilotToolSpec {
        name: "run_status",
        description: "Read one durable Run state",
        parameters: || single("run"),
        authority: ToolAuthority::Inspect,
        effect: ToolEffect::Read,
    },
    PilotToolSpec {
        name: "run_pause",
        description: "Request a pause at the next Graph node boundary; inspect run_status afterwards",
        parameters: || single("run"),
        authority: ToolAuthority::Mutate,
        effect: ToolEffect::Control,
    },
    PilotToolSpec {
        name: "run_resume",
        description: "Resume an existing Run through the same Runner and frozen resources",
        parameters: || single("run"),
        authority: ToolAuthority::Mutate,
        effect: ToolEffect::Control,
    },
    PilotToolSpec {
        name: "run_stop",
        description: "Request cancellation of an existing Run; inspect run_status afterwards",
        parameters: || single("run"),
        authority: ToolAuthority::Mutate,
        effect: ToolEffect::Control,
    },
    PilotToolSpec {
        name: "artifact_read",
        description: "Read a committed UTF-8 artifact (at most 1 MiB), not a live workspace",
        parameters: || json!({"type":"object","properties":{"run":{"type":"string"},"node":{"type":"string"},"path":{"type":"string"}},"required":["run","node","path"],"additionalProperties":false}),
        authority: ToolAuthority::Inspect,
        effect: ToolEffect::Read,
    },
    PilotToolSpec {
        name: "session_wait",
        description: "Read this Session and its associated Runs without waiting",
        parameters: object,
        authority: ToolAuthority::Inspect,
        effect: ToolEffect::Read,
    },
    PilotToolSpec {
        name: "environment_publish",
        description: "Publish a dependency environment you prepared inside your own workspace and point a deployment name at it. The user approves the exact content in a native form; approval is never a tool argument.",
        parameters: || json!({"type":"object","properties":{"directory":{"type":"string"},"name":{"type":"string"}},"required":["directory","name"],"additionalProperties":false}),
        authority: ToolAuthority::Confirm,
        effect: ToolEffect::Asset,
    },
    PilotToolSpec {
        name: "environment_provision",
        description: "Build and publish the dependency environment the recorded requirement asks for. The user approves the exact resolved package set in a native form; approval is never a tool argument. Record the requirement first.",
        parameters: || single("name"),
        authority: ToolAuthority::Confirm,
        effect: ToolEffect::Asset,
    },
    PilotToolSpec {
        name: "plugin_install",
        description: "Install a Plugin candidate you prepared inside your own workspace into the Anchor Library. The user approves the exact reviewed content in a native form; approval is never a tool argument.",
        parameters: || json!({"type":"object","properties":{"directory":{"type":"string"},"id":{"type":"string"}},"required":["directory"],"additionalProperties":false}),
        authority: ToolAuthority::Confirm,
        effect: ToolEffect::Asset,
    },
    PilotToolSpec {
        name: "graph_delete",
        description: "Delete a saved Graph only after a fresh native user confirmation of its exact current contents.",
        parameters: || json!({"type":"object","properties":{"graph":{"type":"string"}},"required":["graph"],"additionalProperties":false}),
        authority: ToolAuthority::Confirm,
        effect: ToolEffect::Asset,
    },
    PilotToolSpec {
        name: "ask_user",
        description: "Ask the user for necessary information using a native form; the current Turn waits for their answer.",
        parameters: || json!({"type":"object","properties":{"message":{"type":"string"},"requested_schema":{"type":"object"}},"required":["message","requested_schema"],"additionalProperties":false}),
        authority: ToolAuthority::Confirm,
        effect: ToolEffect::HostFact,
    },
];

/// The authority recorded for a tool name, or `None` for an unknown tool.
pub(crate) fn authority(name: &str) -> Option<ToolAuthority> {
    PILOT_TOOLS
        .iter()
        .find(|tool| tool.name == name)
        .map(|tool| tool.authority)
}

/// What calling this tool can affect, or `None` for an unknown tool.
pub(crate) fn effect(name: &str) -> Option<ToolEffect> {
    PILOT_TOOLS
        .iter()
        .find(|tool| tool.name == name)
        .map(|tool| tool.effect)
}

pub(crate) struct PilotTools {
    pub(crate) application: RunApplication,
    pub(crate) graph_name: String,
    pub(crate) catalog_root: PathBuf,
    pub(crate) library_root: PathBuf,
    pub(crate) data_root: PathBuf,
    pub(crate) workspace_root: PathBuf,
    pub(crate) sessions: SessionStore,
    pub(crate) owner: String,
    pub(crate) session: String,
    pub(crate) turn: String,
}

pub(crate) fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-_.".contains(character))
}

/// An optional string argument that must be non-empty and bounded when present.
fn optional_bounded(
    arguments: &Value,
    name: &str,
    limit: usize,
) -> Result<Option<String>, ToolError> {
    match arguments.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if !value.trim().is_empty() && value.len() <= limit => {
            Ok(Some(value.clone()))
        }
        Some(_) => Err(rejected(format!(
            "`{name}` must be a non-empty string of at most {limit} characters"
        ))),
    }
}

fn parameter(arguments: &Value, name: &str) -> Result<String, ToolError> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| ToolError::Failed(format!("{name} must be a string")))
}

fn failed() -> ToolError {
    ToolError::Failed("resource unavailable or malformed".into())
}

/// A refusal whose reason the assistant is allowed to read and act on.
fn rejected(reason: impl Into<String>) -> ToolError {
    ToolError::Failed(reason.into())
}

/// Resolve a candidate directory *inside* the assistant's own workspace.
///
/// The model never names a host path: it names a workspace-relative directory,
/// and this resolves and confines it. Symlinks that leave the workspace, `..`,
/// and absolute paths are refused before any review work happens.
pub(crate) fn workspace_directory(workspace: &Path, relative: &str) -> Result<PathBuf, ToolError> {
    let path = Path::new(relative);
    if relative.is_empty()
        || relative.contains('\0')
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(rejected(
            "directory must be a workspace-relative path without `.` or `..` components",
        ));
    }
    let root = workspace
        .canonicalize()
        .map_err(|_| rejected("the assistant workspace is unavailable"))?;
    let candidate = root
        .join(path)
        .canonicalize()
        .map_err(|_| rejected("the candidate directory does not exist"))?;
    if !candidate.starts_with(&root) || !candidate.is_dir() {
        return Err(rejected("the candidate directory escapes the workspace"));
    }
    Ok(candidate)
}

fn public_plugin(definition: &PluginDefinition) -> Value {
    json!({"id":definition.id,"name":definition.name,"description":definition.description,
        "skills":definition.skills,"available":true,"digest":definition.digest})
}

fn text_resource(root: &Path, relative: &str) -> Result<String, ToolError> {
    let mut bytes = Vec::new();
    open_resource(root, relative)
        .map_err(|_| failed())?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| failed())?;
    if bytes.len() > 1024 * 1024 {
        return Err(ToolError::Failed(
            "text resource exceeds 1 MiB preview limit".into(),
        ));
    }
    String::from_utf8(bytes)
        .map_err(|_| ToolError::Failed("binary artifact cannot be decoded as text".into()))
}

impl PilotTools {
    pub(crate) fn for_turn(mut self, turn: &str) -> Self {
        self.turn = turn.into();
        self
    }

    fn require_active_turn(&self) -> Result<(), ToolError> {
        let turn = self
            .sessions
            .get_turn(&self.owner, &self.session, &self.turn)
            .map_err(|_| failed())?;
        if turn.status != anchor_platform_session::TurnStatus::Running {
            return Err(ToolError::Failed("Pilot Turn is not active".into()));
        }
        Ok(())
    }

    /// Refuse to rewrite the Graph that defines this assistant.
    ///
    /// The Graph carries the assistant's role, instructions, plugin bindings and
    /// therefore its own capabilities. Letting the assistant edit it would be a
    /// self-escalation path, so the operator interface stays the only way to
    /// change it.
    pub(crate) fn refuse_self_graph(&self, graph: &str) -> Result<(), ToolError> {
        if graph == self.graph_name {
            return Err(rejected(format!(
                "`{graph}` defines this assistant; change it through the operator interface, not from inside the assistant"
            )));
        }
        Ok(())
    }

    async fn execute(&self, name: &str, arguments: Value) -> Result<Value, ToolError> {
        match name {
            "graph_create" => {
                let graph = parameter(&arguments, "name")?;
                self.refuse_self_graph(&graph)?;
                self.require_active_turn()?;
                self.application
                    .create_graph(&graph, arguments.get("definition").cloned())
                    .await
                    .map_err(|failure| {
                        ToolError::Failed(format!("Graph creation rejected: {failure:?}"))
                    })
            }
            "graph_update" => {
                let graph = parameter(&arguments, "graph")?;
                // A self-target is refused before anything else: the Graph that
                // defines the assistant is not something the assistant may
                // rewrite, regardless of Turn state.
                self.refuse_self_graph(&graph)?;
                self.require_active_turn()?;
                let definition = arguments
                    .get("definition")
                    .filter(|value| value.is_object())
                    .cloned()
                    .ok_or_else(|| ToolError::Failed("definition must be an object".into()))?;
                self.application
                    .update_graph(&graph, definition)
                    .await
                    .map_err(|failure| {
                        ToolError::Failed(format!("Graph update rejected: {failure:?}"))
                    })
            }
            "graph_run" => {
                self.require_active_turn()?;
                let graph = parameter(&arguments, "graph")?;
                let objective = match arguments.get("objective") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(value)) => Some(value.clone()),
                    _ => return Err(ToolError::Failed("objective must be a string".into())),
                };
                let path = self
                    .application
                    .graph_path_checked(&graph)
                    .map_err(|failure| {
                        ToolError::Failed(format!("Graph unavailable: {failure:?}"))
                    })?;
                let lease = self
                    .application
                    .graph_admission_lease(&path)
                    .map_err(|failure| {
                        ToolError::Failed(format!("Run admission rejected: {failure:?}"))
                    })?;
                let (path, bundle) = self.application.load_graph(&graph).map_err(|failure| {
                    ToolError::Failed(format!("Graph unavailable: {failure:?}"))
                })?;
                let run = self
                    .application
                    .admit(
                        graph.clone(),
                        &path,
                        bundle,
                        arguments.get("input").cloned().unwrap_or_else(|| json!({})),
                        AdmissionOptions {
                            objective,
                            trigger: RunTrigger::default(),
                            pilot: Some(PilotRunSource {
                                owner: self.owner.clone(),
                                session: self.session.clone(),
                                turn: self.turn.clone(),
                            }),
                            oauth_owner: Some(crate::api::oauth::binding_owner(&self.owner)),
                        },
                        lease,
                    )
                    .await
                    .map_err(|failure| {
                        ToolError::Failed(format!("Run admission rejected: {failure:?}"))
                    })?;
                let mut result =
                    json!({"run":run,"graph":graph,"session":self.session,"accepted":true});
                if self
                    .sessions
                    .associate_run(&self.owner, &self.session, &self.turn, &run)
                    .is_err()
                {
                    result["association_error"] = json!(
                        "Run accepted; Session association pending reconciliation. Do not retry graph_run; inspect this Run ID."
                    );
                }
                Ok(result)
            }
            "run_pause" | "run_resume" | "run_stop" => {
                self.require_active_turn()?;
                let run = parameter(&arguments, "run")?;
                if !identifier(&run) {
                    return Err(failed());
                }
                let operation = name.strip_prefix("run_").expect("matched Run control");
                self.application
                    .control(&run, operation)
                    .await
                    .map_err(|failure| {
                        ToolError::Failed(format!("Run control rejected: {failure:?}"))
                    })?;
                Ok(json!({"run":run,"operation":operation,"requested":true}))
            }
            "graph_list" => {
                let mut names = vec![self.graph_name.clone()];
                if let Ok(entries) = std::fs::read_dir(&self.catalog_root) {
                    for entry in entries {
                        let entry = entry.map_err(|_| failed())?;
                        let name = entry.file_name().to_string_lossy().into_owned();
                        if identifier(&name)
                            && !names.contains(&name)
                            && entry.file_type().is_ok_and(|kind| kind.is_dir())
                            && entry.path().join("graph.json").is_file()
                        {
                            names.push(name);
                        }
                    }
                }
                names.sort();
                let mut graphs = Vec::new();
                for graph in names {
                    let active = self.application.active_runs(Some(&graph)).await;
                    graphs.push(json!({"graph":graph,"active_runs":active}));
                }
                Ok(json!({"graphs":graphs}))
            }
            "graph_read" => {
                let graph = parameter(&arguments, "graph")?;
                if !identifier(&graph) {
                    return Err(failed());
                }
                let definition =
                    text_resource(&self.application.graph_bundle_path(&graph), "graph.json")?;
                Ok(
                    json!({"graph":graph,"definition":serde_json::from_str::<Value>(&definition).map_err(|_| failed())?}),
                )
            }
            "graph_validate" => {
                let definition = arguments
                    .get("definition")
                    .filter(|value| value.is_object())
                    .ok_or_else(failed)?;
                let validation = (|| {
                    let snapshot =
                        GraphSnapshot::from_authoring(definition.clone()).map_err(|_| failed())?;
                    crate::reject_snapshot(&snapshot).map_err(|_| failed())?;
                    let ids = snapshot
                        .nodes
                        .iter()
                        .flat_map(|node| node.plugins.iter().cloned())
                        .collect::<std::collections::BTreeSet<_>>()
                        .into_iter()
                        .collect::<Vec<_>>();
                    FilePluginCatalog::new(&self.library_root)
                        .resolve(&ids)
                        .map_err(|_| failed())?;
                    Ok::<_, ToolError>(snapshot)
                })();
                match validation {
                    Ok(snapshot) => Ok(
                        json!({"valid":true,"entry":snapshot.entry,"nodes":snapshot.nodes.iter().map(|node| &node.id).collect::<Vec<_>>()}),
                    ),
                    Err(_) => {
                        Ok(json!({"valid":false,"error":"invalid Graph or unavailable Plugin"}))
                    }
                }
            }
            "plugin_list" => {
                let catalog = FilePluginCatalog::new(&self.library_root);
                let mut plugins = Vec::new();
                match std::fs::read_dir(self.library_root.join("plugins")) {
                    Ok(entries) => {
                        for entry in entries {
                            let entry = entry.map_err(|_| failed())?;
                            let id = entry.file_name().to_string_lossy().into_owned();
                            if id.starts_with('.') {
                                continue;
                            }
                            plugins.push(match catalog.definition(&id) {
                                Ok(definition) => public_plugin(&definition),
                                Err(_) => json!({"id":id,"available":false}),
                            });
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(_) => return Err(failed()),
                }
                plugins.sort_by(|left, right| left["id"].as_str().cmp(&right["id"].as_str()));
                Ok(json!({"plugins":plugins}))
            }
            "plugin_read" => {
                let plugin = parameter(&arguments, "plugin")?;
                let definition = FilePluginCatalog::new(&self.library_root)
                    .definition(&plugin)
                    .map_err(|_| failed())?;
                let mut result = public_plugin(&definition);
                let mut instructions = Vec::new();
                if definition.directory.join("instructions.md").is_file() {
                    instructions.push(text_resource(&definition.directory, "instructions.md")?);
                } else {
                    for skill in &definition.skills {
                        instructions.push(text_resource(&definition.directory, skill)?);
                    }
                }
                result["instructions"] = Value::String(instructions.join("\n\n"));
                Ok(result)
            }
            "plugin_review" => {
                let directory = parameter(&arguments, "directory")?;
                let candidate = workspace_directory(&self.workspace_root, &directory)?;
                let id = match arguments.get("id").and_then(Value::as_str) {
                    Some(id) if identifier(id) => id.to_owned(),
                    Some(_) => return Err(rejected("id must be a Plugin identifier")),
                    None => candidate
                        .file_name()
                        .and_then(|name| name.to_str())
                        .filter(|name| identifier(name))
                        .ok_or_else(|| {
                            rejected("the directory name is not a usable Plugin id; pass `id`")
                        })?
                        .to_owned(),
                };
                let review = anchor_library::Library::new(&self.library_root)
                    .review_directory(&id, &candidate)
                    .map_err(|failure| match failure {
                        // The assistant needs the catalog's reason to fix the
                        // manifest; nothing secret is echoed back.
                        anchor_library::InstallError::InvalidPluginReason(reason) => {
                            rejected(format!("Plugin candidate rejected: {reason}"))
                        }
                        anchor_library::InstallError::MissingManifest => {
                            rejected("Plugin candidate has no plugin.json manifest")
                        }
                        _ => rejected("Plugin candidate could not be reviewed"),
                    })?;
                serde_json::to_value(&review)
                    .map_err(|_| rejected("Plugin review could not be encoded"))
            }
            "environment_list" => self.environment_list(&arguments).await,
            "environment_request" => self.environment_request(&arguments).await,
            "environment_references" => self.environment_references(&arguments).await,
            "environment_show" => self.environment_show(&arguments).await,
            "run_list" => {
                let runs = self
                    .application
                    .records()
                    .map_err(|_| failed())?
                    .into_iter()
                    .map(|(id, record)| json!({"run":id,"status":record.status}))
                    .collect::<Vec<_>>();
                Ok(json!({"runs":runs}))
            }
            "run_status" => {
                let run = parameter(&arguments, "run")?;
                if !identifier(&run) {
                    return Err(failed());
                }
                let record = self
                    .application
                    .records()
                    .map_err(|_| failed())?
                    .into_iter()
                    .find(|(id, _)| id == &run)
                    .ok_or_else(failed)?
                    .1;
                Ok(json!({"run":run,"record":record}))
            }
            "artifact_read" => {
                let run = parameter(&arguments, "run")?;
                let node = parameter(&arguments, "node")?;
                let path = parameter(&arguments, "path")?;
                if !identifier(&run) {
                    return Err(failed());
                }
                let record = self
                    .application
                    .records()
                    .map_err(|_| failed())?
                    .into_iter()
                    .find(|(id, _)| id == &run)
                    .ok_or_else(failed)?
                    .1;
                let result = record
                    .results
                    .get(&node)
                    .and_then(|results| results.last())
                    .ok_or_else(failed)?;
                let artifacts = HostArtifacts::new(
                    self.data_root.join("artifacts"),
                    self.workspace_root.clone(),
                );
                let files = artifacts.files_path(&result.commit).map_err(|_| failed())?;
                let text = text_resource(&files, &path)?;
                Ok(json!({"run":run,"node":node,"path":path,"text":text}))
            }
            "session_wait" => {
                let session = self
                    .sessions
                    .get(&self.owner, &self.session)
                    .map_err(|_| failed())?;
                let retained = self
                    .application
                    .records()
                    .map_err(|_| failed())?
                    .into_iter()
                    .filter(|(id, _)| session.run_ids.contains(id))
                    .map(|(id, record)| json!({"run":id,"record":record}))
                    .collect::<Vec<_>>();
                Ok(json!({"session":session,"runs":retained}))
            }
            _ => Err(ToolError::Unknown(name.into())),
        }
    }
}

impl ToolPort for PilotTools {
    fn definitions(&self) -> Vec<ToolDefinition> {
        // Confirm tools are owned by the interaction layer, which advertises them
        // with its own definitions; this layer dispatches only Inspect and Mutate.
        PILOT_TOOLS
            .iter()
            .filter(|tool| tool.authority != ToolAuthority::Confirm)
            .map(|tool| ToolDefinition {
                name: tool.name.into(),
                description: tool.description.into(),
                parameters: (tool.parameters)(),
            })
            .collect()
    }

    fn is_read_only(&self, name: &str) -> bool {
        effect(name) == Some(ToolEffect::Read)
    }
    fn call<'a>(
        &'a self,
        name: &'a str,
        arguments: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ToolResultContent>, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            let result = self.execute(name, arguments).await?;
            Ok(vec![ToolResultContent::text(result.to_string())])
        })
    }
}

#[cfg(test)]
mod tests;
