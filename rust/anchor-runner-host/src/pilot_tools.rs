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
mod graphs;
mod plugins;
mod runs;
mod sessions;

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
        description: "Record what you need a dependency environment to provide (interpreter version prefix like `3.12`, package names, why) under a deployment name, so the operator can provide it. It publishes nothing and installs nothing.",
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
            "graph_create" => self.graph_create(&arguments).await,
            "graph_update" => self.graph_update(&arguments).await,
            "graph_run" => self.graph_run(&arguments).await,
            "run_pause" | "run_resume" | "run_stop" => self.run_control(name, &arguments).await,
            "graph_list" => self.graph_list(&arguments).await,
            "graph_read" => self.graph_read(&arguments).await,
            "graph_validate" => self.graph_validate(&arguments).await,
            "plugin_list" => self.plugin_list(&arguments).await,
            "plugin_read" => self.plugin_read(&arguments).await,
            "plugin_review" => self.plugin_review(&arguments).await,
            "environment_list" => self.environment_list(&arguments).await,
            "environment_request" => self.environment_request(&arguments).await,
            "environment_references" => self.environment_references(&arguments).await,
            "environment_show" => self.environment_show(&arguments).await,
            "run_list" => self.run_list(&arguments).await,
            "run_status" => self.run_status(&arguments).await,
            "artifact_read" => self.artifact_read(&arguments).await,
            "session_wait" => self.session_wait(&arguments).await,
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
