use super::*;
use serde::{Deserialize, Serialize};

/// Coordinator-owned provenance; completion output never grants file access.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Node,
    Fanout,
    Join,
    GraphCall,
    Interruption,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactFreezeContext {
    pub kind: ArtifactKind,
    pub input_commits: Vec<CommitRef>,
}

pub trait ArtifactPort: Send + Sync {
    /// Must be idempotent for an InvocationKey. A retry after freeze succeeded
    /// but before the RunStore commit must resolve to the same immutable commit.
    fn freeze<'a>(
        &'a self,
        key: &'a InvocationKey,
        completion: &'a NodeCompletion,
    ) -> Pin<Box<dyn Future<Output = Result<CommitRef, GraphError>> + Send + 'a>>;
    /// Preserve exact input provenance when the host supports linked files.
    /// Default adapters reject interruptions rather than freeze the node workspace.
    fn freeze_with_context<'a>(
        &'a self,
        key: &'a InvocationKey,
        completion: &'a NodeCompletion,
        context: &'a ArtifactFreezeContext,
    ) -> Pin<Box<dyn Future<Output = Result<CommitRef, GraphError>> + Send + 'a>> {
        if context.kind == ArtifactKind::Interruption {
            return Box::pin(async {
                Err(GraphError::Unsupported(
                    "ArtifactPort must explicitly support interruption control evidence without freezing the node workspace".into(),
                ))
            });
        }
        self.freeze(key, completion)
    }
    /// Read/materialize the exact fixed commit. Implementations must not follow
    /// a mutable workspace head or create/advance a commit as a side effect.
    fn resolve<'a>(
        &'a self,
        commit: &'a CommitRef,
    ) -> Pin<Box<dyn Future<Output = Result<Value, GraphError>> + Send + 'a>>;
    /// Stage the parent-visible committed input files selected by an Op.call
    /// `files` list into the child Run's read-only `/in/call` bundle. Hosts that
    /// cannot expose immutable file trees must fail closed when a selection is
    /// requested; an empty selection is always a no-op. Implementations must be
    /// idempotent for a given child Run so a wait retry never rewrites inputs.
    fn stage_call_inputs<'a>(
        &'a self,
        _child_run_id: &'a str,
        _parents: &'a [CommitRef],
        selections: &'a [CallFileSelection],
    ) -> Pin<Box<dyn Future<Output = Result<(), GraphError>> + Send + 'a>> {
        Box::pin(async move {
            if selections.is_empty() {
                Ok(())
            } else {
                Err(GraphError::Unsupported(
                    "host ArtifactPort cannot stage Op.call input files".into(),
                ))
            }
        })
    }
    /// Copy selected files from a child result commit into the parent call node
    /// workspace `result/` directory, returning the copied relative paths. The
    /// call node's GraphCall commit is frozen after this future resolves, so a
    /// successful export is what makes the files readable downstream.
    fn export_call_result_files<'a>(
        &'a self,
        _call_key: &'a InvocationKey,
        _commit: &'a CommitRef,
        files: &'a [String],
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, GraphError>> + Send + 'a>> {
        Box::pin(async move {
            if files.is_empty() {
                Ok(Vec::new())
            } else {
                Err(GraphError::Unsupported(
                    "host ArtifactPort cannot export Op.call result files".into(),
                ))
            }
        })
    }
}

/// One already-validated `files` selection from an Op.call spec: the committed
/// source node, the source-relative path, and the child-relative `as` alias.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallFileSelection {
    pub node: String,
    pub path: String,
    pub alias: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphCallOutcome {
    Waiting {
        child_run_id: String,
    },
    Completed {
        child_run_id: String,
        output: Value,
    },
    Detached {
        child_run_id: String,
    },
    Failed {
        child_run_id: Option<String>,
        reason: String,
    },
    Uncertain {
        child_run_id: Option<String>,
        reason: String,
    },
}

/// Host-owned admission/execution bridge for Op.call. Implementations MUST
/// atomically persist identity → child Run admission, return that same child
/// for every retry, and report `Uncertain` when they cannot prove the durable
/// fact. `Detached` is legal only after admission is durable. Child execution
/// itself uses the host's ordinary GraphRunner instance.
pub trait GraphCallPort: Send + Sync {
    fn call<'a>(
        &'a self,
        identity: &'a CallIdentity,
        spec: &'a Value,
        input: &'a Value,
        input_commits: &'a [CommitRef],
        cancellation: crate::Cancellation,
    ) -> Pin<Box<dyn Future<Output = Result<GraphCallOutcome, GraphError>> + Send + 'a>>;
}

