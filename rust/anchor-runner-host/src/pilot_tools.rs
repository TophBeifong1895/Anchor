use std::{
    future::Future,
    io::Read,
    path::{Path, PathBuf},
    pin::Pin,
};

use anchor_graph_host::{FilePluginCatalog, PluginCatalog, PluginDefinition};
use anchor_platform_session::SessionStore;
use anchor_runtime::{
    ToolDefinition, ToolError, ToolPort, ToolResultContent,
    graph::{GraphSnapshot, PluginBinding},
};
use anchor_sandbox_bwrap::{BubblewrapPolicy, BubblewrapSandbox};
use serde_json::{Value, json};
use sha2::Digest as _;

mod graphs;
mod plugins;
mod runs;
mod sessions;

use crate::{
    application::{AdmissionOptions, RunApplication, RunTrigger, metadata::PilotRunSource},
    artifacts::HostArtifacts,
    resource_read::open_resource,
    tool_host,
};

/// The Plugin ids this deployment enabled as its in-process fake fixture.
///
/// The same configuration the node host reads: a fixture binding is enabled by
/// the deployment, not by a Library directory.
fn fake_plugin_ids() -> Vec<String> {
    std::env::var("ANCHOR_RUNNER_FAKE_PLUGINS")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Whether this binding's own declaration launches a process.
///
/// The same rule `tool_host::transport_name` applies: a `command` is a stdio
/// server unless the declaration says otherwise, and no `command` is HTTP.
fn declares_stdio(catalog: &FilePluginCatalog, binding: &PluginBinding) -> bool {
    if crate::node_host::is_fake_fixture(binding) {
        return false;
    }
    catalog
        .mcp_servers(&binding.id, false)
        .is_ok_and(|servers| {
            servers.iter().any(|server| {
                let config = &server.config;
                let transport = if config.get("command").is_some() {
                    config
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("stdio")
                } else {
                    config.get("type").and_then(Value::as_str).unwrap_or("http")
                };
                transport == "stdio"
            })
        })
}

/// The node sandbox a Pilot Turn's stdio Plugin servers launch in, and the
/// session-level workspace they share.
///
/// The authority is the deployment's, read from the same configuration a node
/// call reads: `ANCHOR_BWRAP`, `ANCHOR_RUNNER_ALLOWED_COMMANDS`, the deployment
/// tool environment, and `ANCHOR_RUNNER_WORKSPACE_ROOT` as the authorized
/// workspace root. No new permission is invented here — a Plugin server gets
/// exactly the authority a node call's server gets, and only because a human
/// mounted that Plugin in this Session. A deployment that authorized no commands
/// gets no stdio Plugin tools, with a reason.
///
/// A Pilot Turn has no Run, so there is no node workspace to reuse. The Plugin
/// servers get one session-level workspace instead, stable per (owner, session)
/// under the deployment's workspace root, so a Plugin's own state survives the
/// Turn that created it.
fn pilot_plugin_sandbox(
    workspace_root: &Path,
    owner: &str,
    session: &str,
) -> Result<(BubblewrapSandbox, PathBuf), String> {
    let commands = std::env::var("ANCHOR_RUNNER_ALLOWED_COMMANDS").map_err(|_| {
        "Plugin stdio servers require the deployment's ANCHOR_RUNNER_ALLOWED_COMMANDS allow-list"
            .to_owned()
    })?;
    let bwrap = std::env::var_os("ANCHOR_BWRAP")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("bwrap"));
    let identity = serde_json::to_vec(&(owner, session)).map_err(|error| error.to_string())?;
    let workspace = workspace_root
        .join("pilot")
        .join(format!("{:x}", sha2::Sha256::digest(&identity)));
    std::fs::create_dir_all(&workspace)
        .map_err(|error| format!("Pilot Plugin workspace is unavailable: {error}"))?;
    let policy = BubblewrapPolicy::new(
        bwrap,
        commands
            .split(',')
            .filter(|command| !command.is_empty())
            .map(str::to_owned),
    )
    .authorize_workspace_root(workspace_root)
    .authorize_readonly_destination_root("/plugins")
    .allow_network();
    let environment = crate::tool_environment::ToolEnvironment::from_env()?;
    let sandbox = BubblewrapSandbox::new(environment.authorize(policy))
        .map_err(|error| format!("Pilot Plugin sandbox is unavailable: {error}"))?;
    Ok((sandbox, workspace))
}

