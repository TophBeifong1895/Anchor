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
            "environment_list" => {
                let store = anchor_library::EnvironmentStore::new(&self.library_root);
                let names = store
                    .names()
                    .map_err(|_| failed())?
                    .into_iter()
                    .map(|entry| json!({"name":entry.name,"id":entry.id,"version":entry.version}))
                    .collect::<Vec<_>>();
                let environments = store
                    .list()
                    .map_err(|_| failed())?
                    .into_iter()
                    .map(|published| {
                        json!({"id":published.id,"bytes":published.bytes,"files":published.files})
                    })
                    .collect::<Vec<_>>();
                Ok(json!({"names":names,"environments":environments}))
            }
            "environment_request" => {
                // The model may ask for an environment and explain why; it may not
                // publish anything, so this is a Mutate tool rather than a Confirm
                // one: it writes only its own durable request. Approval fields are
                // refused here just as they are in the confirmed tools.
                self.require_active_turn()?;
                if let Some(key) = arguments.as_object().and_then(|object| {
                    object.keys().find(|key| {
                        !matches!(
                            key.as_str(),
                            "name" | "purpose" | "interpreter" | "packages" | "notes"
                        )
                    })
                }) {
                    return Err(rejected(format!(
                        "environment_request does not accept `{key}`"
                    )));
                }
                let name = parameter(&arguments, "name")?;
                if !anchor_runtime::graph::is_environment_name(&name) {
                    return Err(rejected(
                        "name must be at most 64 characters of letters, digits, dot, dash and underscore",
                    ));
                }
                let purpose = parameter(&arguments, "purpose")?;
                if purpose.trim().is_empty() || purpose.len() > 4096 {
                    return Err(rejected("purpose must be non-empty and bounded"));
                }
                let interpreter = optional_bounded(&arguments, "interpreter", 128)?;
                let notes = optional_bounded(&arguments, "notes", 4096)?;
                let packages = match arguments.get("packages") {
                    None => Vec::new(),
                    Some(Value::Array(items)) if items.len() <= 200 => items
                        .iter()
                        .map(|item| {
                            item.as_str()
                                .filter(|package| {
                                    !package.trim().is_empty()
                                        && package.len() <= 256
                                        && package.chars().all(|character| {
                                            character.is_ascii_alphanumeric()
                                                || "._-<>=!~*+,".contains(character)
                                        })
                                })
                                .map(str::to_owned)
                                .ok_or_else(|| rejected("packages must be bounded name strings"))
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                    Some(_) => return Err(rejected("packages must be an array of at most 200")),
                };
                // The assistant may not ask to change its own dependencies either,
                // and "its own" includes the Graphs it calls.
                let own = self
                    .application
                    .assistant_uses_environment_name(&self.graph_name, &name)
                    .map_err(|error| match error {
                        crate::application::ApplicationError::Invalid(reason) => rejected(reason),
                        _ => failed(),
                    })?;
                if own {
                    return Err(rejected(format!(
                        "`{name}` provides this assistant's own dependencies; ask the operator to change it through the operator interface"
                    )));
                }
                let ask = crate::application::environments::EnvironmentRequirementAsk {
                    requester: crate::application::environments::EnvironmentPlanRequester {
                        owner: self.owner.clone(),
                        session: self.session.clone(),
                        turn: self.turn.clone(),
                    },
                    purpose,
                    interpreter,
                    packages,
                    notes,
                };
                let requirement = self
                    .application
                    .ask_environment_requirement(&self.library_root, &name, ask)
                    .map_err(|error| match error {
                        // The model can act on both of these, so surface them.
                        crate::application::ApplicationError::Invalid(reason)
                        | crate::application::ApplicationError::Storage(reason) => {
                            ToolError::Failed(reason)
                        }
                        _ => failed(),
                    })?;
                let references = self
                    .application
                    .environment_references(&name)
                    .ok()
                    .map(|references| {
                        json!({
                            "nodes": references.nodes.len(),
                            "servers": references.servers.len()
                        })
                    })
                    .unwrap_or(Value::Null);
                Ok(json!({
                    "requested": true,
                    "name": name,
                    "asks": requirement.requests.len(),
                    "observed": requirement.observed,
                    "referenced_by": references
                }))
            }
            "environment_references" => {
                let name = parameter(&arguments, "name")?;
                let references = self
                    .application
                    .environment_references(&name)
                    .map_err(|_| failed())?;
                serde_json::to_value(&references).map_err(|_| failed())
            }
            "environment_show" => {
                let store = anchor_library::EnvironmentStore::new(&self.library_root);
                // Reject a present-but-unusable argument instead of silently
                // falling back to the other one.
                for field in ["id", "name"] {
                    if let Some(value) = arguments.get(field) {
                        let usable = value.as_str().is_some_and(|text| !text.is_empty());
                        if !usable {
                            return Err(rejected(format!(
                                "`{field}` must be a non-empty string when present"
                            )));
                        }
                    }
                }
                let id = match (
                    arguments.get("id").and_then(Value::as_str),
                    arguments.get("name").and_then(Value::as_str),
                ) {
                    (Some(id), None) => id.to_owned(),
                    (None, Some(name)) => store
                        .names()
                        .map_err(|_| failed())?
                        .into_iter()
                        .find(|entry| entry.name == name)
                        .map(|entry| entry.id)
                        .ok_or_else(|| rejected("no dependency environment has that name"))?,
                    _ => return Err(rejected("pass exactly one of `id` or `name`")),
                };
                // `describe` re-hashes the published tree and reports whether it
                // is still writable, so an operator can inspect a store that
                // admission would refuse. The reason names the environment: a
                // bare "content is missing" tells nobody which one.
                let published = store.describe(&id).map_err(|error| {
                    rejected(format!(
                        "dependency environment `{id}` is not usable: {error}"
                    ))
                })?;
                Ok(json!({
                    "id":published.id,
                    "bytes":published.bytes,
                    "files":published.files,
                    "directories":published.directories,
                    "verified": !published.writable,
                    "writable": published.writable,
                }))
            }
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
mod tests {
    use super::*;
    use anchor_platform_session::CreateSession;

    fn fixture() -> (tempfile::TempDir, PilotTools) {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("fixture")).unwrap();
        std::fs::write(root.path().join("fixture/graph.json"), r#"{"entry":"idle","ops":{"idle":{"run":"true"}},"nodes":[{"id":"idle","op":"idle"}],"edges":[]}"#).unwrap();
        std::fs::write(
            root.path().join("fixture/manifest.json"),
            r#"{"format":1,"graph":"graph.json","plugins":[]}"#,
        )
        .unwrap();
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
        let tools = PilotTools {
            application: RunApplication::new(root.path().join("state"), root.path().to_path_buf()),
            graph_name: "fixture".into(),
            catalog_root: root.path().to_path_buf(),
            library_root: root.path().to_path_buf(),
            data_root: root.path().join("state"),
            workspace_root: root.path().join("work"),
            sessions,
            owner: "local".into(),
            session: "pilot".into(),
            turn: String::new(),
        };
        (root, tools)
    }

    #[tokio::test]
    async fn assistant_reviews_a_workspace_candidate_without_installing_it() {
        let (root, tools) = fixture();
        let candidate = root.path().join("work/candidate");
        std::fs::create_dir_all(candidate.join("skills/check")).unwrap();
        std::fs::write(
            candidate.join("plugin.json"),
            r#"{"name":"Candidate","description":"from the workspace","skills":"skills","mcpServers":{"probe":{"command":"bin/run","args":["serve"],"env_vars":["CANDIDATE_TOKEN"]}}}"#,
        )
        .unwrap();
        std::fs::write(candidate.join("skills/check/SKILL.md"), "# check\n").unwrap();

        let review = tools
            .execute("plugin_review", json!({"directory":"candidate"}))
            .await
            .unwrap();
        assert_eq!(review["id"], "candidate");
        assert_eq!(review["name"], "Candidate");
        assert_eq!(
            review["resources"],
            json!(["plugin.json", "skills/check/SKILL.md"])
        );
        assert_eq!(review["servers"][0]["name"], "probe");
        assert_eq!(
            review["servers"][0]["environment"],
            json!(["CANDIDATE_TOKEN"])
        );
        assert!(
            !root.path().join("plugins").exists(),
            "a review must not publish anything"
        );
    }

    #[tokio::test]
    async fn assistant_review_refuses_paths_outside_its_workspace() {
        let (root, tools) = fixture();
        let workspace = root.path().join("work");
        std::fs::create_dir_all(workspace.join("candidate")).unwrap();
        std::fs::write(
            workspace.join("candidate/plugin.json"),
            r#"{"name":"Candidate"}"#,
        )
        .unwrap();
        std::fs::write(workspace.join("plain.txt"), "not a directory").unwrap();
        std::os::unix::fs::symlink(root.path().join("fixture"), workspace.join("escape")).unwrap();

        for directory in [
            "/etc",
            "../fixture",
            "candidate/../candidate",
            "escape",
            "plain.txt",
            "missing",
            "",
        ] {
            let error = tools
                .execute("plugin_review", json!({"directory":directory}))
                .await
                .unwrap_err();
            assert!(
                matches!(error, ToolError::Failed(_)),
                "{directory:?} was accepted"
            );
        }

        // The same candidate is accepted once addressed inside the workspace.
        assert!(
            tools
                .execute("plugin_review", json!({"directory":"candidate"}))
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn assistant_refuses_to_rewrite_or_delete_its_own_graph() {
        let (_root, tools) = fixture();
        let definition = json!({"entry":"idle","ops":{"idle":{"run":"true"}},"nodes":[{"id":"idle","op":"idle"}],"edges":[]});

        for error in [
            tools
                .execute(
                    "graph_update",
                    json!({"graph":"fixture","definition":definition}),
                )
                .await
                .unwrap_err(),
            tools
                .execute(
                    "graph_create",
                    json!({"name":"fixture","definition":definition}),
                )
                .await
                .unwrap_err(),
        ] {
            match error {
                ToolError::Failed(reason) => {
                    assert!(reason.contains("defines this assistant"), "{reason}");
                }
                other => panic!("expected a refusal, got {other:?}"),
            }
        }

        // Reading the assistant's own Graph stays available, and another Graph
        // is not refused by this guard.
        assert!(
            tools
                .execute("graph_read", json!({"graph":"fixture"}))
                .await
                .is_ok()
        );
        let other = tools
            .execute(
                "graph_update",
                json!({"graph":"other","definition":definition}),
            )
            .await
            .unwrap_err();
        match other {
            ToolError::Failed(reason) => assert!(!reason.contains("defines this assistant")),
            other => panic!("expected a Turn-state failure, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn read_tools_do_not_create_runs_or_modify_definitions() {
        let (root, tools) = fixture();
        let original = std::fs::read(root.path().join("fixture/graph.json")).unwrap();
        assert_eq!(tools.definitions().len(), 20);
        assert_eq!(
            tools
                .definitions()
                .iter()
                .filter(|tool| tools.is_read_only(&tool.name))
                .count(),
            13
        );
        assert!(!tools.is_read_only("graph_run"));
        assert_eq!(
            tools
                .execute("graph_read", json!({"graph":"fixture"}))
                .await
                .unwrap()["definition"]["entry"],
            "idle"
        );
        assert_eq!(
            tools
                .execute(
                    "graph_validate",
                    json!({"definition":serde_json::from_slice::<Value>(&original).unwrap()})
                )
                .await
                .unwrap()["valid"],
            true
        );
        assert_eq!(
            tools.execute("plugin_list", json!({})).await.unwrap(),
            json!({"plugins":[]})
        );
        assert_eq!(
            tools.execute("run_list", json!({})).await.unwrap(),
            json!({"runs":[]})
        );
        assert_eq!(
            tools.execute("session_wait", json!({})).await.unwrap()["session"]["id"],
            "pilot"
        );
        assert!(
            tools
                .execute("graph_run", json!({"graph":"fixture"}))
                .await
                .is_err()
        );
        assert_eq!(
            std::fs::read(root.path().join("fixture/graph.json")).unwrap(),
            original
        );
        assert!(!root.path().join("state").exists());
    }

    #[tokio::test]
    async fn graph_mutations_require_an_owned_running_turn_and_do_not_start_runs() {
        let (root, tools) = fixture();
        let definition = json!({"entry":"idle","ops":{"idle":{"run":"true"}},
            "nodes":[{"id":"idle","op":"idle"}],"edges":[]});
        assert!(
            tools
                .execute(
                    "graph_create",
                    json!({"name":"new","definition":definition})
                )
                .await
                .is_err()
        );
        let (turn, _) = tools
            .sessions
            .create_turn("local", "pilot", "write-fixture", Some("create a Graph"))
            .unwrap();
        let mut tools = tools.for_turn(&turn.id);
        tools.owner = "responses-other".into();
        assert!(
            tools
                .execute(
                    "graph_create",
                    json!({"name":"new","definition":definition})
                )
                .await
                .is_err()
        );
        assert!(!root.path().join("new").exists());
        tools.owner = "local".into();
        let created = tools
            .execute(
                "graph_create",
                json!({"name":"new","definition":definition}),
            )
            .await
            .unwrap();
        assert_eq!(created["graph"], "new");
        assert!(tools.application.records().unwrap().is_empty());
        let mut updated = definition;
        updated["objective"] = json!("updated only, not executed");
        tools
            .execute("graph_update", json!({"graph":"new","definition":updated}))
            .await
            .unwrap();
        assert_eq!(
            tools
                .execute("graph_read", json!({"graph":"new"}))
                .await
                .unwrap()["definition"],
            updated
        );
        assert!(tools.application.records().unwrap().is_empty());
        assert!(
            tools
                .execute("graph_delete", json!({"graph":"new"}))
                .await
                .is_err()
        );
        tools
            .sessions
            .finish_turn(
                "local",
                "pilot",
                &turn.id,
                anchor_platform_session::TurnStatus::Completed,
                None,
            )
            .unwrap();
        assert!(
            tools
                .execute("graph_update", json!({"graph":"new","definition":updated}))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn graph_paths_symlinks_and_plugin_credentials_fail_closed() {
        let (root, tools) = fixture();
        assert!(
            tools
                .execute("graph_read", json!({"graph":"../secret"}))
                .await
                .is_err()
        );
        std::fs::write(root.path().join("secret.json"), "host-secret").unwrap();
        std::fs::remove_file(root.path().join("fixture/graph.json")).unwrap();
        std::os::unix::fs::symlink(
            root.path().join("secret.json"),
            root.path().join("fixture/graph.json"),
        )
        .unwrap();
        assert!(
            tools
                .execute("graph_read", json!({"graph":"fixture"}))
                .await
                .is_err()
        );
        let plugin = root.path().join("plugins/demo");
        std::fs::create_dir_all(&plugin).unwrap();
        std::fs::write(plugin.join("plugin.json"), r#"{"name":"Demo","mcpServers":{"remote":{"url":"https://private.invalid/?key=do-not-expose","headers":{"Authorization":"private-key"}}}}"#).unwrap();
        let public = tools
            .execute("plugin_list", json!({}))
            .await
            .unwrap()
            .to_string();
        assert!(
            !public.contains("private-key")
                && !public.contains("private.invalid")
                && !public.contains("do-not-expose"),
            "{public}"
        );
    }
    #[tokio::test]
    async fn a_requirement_for_a_name_the_assistants_called_subgraph_uses_is_refused() {
        let (root, mut tools) = fixture();
        // The own-dependency guard loads bundles, which needs the run store.
        std::fs::create_dir_all(root.path().join("state/runs")).unwrap();
        // Recording a need is a host fact a turn owns, so the turn must run.
        let (turn, _) = tools
            .sessions
            .create_turn("local", "pilot", "fixture-request", Some("fixture"))
            .unwrap();
        tools.turn = turn.id;
        // The assistant's own Graph calls `sub`, and `sub` declares the name.
        std::fs::create_dir_all(root.path().join("sub")).unwrap();
        std::fs::write(
            root.path().join("sub/graph.json"),
            json!({
                "objective": "sub",
                "entry": "inside",
                "agents": {"worker": {"model": "test"}},
                "ops": {},
                "nodes": [{"id": "inside", "agent": "worker", "environment": "research"}],
                "edges": []
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            root.path().join("sub/manifest.json"),
            json!({"format": 1, "graph": "graph.json", "plugins": []}).to_string(),
        )
        .unwrap();
        std::fs::write(
            root.path().join("fixture/graph.json"),
            json!({
                "objective": "assistant",
                "entry": "work",
                "agents": {"worker": {"model": "test"}},
                "ops": {"work": {"call": {"graph": "sub", "mode": "wait"}}},
                "nodes": [{"id": "work", "op": "work"}],
                "edges": []
            })
            .to_string(),
        )
        .unwrap();

        // Asking for the name its called Graph uses would let the assistant choose
        // its own dependency content, so it is refused.
        let refused = tools
            .execute(
                "environment_request",
                json!({"name":"research","purpose":"would change itself"}),
            )
            .await;
        assert!(refused.is_err(), "{refused:?}");
        // A name nobody in its execution closure uses is still allowed.
        let allowed = tools
            .execute(
                "environment_request",
                json!({"name":"unrelated","purpose":"a real need"}),
            )
            .await;
        assert!(allowed.is_ok(), "{allowed:?}");
    }

    #[tokio::test]
    async fn the_assistant_records_an_environment_requirement_without_publishing() {
        let (root, mut tools) = fixture();
        // The own-dependency guard loads bundles, which needs the run store.
        std::fs::create_dir_all(root.path().join("state/runs")).unwrap();
        // Recording a need is a host fact a turn owns, so the turn must run.
        let (turn, _) = tools
            .sessions
            .create_turn("local", "pilot", "fixture-request", Some("fixture"))
            .unwrap();
        tools.turn = turn.id;
        let requested = tools
            .execute(
                "environment_request",
                json!({
                    "name":"research",
                    "purpose":"parse the weekly reports",
                    "interpreter":"python>=3.11",
                    "packages":["pandas"]
                }),
            )
            .await
            .unwrap();
        assert_eq!(requested["requested"], true);
        assert_eq!(requested["name"], "research");
        let recorded = crate::application::environments::list_environment_requirements(
            &root.path().join("state"),
        )
        .unwrap();
        assert_eq!(recorded.unreadable, 0);
        assert_eq!(recorded.requirements.len(), 1);
        assert_eq!(recorded.requirements[0].name, "research");
        assert_eq!(recorded.requirements[0].requests.len(), 1);
        assert_eq!(
            recorded.requirements[0].requests[0].packages,
            vec!["pandas"]
        );
        assert_eq!(recorded.requirements[0].format, 1);
        assert!(recorded.requirements[0].observed.is_none());

        // A model-supplied approval field, or a malformed ask, is refused.
        for refused in [
            json!({"name":"research","purpose":"x","confirm":true}),
            json!({"name":"research"}),
            json!({"name":"re search","purpose":"x"}),
            json!({"name":"research","purpose":""}),
            json!({"name":"research","purpose":"x","packages":"pandas"}),
            json!({"name":"research","purpose":"x","packages":["bad package!"]}),
            json!({"name":"research","purpose":"x","interpreter":""}),
        ] {
            assert!(
                tools
                    .execute("environment_request", refused.clone())
                    .await
                    .is_err(),
                "{refused} must be refused"
            );
        }
        // Nothing was published: the requirement is a request, not an asset.
        let store = anchor_library::EnvironmentStore::new(root.path());
        assert!(store.names().unwrap().is_empty());
        assert!(
            store.list().unwrap().is_empty(),
            "recording a need must not publish any content"
        );
    }

    #[tokio::test]
    async fn the_assistant_sees_environments_without_changing_them() {
        let (root, tools) = fixture();
        let prepared = root.path().join("prepared");
        std::fs::create_dir_all(prepared.join("bin")).unwrap();
        std::fs::write(prepared.join("marker.txt"), "one").unwrap();
        let store = anchor_library::EnvironmentStore::new(root.path());
        let published = store.publish(&prepared).unwrap();
        store.set_name("research", &published.id, None).unwrap();

        let listed = tools.execute("environment_list", json!({})).await.unwrap();
        assert_eq!(listed["names"][0]["name"], "research");
        assert_eq!(listed["names"][0]["id"], published.id);
        assert_eq!(listed["names"][0]["version"], 1);
        assert_eq!(listed["environments"][0]["id"], published.id);

        // By name and by id describe the same published content, and the digest is
        // re-verified rather than assumed.
        let by_name = tools
            .execute("environment_show", json!({"name":"research"}))
            .await
            .unwrap();
        assert_eq!(by_name["id"], published.id);
        assert_eq!(by_name["verified"], true);
        let by_id = tools
            .execute("environment_show", json!({"id":published.id}))
            .await
            .unwrap();
        assert_eq!(by_id["id"], published.id);

        // Unknown or ambiguous requests are refused, and neither tool writes.
        let missing = tools
            .execute("environment_show", json!({"name":"missing"}))
            .await
            .unwrap_err();
        assert!(
            missing.to_string().contains("no dependency environment"),
            "{missing}"
        );
        let both = tools
            .execute(
                "environment_show",
                json!({"id":published.id,"name":"research"}),
            )
            .await
            .unwrap_err();
        assert!(both.to_string().contains("exactly one"), "{both}");
        assert!(!tools.is_read_only("plugin_install"));
        assert!(tools.is_read_only("environment_list"));
        assert!(tools.is_read_only("environment_show"));
    }

    #[test]
    fn the_tool_table_is_the_only_source_of_authority() {
        let names = PILOT_TOOLS.iter().map(|tool| tool.name).collect::<Vec<_>>();
        let unique = names.iter().collect::<std::collections::BTreeSet<_>>();
        assert_eq!(names.len(), unique.len(), "tool names must be unique");

        let inspect = PILOT_TOOLS
            .iter()
            .filter(|tool| tool.authority == ToolAuthority::Inspect)
            .count();
        let mutate = PILOT_TOOLS
            .iter()
            .filter(|tool| tool.authority == ToolAuthority::Mutate)
            .count();
        let confirm = PILOT_TOOLS
            .iter()
            .filter(|tool| tool.authority == ToolAuthority::Confirm)
            .map(|tool| tool.name.to_owned())
            .collect::<std::collections::BTreeSet<_>>();
        // The confirmed tools are exactly the ones the interaction layer owns, so
        // a new confirmed tool cannot be added to one list and forgotten in the
        // other — which is what made "approval is never a tool argument" a
        // property of a list rather than of the design.
        let interactive = crate::goose_acp::pilot_interaction::definitions();
        let interactive = interactive
            .into_iter()
            .map(|definition| definition.name.to_string())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(confirm, interactive);

        // The advertised schema of a confirmed tool must be the one the
        // interaction layer shows, or the model is told something else than the
        // human is asked.
        for tool in PILOT_TOOLS
            .iter()
            .filter(|tool| tool.authority == ToolAuthority::Confirm)
        {
            let advertised = crate::goose_acp::pilot_interaction::definitions()
                .into_iter()
                .find(|definition| definition.name == tool.name)
                .unwrap_or_else(|| panic!("{} is not advertised", tool.name));
            assert_eq!(
                advertised.parameters,
                (tool.parameters)(),
                "{} schema drifted",
                tool.name
            );
        }

        let (root, tools) = fixture();
        let _ = root;
        assert_eq!(tools.definitions().len(), inspect + mutate);
        assert_eq!(
            tools
                .definitions()
                .iter()
                .filter(|tool| tools.is_read_only(&tool.name))
                .count(),
            inspect
        );
        assert_eq!((inspect, mutate, confirm.len()), (13, 7, 5));
        // The two axes must agree: everything readable is Inspect, and every
        // Inspect tool is read-only. "Only assets need a human click" is then a
        // property of the table, not a sentence in a document.
        for tool in PILOT_TOOLS {
            assert_eq!(
                tool.authority == ToolAuthority::Inspect,
                tool.effect == ToolEffect::Read,
                "{} authority and effect disagree",
                tool.name
            );
        }
        let host_facts = PILOT_TOOLS
            .iter()
            .filter(|tool| tool.effect == ToolEffect::HostFact)
            .map(|tool| tool.name)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            host_facts,
            ["ask_user", "environment_request"]
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>(),
            "a host fact changes no asset and needs no approval"
        );
    }
}