#[derive(Debug, Clone)]
pub struct NodeExecutionRequest {
    pub key: InvocationKey,
    /// Provider/model reference selected by the Graph for Agent nodes.
    pub model: Option<String>,
    pub task: String,
    pub instructions: String,
    pub routes: Vec<String>,
    pub input: Value,
    pub input_commits: Vec<CommitRef>,
    /// Secret-free Plugin resource identities resolved by the host.
    pub plugins: Vec<PluginBinding>,
    /// Dependency environments this invocation uses, keyed by site. The host
    /// freezes them at admission; an empty map keeps the legacy behaviour.
    pub environments: BTreeMap<String, EnvironmentBinding>,
    /// A cumulative provider-request budget, supported only when the node
    /// adapter advertises exact request accounting.
    pub max_provider_requests: Option<u64>,
    pub wall_time_limit_seconds: Option<f64>,
    /// Graph request intent only. The host adapter must apply its own network authorization.
    pub network: bool,
    pub kind: NodeKind,
    pub operation: Option<Value>,
    pub cancellation: crate::Cancellation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginBinding {
    pub id: String,
    pub digest: String,
    #[serde(default)]
    pub resources: Vec<String>,
    #[serde(default)]
    pub mcp_servers: Vec<String>,
    /// Environment name declared by each local stdio server, if any. Declarations
    /// only: the Run resolves them per node and server into its frozen bindings.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub environments: BTreeMap<String, String>,
}

/// Where a dependency environment is used. The site is part of the persisted
/// fact: without it a binding cannot be told apart from a binding for another
/// purpose, and a Run could not be validated against what it declared.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EnvironmentSite {
    /// The node's own task commands: AgentNode tools and `Op.run`.
    NodeTask { node: String },
    /// One local stdio MCP server of a Plugin bound to a node.
    PluginServer {
        node: String,
        plugin: String,
        server: String,
    },
}

/// One Plugin stdio server, as its launcher identifies it.
///
/// The frozen map is keyed by [`EnvironmentSite::key`] because a Run records
/// *where* an environment is used; a launcher only ever starts one server at a
/// time, so it keys its prepared environments by this pair instead of formatting
/// a string of its own.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginServerId {
    pub plugin: String,
    pub server: String,
}

impl PluginServerId {
    pub fn new(plugin: impl Into<String>, server: impl Into<String>) -> Self {
        Self {
            plugin: plugin.into(),
            server: server.into(),
        }
    }
}

impl std::fmt::Display for PluginServerId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}/{}", self.plugin, self.server)
    }
}

impl EnvironmentSite {
    /// The node this site belongs to.
    pub fn node(&self) -> &str {
        match self {
            EnvironmentSite::NodeTask { node } | EnvironmentSite::PluginServer { node, .. } => node,
        }
    }

    /// The Plugin server this site names, if it names one.
    pub fn plugin_server_id(&self) -> Option<PluginServerId> {
        match self {
            EnvironmentSite::PluginServer { plugin, server, .. } => {
                Some(PluginServerId::new(plugin, server))
            }
            EnvironmentSite::NodeTask { .. } => None,
        }
    }

    /// The site a canonical key names, if it names one.
    ///
    /// `key` is injective because Plugin and server references cannot contain
    /// `/` while node ids are not part of a `plugin_server:` key's tail.
    pub fn from_key(key: &str) -> Option<EnvironmentSite> {
        if let Some(node) = key.strip_prefix("node_task:") {
            return Some(EnvironmentSite::NodeTask {
                node: node.to_owned(),
            });
        }
        let rest = key.strip_prefix("plugin_server:")?;
        let (node, rest) = rest.split_once('/')?;
        let (plugin, server) = rest.split_once('/')?;
        Some(EnvironmentSite::PluginServer {
            node: node.to_owned(),
            plugin: plugin.to_owned(),
            server: server.to_owned(),
        })
    }

    /// The canonical key used in the persisted binding map.
    pub fn key(&self) -> String {
        match self {
            EnvironmentSite::NodeTask { node } => format!("node_task:{node}"),
            EnvironmentSite::PluginServer {
                node,
                plugin,
                server,
            } => format!("plugin_server:{node}/{plugin}/{server}"),
        }
    }
}

impl NodeExecutionRequest {
    /// The environment for this node's own task commands, if it declared one.
    pub fn task_environment(&self) -> Option<&EnvironmentBinding> {
        self.environments
            .values()
            .find(|binding| matches!(binding.site, Some(EnvironmentSite::NodeTask { .. })))
    }
}

/// A frozen dependency-environment binding: where it is used, the declared name
/// and the exact published digest the Run resolved it to at admission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentBinding {
    /// Absent only while a format-9 record is being migrated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site: Option<EnvironmentSite>,
    pub name: String,
    pub id: String,
}

impl EnvironmentBinding {
    pub fn node_task(
        node: impl Into<String>,
        name: impl Into<String>,
        id: impl Into<String>,
    ) -> Self {
        Self {
            site: Some(EnvironmentSite::NodeTask { node: node.into() }),
            name: name.into(),
            id: id.into(),
        }
    }
}