/// The Plugin tool ports one Pilot Turn froze, and why a mounted Plugin is absent.
///
/// The Turn's Session switches are frozen here, once, against the new Turn id: a
/// click made *during* this Turn cannot change what this Turn may use, and the
/// next Turn sees it. `required`/`default` do not apply — the Pilot's own Turn
/// has no Graph snapshot of its own, so a Plugin reaches it only because a human
/// mounted it in the Session.
///
/// A Plugin that is not installed, whose view is not usable, or whose MCP server
/// cannot bind is absent and named in `reasons`. Nothing here installs a Plugin,
/// builds or repairs a view, or fetches a package.
pub(crate) async fn frozen_plugin_tools(
    data_root: &Path,
    library_root: &Path,
    workspace_root: &Path,
    owner: &str,
    session: &str,
    turn: &str,
) -> Result<(Vec<std::sync::Arc<dyn ToolPort>>, Vec<String>), String> {
    let source = crate::plugin_mounts::SessionMounts::open(data_root)?;
    let frozen = crate::node_host::freeze_session_switches(data_root, session, turn, &source)?;
    let fixture = tool_host::PluginToolHost::new(fake_plugin_ids());
    let (bindings, mut reasons) =
        crate::node_host::session_plugin_mounts(library_root, &fixture, &frozen.switches)?;
    if frozen.snapshot_unreadable {
        reasons.push(
            "no Plugin mount snapshot could be read; the Session's current switches were used"
                .to_owned(),
        );
    }
    let catalog = FilePluginCatalog::new(library_root);
    let environment = crate::tool_environment::ToolEnvironment::from_env()?;
    // Only a stdio server needs the deployment sandbox: an HTTP server connects
    // from the host, and the in-process fixture needs no process at all.
    let mut stdio = None;
    if bindings
        .iter()
        .any(|binding| declares_stdio(&catalog, binding))
    {
        match pilot_plugin_sandbox(workspace_root, owner, session) {
            Ok(pair) => stdio = Some(pair),
            Err(reason) => reasons.push(reason),
        }
    }
    let mut ports: Vec<std::sync::Arc<dyn ToolPort>> = Vec::new();
    for binding in &bindings {
        let single = std::slice::from_ref(binding);
        let mut mcp = match tool_host::McpToolConfig::from_catalog(library_root, single) {
            Ok(mcp) => mcp,
            Err(reason) => {
                reasons.push(reason);
                continue;
            }
        };
        mcp.environment = environment.clone();
        if let Err(reason) = mcp.authorize_oauth(Some(owner.to_owned()), true).await {
            reasons.push(reason);
            continue;
        }
        let unavailable = |reason: String| {
            format!(
                "Plugin `{}` MCP tools are unavailable: {reason}",
                binding.id
            )
        };
        let assembled = if crate::node_host::is_fake_fixture(binding) {
            fixture.assemble(&mcp, single, true).await
        } else if declares_stdio(&catalog, binding) {
            let Some((sandbox, workspace)) = &stdio else {
                // The sandbox reason is already recorded; a stdio Plugin never
                // runs outside the deployment's sandbox.
                continue;
            };
            let mount = match tool_host::McpToolConfig::plugin_mounts(library_root, single) {
                Ok(mount) => mount,
                Err(reason) => {
                    reasons.push(reason);
                    continue;
                }
            };
            let views = match crate::plugin_view::prepare_views_async(
                single.to_vec(),
                crate::deployment_library_root(),
            )
            .await
            {
                Ok(views) => views,
                Err(error) => {
                    reasons.push(error.to_string());
                    continue;
                }
            };
            fixture
                .assemble_with_sandbox(
                    &mcp,
                    single,
                    true,
                    tool_host::SandboxLaunch {
                        sandbox,
                        workspace,
                        readonly_inputs: &mount,
                        plugin_views: &views,
                    },
                )
                .await
        } else {
            fixture.assemble(&mcp, single, true).await
        };
        match assembled {
            Ok(port) => ports.push(port),
            Err(reason) => reasons.push(unavailable(reason)),
        }
    }
    Ok((ports, reasons))
}

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
    /// Changes a saved asset: a Graph or a Plugin.
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

