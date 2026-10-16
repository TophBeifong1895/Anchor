//! Executes one node request; graph scheduling remains in the shared Runner.
use super::{HostArtifacts, create_durable_directory, write_durable};
use crate::{
    local_inputs::LocalInputs,
    op,
    plugin_mounts::{self, MountSnapshot, PluginMountSource},
    tool_host,
};
use anchor_graph_host::PluginCatalog;
use anchor_runtime::graph::{
    CommitRef, CompletionFact, FileRunStore, GraphError, GraphRunRecord, InvocationKey,
    NodeCompletion, NodeExecutionCapabilities, NodeExecutionOutcome, NodeExecutionPort,
    NodeExecutionRequest, NodeKind, PluginBinding, RecoveryDecision, RunStore,
};
use anchor_runtime::{NetworkPolicy, SandboxError, SandboxPort, SandboxRequest, SandboxStatus};
use anchor_sandbox_bwrap::BubblewrapSandbox;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::{self, Write},
    path::{Path, PathBuf},
    pin::Pin,
    sync::atomic::Ordering,
    time::Duration,
};

mod assistant;
mod contracts;
mod conversation;
mod execution;
mod facts;
mod inputs;
mod media;
mod mounts;
mod preparation;
mod resolution;
mod turn_plugins;

pub(crate) use contracts::{
    NodeConversationHint, NodeHostResolver, NodeImage, ResolutionError, ResolvedNodeTools,
    ToolResolution,
};

pub(crate) fn conversation_hint_for(
    metadata: &crate::application::RunMetadata,
    node: &str,
) -> Option<NodeConversationHint> {
    let binding = metadata.conversation.as_ref()?;
    let identity = serde_json::to_vec(&(&metadata.bundle_source, &binding.session, node))
        .expect("host conversation identity is serializable");
    Some(NodeConversationHint {
        key: format!("{:x}", Sha256::digest(identity)),
    })
}

pub(crate) struct HostNodes {
    pub(crate) control: crate::HostControl,
    pub(crate) sandbox: std::sync::Arc<BubblewrapSandbox>,
    pub(crate) allowed_commands: Vec<String>,
    pub(crate) artifacts: HostArtifacts,
    pub(crate) facts_root: PathBuf,
    pub(crate) io_resolver: std::sync::Arc<HostIoResolver>,
    pub(crate) goose_nodes: Option<crate::goose_acp::GooseNodePort>,
}

/// Resolves current Library resources under the Run’s admitted authority. Tool clients and
/// sandbox access are owned by the returned ToolPort so the Goose MCP bridge
/// can bind them without borrowing HostNodes.
pub(crate) struct HostIoResolver {
    artifacts: HostArtifacts,
    sandbox: std::sync::Arc<BubblewrapSandbox>,
    tools: std::sync::Arc<tool_host::PluginToolHost>,
    mcp: tool_host::McpToolConfig,
    runtime_configuration:
        Option<std::sync::Arc<crate::runtime_configuration::RuntimeConfiguration>>,
    run_store: FileRunStore,
    /// The one place Plugins exist: the deployment's Library root. There is no
    /// second resolution path (no bundle-first, no catalog-first fallback).
    library_root: Option<PathBuf>,
    /// Which view each invocation actually resolved, keyed by the invocation's
    /// durable key. Read once when the node's result is written.
    observed_views: std::sync::Mutex<BTreeMap<String, BTreeMap<String, String>>>,
    /// The code digest each invocation actually mounted, keyed by its durable key.
    observed_plugin_digests: std::sync::Mutex<BTreeMap<String, BTreeMap<String, String>>>,
    local_inputs: LocalInputs,
    /// The read-only view of Session Plugin switches, injected by the host that
    /// owns the Session store. The resolver never opens that database itself.
    mounts: std::sync::Arc<dyn PluginMountSource>,
    /// Operator configuration supplied by the composing Host, not an HTTP entrypoint.
    pilot_graph: Option<String>,
}

use facts::fact_stem;
pub(crate) use facts::load_host_completion_fact;
use mounts::MountedPlugins;
pub(crate) use mounts::{freeze_session_switches, is_fake_fixture, session_plugin_mounts};

impl HostIoResolver {
    // One argument per host capability the resolver is allowed to reach; the host
    // that owns them wires them all at once.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        artifacts: HostArtifacts,
        sandbox: std::sync::Arc<BubblewrapSandbox>,
        tools: std::sync::Arc<tool_host::PluginToolHost>,
        mcp: tool_host::McpToolConfig,
        run_store: FileRunStore,
        local_inputs: LocalInputs,
        mounts: std::sync::Arc<dyn PluginMountSource>,
        pilot_graph: Option<String>,
    ) -> Self {
        Self {
            artifacts,
            sandbox,
            tools,
            mcp,
            runtime_configuration: None,
            run_store,
            library_root: None,
            observed_views: std::sync::Mutex::new(BTreeMap::new()),
            observed_plugin_digests: std::sync::Mutex::new(BTreeMap::new()),
            local_inputs,
            mounts,
            pilot_graph,
        }
    }

    /// Point the resolver at the deployment's Library root.
    ///
    /// The caller that wires the host decides this once (`crate::library_root`) and
    /// hands the same answer to every host in the process, so the API that installs
    /// a Plugin and the node call that runs it cannot disagree. `None` means the
    /// deployment configured no Library root: a Plugin-bearing call then refuses
    /// with a named error instead of guessing a path.
    pub(crate) fn with_library_root(mut self, root: Option<PathBuf>) -> Self {
        self.library_root = root;
        self
    }

    pub(crate) fn with_runtime_configuration(
        mut self,
        configuration: Option<std::sync::Arc<crate::runtime_configuration::RuntimeConfiguration>>,
    ) -> Self {
        self.runtime_configuration = configuration;
        self
    }

    /// The Library root Plugins are resolved from, or a reason there is none.
    pub(super) fn plugin_library_root(&self) -> Result<&Path, String> {
        self.library_root.as_deref().ok_or_else(|| {
            "Plugin-bearing execution requires the deployment's Library root (ANCHOR_RUNNER_LIBRARY_ROOT or ANCHOR_RUNNER_CATALOG_ROOT)".to_owned()
        })
    }
}

#[cfg(test)]
mod resolver_tests;