/// Persisted, non-secret facts for one unresolved tool call.
/// Tool arguments are not part of this recovery projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryAttempt {
    pub attempt_id: i64,
    pub step: u32,
    pub tool: String,
    pub started_at: String,
}

/// An explicit decision about an unresolved tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case", deny_unknown_fields)]
pub enum RecoveryDecision {
    Retry,
    Completed { observation: String },
    Abort,
}

/// The unresolved attempt is bound to the exact durable Graph invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingRecovery {
    pub key: InvocationKey,
    pub attempt: RecoveryAttempt,
}

/// Durable operator decision for one exact Harness attempt. Keeping this in
/// the Graph Run makes resubmission idempotent after the pending list advances.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoverySubmission {
    pub key: InvocationKey,
    pub attempt_id: i64,
    pub decision: RecoveryDecision,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CompletionFact {
    NotStarted,
    /// The host has a durable execution id/checkpoint for this invocation and
    /// may re-enter its own resume protocol. This does not authorize replaying
    /// an uncertain external effect; the execution backend remains responsible
    /// for requiring an explicit recovery decision where needed.
    Resumable,
    Completed(NodeCompletion),
    Yielded {
        reason: String,
        route: Option<String>,
    },
    Failed(String),
    Uncertain(String),
}

pub trait NodeExecutionPort: Send + Sync {
    fn capabilities(&self) -> NodeExecutionCapabilities;
    fn graph_call_port(&self) -> Option<&dyn GraphCallPort> {
        None
    }
    /// Resolve only public, immutable resource identity. Secrets and live
    /// clients stay in the host adapter. Default denies Plugin use.
    fn resolve_plugins(&self, _plugin_ids: &[String]) -> Result<Vec<PluginBinding>, GraphError> {
        Err(GraphError::Unsupported(
            "Plugin resolver is not configured".into(),
        ))
    }
    fn completion_fact<'a>(
        &'a self,
        key: &'a InvocationKey,
    ) -> Pin<Box<dyn Future<Output = Result<CompletionFact, GraphError>> + Send + 'a>>;
    /// Persist an operator decision before a waiting Graph Run is re-entered.
    /// Implementations must bind it to this invocation and attempt and reject
    /// conflicting resubmissions.
    fn record_recovery_decision(
        &self,
        _key: &InvocationKey,
        _attempt_id: i64,
        _decision: RecoveryDecision,
    ) -> Result<(), GraphError> {
        Err(GraphError::Unsupported(
            "node executor does not support tool recovery decisions".into(),
        ))
    }
    /// On `Completed`, `Yielded` or `Failed`, the implementation MUST durably write the
    /// matching completion fact before resolving this future. If it cannot
    /// establish whether execution completed it must persist/return Uncertain.
    /// BudgetExhausted and Suspended retain the same invocation's checkpoint and
    /// MUST NOT publish a completion fact.
    fn execute<'a>(
        &'a self,
        request: NodeExecutionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<NodeExecutionOutcome, GraphError>> + Send + 'a>>;

    /// The dependency views this invocation actually used, keyed by Plugin id.
    ///
    /// Read once when the node's result is written, so a Run records which view
    /// each step used. It is an observation: returning nothing is not a failure,
    /// and nothing here may refuse an invocation (standard invariant 6).
    fn observed_plugin_views(
        &self,
        _key: &InvocationKey,
    ) -> std::collections::BTreeMap<String, String> {
        std::collections::BTreeMap::new()
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NodeExecutionCapabilities {
    pub agent: bool,
    pub op_run: bool,
    pub host_operations: bool,
    /// True only when max_provider_requests is enforced as the cumulative
    /// provider request count, rather than translated to a turn count.
    pub exact_provider_request_budget: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NodeExecutionOutcome {
    Completed(NodeCompletion),
    Yielded {
        reason: String,
        route: Option<String>,
    },
    /// Unknown external tool effects require an operator decision before this
    /// invocation can advance. The Graph cursor remains unchanged.
    WaitingRecovery {
        attempts: Vec<RecoveryAttempt>,
    },
    BudgetExhausted {
        model_requests: u64,
    },
    Suspended,
    Cancelled,
    Interrupted {
        reason: String,
    },
    /// The operator explicitly chose to abort this invocation after an
    /// unresolved external tool effect. The containing Graph Run is terminal.
    Aborted,
    /// A known terminal node failure. The host has established that the
    /// invocation failed; the Graph Run records a terminal failure and never
    /// treats it as an invitation to replay the invocation.
    Failed {
        reason: String,
    },
}

pub trait RunControl: Send + Sync {
    fn pause_requested(&self) -> bool;
    fn stop_requested(&self) -> bool;
    fn cancellation(&self) -> crate::Cancellation;
}