/// The Session-mount freezing the Pilot's own Turn goes through.
///
/// These exercise the entry point `PilotService::admit` calls, then the same two
/// functions `PilotPort` uses to build its tool list and route a call, so a
/// Plugin that appears here appears in the Pilot's tools.
#[cfg(test)]
mod frozen_mounts_tests {
    use super::*;
    use crate::goose_acp::pilot::{plugin_definitions, plugin_port};
    use anchor_platform_session::CreateSession;
    use serde_json::json;

    /// Set one environment variable for a test, and restore it on drop.
    struct EnvVar(&'static str, Option<std::ffi::OsString>);

    impl EnvVar {
        fn set(name: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let previous = std::env::var_os(name);
            unsafe { std::env::set_var(name, value) };
            Self(name, previous)
        }
    }

    impl Drop for EnvVar {
        fn drop(&mut self) {
            unsafe {
                match &self.1 {
                    Some(value) => std::env::set_var(self.0, value),
                    None => std::env::remove_var(self.0),
                }
            }
        }
    }

    struct Fixture {
        _root: tempfile::TempDir,
        root: PathBuf,
        state: PathBuf,
        /// The deployment's workspace root: what a node call authorizes, and what
        /// a Pilot Turn's Plugin sandbox is built from.
        workspace_root: PathBuf,
        sessions: SessionStore,
    }

    impl Fixture {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let state = root.path().join("state");
            std::fs::create_dir_all(state.join("platform")).unwrap();
            let sessions = SessionStore::open(state.join("platform/sessions.sqlite")).unwrap();
            sessions
                .create(
                    "local",
                    CreateSession {
                        id: Some("sess".into()),
                        ..Default::default()
                    },
                )
                .unwrap();
            let path = root.path().to_path_buf();
            let workspace_root = path.join("workspaces");
            std::fs::create_dir_all(&workspace_root).unwrap();
            Self {
                _root: root,
                root: path,
                state,
                workspace_root,
                sessions,
            }
        }

        fn switch(&self, plugin: &str, on: bool) {
            self.sessions
                .set_plugin_mount("local", "sess", plugin, on, "operator", "the Session view")
                .unwrap();
        }

        /// The built-in Pilot tool list, exactly as `PilotTools` builds it.
        fn base_definitions(&self) -> Vec<ToolDefinition> {
            PilotTools {
                application: RunApplication::new(self.state.clone(), self.root.clone()),
                graph_name: "pilot".into(),
                catalog_root: self.root.clone(),
                library_root: self.root.clone(),
                data_root: self.state.clone(),
                workspace_root: self.root.join("workspaces"),
                sessions: self.sessions.clone(),
                owner: "local".into(),
                session: "sess".into(),
                turn: String::new(),
            }
            .definitions()
        }

        async fn freeze(&self, turn: &str) -> (Vec<std::sync::Arc<dyn ToolPort>>, Vec<String>) {
            frozen_plugin_tools(
                &self.state,
                &self.root,
                &self.workspace_root,
                "local",
                "sess",
                turn,
            )
            .await
            .unwrap()
        }
    }

    fn names(definitions: &[ToolDefinition]) -> Vec<String> {
        definitions
            .iter()
            .map(|definition| definition.name.to_string())
            .collect()
    }

    #[tokio::test]
    async fn a_session_mounted_plugin_reaches_the_pilots_tool_list() {
        let _env = crate::PROCESS_ENV_LOCK.lock().await;
        let _fake = EnvVar::set("ANCHOR_RUNNER_FAKE_PLUGINS", "fake-tools");
        let fixture = Fixture::new();
        // Nothing is mounted yet: the Pilot's list is the built-in tools only.
        let (ports, reasons) = fixture.freeze("turn-1").await;
        assert!(reasons.is_empty(), "{reasons:?}");
        let before = names(&plugin_definitions(fixture.base_definitions(), &ports));
        assert!(before.iter().any(|name| name == "graph_run"));
        assert!(!before.iter().any(|name| name == tool_host::FAKE_ECHO_TOOL));

        // The human mounts an installed Plugin before the next Turn.
        fixture.switch("fake-tools", true);
        let (ports, reasons) = fixture.freeze("turn-2").await;
        assert!(reasons.is_empty(), "{reasons:?}");
        let after = names(&plugin_definitions(fixture.base_definitions(), &ports));
        assert!(
            after.iter().any(|name| name == tool_host::FAKE_ECHO_TOOL),
            "{after:?}"
        );
        // The built-in control tools are not a switchable capability.
        assert!(after.iter().any(|name| name == "graph_run"));
        assert!(!after.iter().any(|name| name == "plugin_install"));
        // And the mounted Plugin's tool really is callable through the frozen port.
        let port = plugin_port(&ports, tool_host::FAKE_ECHO_TOOL).unwrap();
        let result = port
            .call(tool_host::FAKE_ECHO_TOOL, json!({"value": {"answer": 42}}))
            .await
            .unwrap();
        assert_eq!(result[0].as_json(), Some(&json!({"answer": 42})));
    }

    #[tokio::test]
    async fn a_click_mid_turn_does_not_change_that_turns_pilot_tools_or_a_frozen_retry() {
        let _env = crate::PROCESS_ENV_LOCK.lock().await;
        let _fake = EnvVar::set("ANCHOR_RUNNER_FAKE_PLUGINS", "fake-tools");
        let fixture = Fixture::new();
        fixture.switch("fake-tools", true);
        let (running, _) = fixture.freeze("turn-1").await;
        // The human switches it off while turn-1 is still running.
        fixture.switch("fake-tools", false);
        // The Turn that already started keeps what it froze.
        let running = names(&plugin_definitions(Vec::new(), &running));
        assert!(
            running.iter().any(|name| name == tool_host::FAKE_ECHO_TOOL),
            "{running:?}"
        );
        // A retry of the *same* Turn reads that Turn's snapshot, not the Session.
        let (retry, _) = fixture.freeze("turn-1").await;
        let retry = names(&plugin_definitions(Vec::new(), &retry));
        assert!(
            retry.iter().any(|name| name == tool_host::FAKE_ECHO_TOOL),
            "{retry:?}"
        );
        // The next Turn sees the click.
        let (next, _) = fixture.freeze("turn-2").await;
        let next = names(&plugin_definitions(Vec::new(), &next));
        assert!(
            !next.iter().any(|name| name == tool_host::FAKE_ECHO_TOOL),
            "{next:?}"
        );
    }

    #[tokio::test]
    async fn an_absent_or_unusable_plugin_is_absent_with_a_reason() {
        let _env = crate::PROCESS_ENV_LOCK.lock().await;
        let fixture = Fixture::new();
        // Installed, but its own declaration's view is not there.
        let plugin = fixture.root.join("plugins/needsview");
        std::fs::create_dir_all(&plugin).unwrap();
        std::fs::write(
            plugin.join("plugin.json"),
            json!({"name":"Needs view","dependencies":{"python":["idna==3.7"]}}).to_string(),
        )
        .unwrap();
        fixture.switch("ghost", true);
        fixture.switch("needsview", true);

        let (ports, reasons) = fixture.freeze("turn-1").await;
        assert!(ports.is_empty());
        assert!(
            reasons.iter().any(|reason| reason.contains("`ghost`")
                && reason.contains("not available in this Library")),
            "{reasons:?}"
        );
        assert!(
            reasons
                .iter()
                .any(|reason| reason.contains("`needsview`") && reason.contains("view")),
            "{reasons:?}"
        );
    }
    /// A loopback MCP server, so the decisive test can mount a Plugin that is
    /// really installed in the Library rather than the in-process fixture.
    #[derive(Clone)]
    struct HttpFixtureServer;

    impl rmcp::ServerHandler for HttpFixtureServer {
        fn get_info(&self) -> rmcp::model::ServerInfo {
            rmcp::model::ServerInfo::new(
                rmcp::model::ServerCapabilities::builder()
                    .enable_tools()
                    .build(),
            )
            .with_server_info(rmcp::model::Implementation::new("pilot-mount-fixture", "1"))
        }

        async fn list_tools(
            &self,
            _: Option<rmcp::model::PaginatedRequestParams>,
            _: rmcp::service::RequestContext<rmcp::RoleServer>,
        ) -> Result<rmcp::model::ListToolsResult, rmcp::model::ErrorData> {
            let schema: std::sync::Arc<serde_json::Map<String, Value>> = std::sync::Arc::new(
                serde_json::from_value(json!({
                    "type":"object","properties":{"value":{"type":"string"}},
                    "required":["value"],"additionalProperties":false
                }))
                .unwrap(),
            );
            Ok(rmcp::model::ListToolsResult {
                tools: vec![rmcp::model::Tool::new("echo", "Echo the value.", schema)],
                next_cursor: None,
                meta: None,
            })
        }

        async fn call_tool(
            &self,
            request: rmcp::model::CallToolRequestParams,
            _: rmcp::service::RequestContext<rmcp::RoleServer>,
        ) -> Result<rmcp::model::CallToolResult, rmcp::model::ErrorData> {
            let value = request.arguments.unwrap().get("value").cloned().unwrap();
            let mut result =
                rmcp::model::CallToolResult::success(vec![rmcp::model::ContentBlock::text(
                    value.as_str().unwrap_or_default(),
                )]);
            result.structured_content = Some(value);
            Ok(result)
        }
    }

    struct HttpFixture {
        endpoint: String,
        cancellation: tokio_util::sync::CancellationToken,
        task: tokio::task::JoinHandle<()>,
    }

    impl Drop for HttpFixture {
        fn drop(&mut self) {
            self.cancellation.cancel();
            self.task.abort();
        }
    }

    async fn http_mcp_fixture() -> HttpFixture {
        let cancellation = tokio_util::sync::CancellationToken::new();
        let service: rmcp::transport::StreamableHttpService<
            HttpFixtureServer,
            rmcp::transport::streamable_http_server::session::local::LocalSessionManager,
        > = rmcp::transport::StreamableHttpService::new(
            || Ok(HttpFixtureServer),
            Default::default(),
            rmcp::transport::StreamableHttpServerConfig::default()
                .with_sse_keep_alive(None)
                .with_cancellation_token(cancellation.child_token()),
        );
        let app = axum::Router::new().nest_service("/mcp", service);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        HttpFixture {
            endpoint,
            cancellation,
            task,
        }
    }

    #[tokio::test]
    async fn an_installed_plugin_with_an_http_server_reaches_the_pilots_tool_list() {
        let _env = crate::PROCESS_ENV_LOCK.lock().await;
        let fixture = Fixture::new();
        let server = http_mcp_fixture().await;
        let plugin = fixture.root.join("plugins/demo");
        std::fs::create_dir_all(&plugin).unwrap();
        std::fs::write(
            plugin.join("plugin.json"),
            json!({"name":"Demo","mcpServers":{"remote":{"type":"http","url":server.endpoint}}})
                .to_string(),
        )
        .unwrap();
        fixture.switch("demo", true);

        let (ports, reasons) = fixture.freeze("turn-1").await;
        assert!(reasons.is_empty(), "{reasons:?}");
        let names = names(&plugin_definitions(fixture.base_definitions(), &ports));
        assert!(
            names.iter().any(|name| name == "demo-remote_echo"),
            "{names:?}"
        );
        let port = plugin_port(&ports, "demo-remote_echo").unwrap();
        let result = port
            .call("demo-remote_echo", json!({"value":"mounted"}))
            .await
            .unwrap();
        assert_eq!(result[0].as_json(), Some(&json!("mounted")));
    }
    /// The repository's stdio MCP fixture binary (the same one the node-host
    /// tests use), built once per test process.
    fn stdio_fixture_binary() -> PathBuf {
        static BINARY: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
        BINARY
            .get_or_init(|| {
                let executable = std::env::current_exe().unwrap();
                let profile = executable.parent().unwrap().parent().unwrap();
                let target = profile.parent().unwrap();
                let mut build = std::process::Command::new("cargo");
                build
                    .args([
                        "build",
                        "--offline",
                        "-p",
                        "anchor-mcp-host",
                        "--example",
                        "http_fixture",
                    ])
                    .arg("--manifest-path")
                    .arg(
                        Path::new(env!("CARGO_MANIFEST_DIR"))
                            .parent()
                            .unwrap()
                            .join("Cargo.toml"),
                    )
                    .env("CARGO_TARGET_DIR", target);
                if profile.file_name().is_some_and(|name| name == "release") {
                    build.arg("--release");
                }
                let output = build.output().unwrap();
                assert!(
                    output.status.success(),
                    "the stdio fixture did not build: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                profile.join("examples/http_fixture")
            })
            .clone()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_installed_stdio_plugin_reaches_the_pilots_tool_list_and_runs() {
        use std::os::unix::fs::PermissionsExt;

        let _env = crate::PROCESS_ENV_LOCK.lock().await;
        // The deployment's own node-call authority: the command allow-list and the
        // workspace root a Plugin's stdio server is launched under.
        let _commands = EnvVar::set("ANCHOR_RUNNER_ALLOWED_COMMANDS", "sh");
        let fixture = Fixture::new();
        let _workspace = EnvVar::set("ANCHOR_RUNNER_WORKSPACE_ROOT", &fixture.workspace_root);

        let plugin = fixture.root.join("plugins/probe");
        std::fs::create_dir_all(&plugin).unwrap();
        std::fs::copy(stdio_fixture_binary(), plugin.join("mcp-fixture")).unwrap();
        std::fs::write(
            plugin.join("server.sh"),
            "#!/usr/bin/bash\nexec ./mcp-fixture --stdio \"$@\"\n",
        )
        .unwrap();
        for file in ["server.sh", "mcp-fixture"] {
            std::fs::set_permissions(plugin.join(file), std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }
        std::fs::write(
            plugin.join("plugin.json"),
            json!({
                "name":"Stdio probe",
                "mcpServers":{"probe":{"command":"server.sh","args":["echo","--prefix","mounted:"]}}
            })
            .to_string(),
        )
        .unwrap();
        fixture.switch("probe", true);

        let (ports, reasons) = fixture.freeze("turn-1").await;
        assert!(reasons.is_empty(), "{reasons:?}");
        let names = names(&plugin_definitions(fixture.base_definitions(), &ports));
        assert!(
            names.iter().any(|name| name == "probe-probe_echo"),
            "{names:?}"
        );
        // The tool is not just advertised: the sandboxed server really answers.
        let port = plugin_port(&ports, "probe-probe_echo").unwrap();
        let result = port
            .call("probe-probe_echo", json!({"value":"ok"}))
            .await
            .unwrap();
        assert_eq!(result[0].as_text(), Some("mounted:ok"));
        // The server ran inside its session workspace, not somewhere else.
        let log = std::fs::read_to_string(
            fixture
                .workspace_root
                .join("pilot")
                .join(format!(
                    "{:x}",
                    sha2::Sha256::digest(serde_json::to_vec(&("local", "sess")).unwrap())
                ))
                .join("mcp-log"),
        )
        .unwrap();
        assert!(log.contains("tools/call"), "{log:?}");
    }
}
