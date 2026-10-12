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
mod media;

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
}

/// The reserved evidence key for a call's Plugin *mounting*, next to the
/// per-Plugin view digests: where the switches came from, and why a Plugin the
/// call named is not mounted. It is deliberately not a Plugin id.
const MOUNT_EVIDENCE_KEY: &str = "plugin-mounts";

/// The Plugin set one node call mounts, and the Session facts behind it.
#[derive(Debug)]
struct MountedPlugins {
    bindings: Vec<PluginBinding>,
    network: bool,
    /// One line per Plugin the call named that is *not* mounted, and why.
    reasons: Vec<String>,
    /// The Session whose switches applied, when the Run belongs to one.
    session: Option<String>,
    /// Whether a Turn snapshot pinned those switches.
    snapshot: bool,
    /// Plugins the Graph's declaration defaulted on (no click, no audit event).
    by_declaration: Vec<String>,
    /// Plugins a human's Session switch turned on (the audit event's subject).
    by_switch: Vec<String>,
    /// A snapshot file exists but could not be read: the switches are live and the
    /// observation says so, rather than claiming they were frozen.
    snapshot_unreadable: bool,
}

/// The deployment's in-process fake fixture. It is a binding the host enabled by
/// configuration, not a Plugin the Library has on disk.
pub(crate) fn is_fake_fixture(binding: &PluginBinding) -> bool {
    tool_host::is_fake_fixture(binding)
}

/// One Turn's Session switches, frozen when the Turn starts.
///
/// Run-independent on purpose. A node call reads them from its Run; the Pilot's
/// own Turn has no Run at all, and it must freeze exactly the same way: the first
/// caller of a Turn writes the snapshot, and every later caller — including a
/// retry — reads it, so a click made mid-Turn cannot change what that Turn uses.
pub(crate) struct FrozenSwitches {
    /// The switches this Turn applies: the snapshot's, once it exists.
    pub(crate) switches: BTreeMap<String, bool>,
    /// The snapshot itself, which is what `resolve` reads.
    pub(crate) snapshot: MountSnapshot,
    /// A snapshot file exists but could not be read: the switches above are the
    /// Session's *current* ones and the call must say so.
    pub(crate) snapshot_unreadable: bool,
}

/// Freeze (or read) one Turn's Session switches.
pub(crate) fn freeze_session_switches(
    state_root: &Path,
    session: &str,
    turn: &str,
    source: &dyn PluginMountSource,
) -> Result<FrozenSwitches, String> {
    let (switches, network_allowed) = source.session_policy(session)?;
    // The Turn's snapshot is written by its **first** caller, whatever that call
    // needs: a Turn whose first node has no Plugins must still pin the switches it
    // started with, or a click made mid-Turn would take effect immediately.
    let path = plugin_mounts::snapshot_path(state_root, session, turn);
    let existing = plugin_mounts::read_snapshot(&path, session, turn);
    let snapshot_unreadable = existing.is_none() && path.exists();
    let mut snapshot = match existing {
        Some(snapshot) => snapshot,
        None => {
            let mut candidate = MountSnapshot::new(session, turn, &switches);
            candidate.network_allowed = network_allowed;
            plugin_mounts::write_snapshot(&path, &candidate)
                .map_err(|error| format!("Turn Plugin mount snapshot is unavailable: {error}"))?;
            // Another attempt of this Turn may have won the race; the snapshot on
            // disk is the one this Turn uses.
            plugin_mounts::read_snapshot(&path, session, turn).unwrap_or(candidate)
        }
    };
    if snapshot_unreadable {
        // Mounts retain the existing compatibility fallback. An unreadable
        // permission snapshot cannot grant connectivity to this Turn.
        snapshot.network_allowed = false;
    }
    Ok(FrozenSwitches {
        switches: snapshot.mounts.clone(),
        snapshot,
        snapshot_unreadable,
    })
}

/// The Plugins one frozen Session turn mounts, without a Run and without a Graph.
///
/// The Pilot's own Turn has neither: a Plugin reaches it because a human mounted
/// it in the Session. The rules are the node call's rules — the Library must have
/// it (or the deployment's fake fixture must be enabled), and the view its own
/// declaration maps to must be usable. Nothing here builds, repairs or installs
/// anything, and `reasons` says why a switched-on Plugin is absent.
/// The caller holds the Library publication lease while selecting mounts.
pub(crate) fn session_plugin_mounts(
    library_root: &Path,
    fixture: &tool_host::PluginToolHost,
    switches: &BTreeMap<String, bool>,
) -> Result<(Vec<PluginBinding>, Vec<String>), String> {
    let catalog = anchor_graph_host::FilePluginCatalog::new(library_root);
    let mut installed = BTreeMap::new();
    let mut definitions = BTreeMap::new();
    for id in switches.keys() {
        let fake = fixture
            .resolve_fixture_plugins(std::slice::from_ref(id))
            .is_ok();
        let definition = if fake {
            None
        } else {
            catalog.definition(id).ok()
        };
        installed.insert(id.clone(), fake || definition.is_some());
        if let Some(definition) = definition {
            definitions.insert(id.clone(), definition);
        }
    }
    let resolved = plugin_mounts::resolve(None, switches, &[], &installed);
    let mut reasons = resolved.reasons;
    let mut bindings = Vec::new();
    for id in resolved.mounts.keys() {
        if let Ok(mut fixture_bindings) = fixture.resolve_fixture_plugins(std::slice::from_ref(id))
        {
            bindings.append(&mut fixture_bindings);
            continue;
        }
        let definition = definitions
            .get(id)
            .ok_or_else(|| format!("Plugin `{id}` is not available in this Library"))?;
        // A Plugin whose own declaration's view is not usable does not appear: a
        // mount never becomes a view build.
        if let Err(unusable) =
            crate::plugin_view::PreparedPlugin::from_definition(library_root, definition)
        {
            reasons.push(unusable.to_string());
            continue;
        }
        bindings.push(definition.binding());
    }
    Ok((bindings, reasons))
}

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
        }
    }

    /// Remember what this invocation observed about its Plugins, for the Run's
    /// own record: which view each one resolved, and where its mount switches
    /// came from together with why a Plugin the call named is not mounted.
    fn remember_plugin_observation(
        &self,
        key: &InvocationKey,
        views: BTreeMap<String, String>,
        digests: BTreeMap<String, String>,
        mounted: &MountedPlugins,
    ) {
        let mut observed = views;
        if let Some(session) = &mounted.session {
            let source = if mounted.snapshot {
                "this Turn's snapshot"
            } else if mounted.snapshot_unreadable {
                // A snapshot exists but could not be read: the switches are live,
                // and the observation says so instead of claiming they were frozen.
                "no snapshot (unreadable); the Session's current switches"
            } else {
                "the Session's current switches"
            };
            let mut value = format!("source={source}; session={session}");
            value.push_str(&format!("; network={}", mounted.network));
            if !mounted.by_declaration.is_empty() {
                value.push_str(&format!(
                    "; by-declaration=[{}]",
                    mounted.by_declaration.join(",")
                ));
            }
            if !mounted.by_switch.is_empty() {
                value.push_str(&format!("; by-switch=[{}]", mounted.by_switch.join(",")));
            }
            if !mounted.reasons.is_empty() {
                value.push_str("; ");
                value.push_str(&mounted.reasons.join("; "));
            }
            observed.insert(MOUNT_EVIDENCE_KEY.to_owned(), value);
        }
        if observed.is_empty() && digests.is_empty() {
            return;
        }
        if !observed.is_empty()
            && let Ok(mut views) = self.observed_views.lock()
        {
            views.insert(key.durable_key(), observed);
        }
        if !digests.is_empty()
            && let Ok(mut observations) = self.observed_plugin_digests.lock()
        {
            observations.insert(key.durable_key(), digests);
        }
    }

    /// Take the observation for one invocation (reading it consumes it, so a long
    /// lived host does not accumulate one entry per node call).
    pub(crate) fn observed_plugin_views(&self, key: &InvocationKey) -> BTreeMap<String, String> {
        self.observed_views
            .lock()
            .ok()
            .and_then(|mut observed| observed.remove(&key.durable_key()))
            .unwrap_or_default()
    }

    pub(crate) fn observed_plugin_digests(&self, key: &InvocationKey) -> BTreeMap<String, String> {
        self.observed_plugin_digests
            .lock()
            .ok()
            .and_then(|mut observed| observed.remove(&key.durable_key()))
            .unwrap_or_default()
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

    fn tool_environment(&self) -> Result<crate::tool_environment::ToolEnvironment, String> {
        let environment = self.mcp.environment.clone();
        match &self.runtime_configuration {
            Some(configuration) => environment.with_runtime_snapshot(&configuration.snapshot()?),
            None => Ok(environment),
        }
    }

    /// The Library root Plugins are resolved from, or a reason there is none.
    fn plugin_library_root(&self) -> Result<&Path, String> {
        self.library_root.as_deref().ok_or_else(|| {
            "Plugin-bearing execution requires the deployment's Library root (ANCHOR_RUNNER_LIBRARY_ROOT or ANCHOR_RUNNER_CATALOG_ROOT)".to_owned()
        })
    }

    pub(crate) fn local_inputs_configured(&self) -> bool {
        self.local_inputs.configured()
    }

    /// Prepare the one writable tree this invocation owns.
    ///
    /// A handed-over assistant Run seeds its work node's scene from the Run it
    /// took the instance over from; every other preparation is the plain
    /// unbound/committed-seed behaviour. The seed decision is a durable host fact
    /// (`crate::assistant::handover_workspace_source`), never graph content or
    /// user input.
    pub(crate) fn prepare_node_workspace(
        &self,
        key: &InvocationKey,
        inputs: &[CommitRef],
    ) -> Result<PathBuf, String> {
        let seed =
            crate::assistant::handover_workspace_source(self.local_inputs.state_root(), key)?;
        self.artifacts
            .prepare_workspace_from(key, inputs, seed.as_ref())
            .map_err(|error| error.to_string())
    }

    pub(crate) fn persistent_workspace(&self, key: &InvocationKey) -> Result<bool, String> {
        crate::assistant::source(self.local_inputs.state_root(), key).map(|source| source.is_some())
    }

    pub(crate) fn validate_plugin_configuration(
        &self,
        bindings: &[PluginBinding],
    ) -> Result<(), String> {
        self.catalog_config(bindings)?
            .validate_bindings(bindings, true)
    }

    pub(crate) fn bind_local_inputs(
        &self,
        record: &GraphRunRecord,
        graph: Option<&str>,
    ) -> Result<(), String> {
        let existing = self
            .run_store
            .load(&record.run_id)
            .map_err(|error| error.to_string())?;
        if let Some(existing) = &existing
            && (existing.snapshot != record.snapshot || existing.input != record.input)
        {
            return Err("Run local input identity differs from the admitted snapshot/input".into());
        }
        self.local_inputs.freeze(
            &record.run_id,
            &record.graph_digest,
            &record.snapshot,
            graph,
            existing.is_none(),
        )
    }

    pub(crate) fn verify_local_inputs(&self, record: &GraphRunRecord) -> Result<(), String> {
        self.local_inputs.verify(record)?;
        if self.local_inputs.configured() {
            let metadata =
                crate::application::metadata::load(self.local_inputs.state_root(), &record.run_id)
                    .map_err(|error| format!("local input Run identity unavailable: {error:?}"))?
                    .ok_or("local input authorization requires immutable Run Graph metadata")?;
            if metadata.graph_digest != record.graph_digest {
                return Err(
                    "local input Run Graph metadata differs from the admitted snapshot".into(),
                );
            }
            self.local_inputs.freeze(
                &record.run_id,
                &record.graph_digest,
                &record.snapshot,
                Some(&metadata.graph),
                false,
            )?;
        }
        Ok(())
    }

    fn local_input_mounts(
        &self,
        key: &InvocationKey,
    ) -> Result<Vec<anchor_runtime::ReadOnlyInput>, String> {
        let record = self
            .run_store
            .load(&key.run_id)
            .map_err(|error| error.to_string())?
            .ok_or("local input mounts require a durable admitted Run")?;
        // Old Runs could not receive local inputs. Only their empty authority
        // may be recorded during migration; new grants require a new Run.
        if !self.local_inputs.configured() {
            self.local_inputs.freeze(
                &record.run_id,
                &record.graph_digest,
                &record.snapshot,
                None,
                false,
            )?;
        }
        self.verify_local_inputs(&record)?;
        let mut mounts = self.local_inputs.mounts(&record, key)?;
        if let Some(mount) = self.channel_inputs(key)?.mount {
            mounts.push(mount);
        }
        mounts.extend(
            self.pending_channel_inputs(key)?
                .into_iter()
                .filter_map(|inputs| inputs.mount),
        );
        Ok(mounts)
    }

    /// Recheck the admitted authority before exposing a prepared current binding.
    fn validate_invocation_plugins(
        admitted: &GraphRunRecord,
        request: &NodeExecutionRequest,
        bindings: &[PluginBinding],
    ) -> Result<(), String> {
        for binding in bindings {
            let declared = request
                .plugins
                .iter()
                .any(|declared| declared.id == binding.id);
            match (admitted.plugin_bindings.get(&binding.id), declared) {
                // A Run freezes Host authority, while executable bytes and its
                // dependency view follow the current installed Plugin.
                (Some(frozen), _) if !frozen.same_authorization(binding) => {
                    return Err(format!(
                        "Plugin `{}` Host authorization changed since this Graph Run was admitted",
                        binding.id,
                    ));
                }
                // A Graph declaration with no admitted binding is corrupt.
                (None, true) => {
                    return Err(format!(
                        "Plugin `{}` is not bound by the admitted Run",
                        binding.id,
                    ));
                }
                // A human's frozen Session mount authorizes an additional Plugin.
                _ => {}
            }
        }
        Ok(())
    }

    fn catalog_config(
        &self,
        bindings: &[PluginBinding],
    ) -> Result<tool_host::McpToolConfig, String> {
        if bindings.iter().all(is_fake_fixture) {
            return Ok(self.mcp.clone());
        }
        let root = self.plugin_library_root()?;
        let mut config = tool_host::McpToolConfig::from_catalog(root, bindings)?;
        config.environment = self.mcp.environment.clone();
        Ok(config)
    }

    fn oauth_owner_for_run(&self, run_id: &str) -> Result<Option<String>, String> {
        crate::application::metadata::load(self.local_inputs.state_root(), run_id)
            .map(|metadata| metadata.and_then(|metadata| metadata.oauth_owner))
            .map_err(|error| format!("OAuth Run owner is unavailable: {error:?}"))
    }

    /// The Session and Turn this Run's mount switches come from.
    ///
    /// The immutable Run metadata is the only place that says which Session a Run
    /// belongs to: a pilot Run and a channel Run record Session and Turn, a
    /// persistent assistant records the Session and takes its Turn from the
    /// assistant invocation's own immutable binding, and a plain conversation
    /// records the Session only. When no Turn id is recorded, this Run *is* the
    /// unit whose switches are frozen, so it names itself. A Run in no Session
    /// mounts only what its Graph declares.
    fn session_turn(&self, key: &InvocationKey) -> Result<Option<(String, String)>, String> {
        let root = self.local_inputs.state_root();
        let Some(metadata) = crate::application::metadata::load(root, &key.run_id)
            .map_err(|error| format!("Run Session metadata unavailable: {error:?}"))?
        else {
            return Ok(None);
        };
        if let Some(source) = &metadata.pilot {
            return Ok(Some((source.session.clone(), source.turn.clone())));
        }
        if let Some(source) = &metadata.channel {
            return Ok(Some((source.session.clone(), source.turn.clone())));
        }
        if let Some(source) = &metadata.assistant {
            let turn = crate::assistant::binding(root, key)?
                .map(|binding| binding.turn)
                .unwrap_or_else(|| key.run_id.clone());
            return Ok(Some((source.session.clone(), turn)));
        }
        if let Some(call) = &metadata.session_call {
            return Ok(Some((call.context.session.clone(), key.run_id.clone())));
        }
        if let Some(source) = &metadata.conversation {
            return Ok(Some((source.session.clone(), key.run_id.clone())));
        }
        Ok(None)
    }

    /// The Plugins this node call actually mounts, and why the others are off.
    ///
    /// The Graph's declaration is a *default*: a required Plugin is always
    /// mounted, an optional one follows the Session's switches, and a Plugin the
    /// human mounted in the Session is mounted even though no Graph named it. A
    /// Plugin the Library does not have is not mounted, and `reasons` says so.
    ///
    /// The switches are frozen per Turn: the first call of a Turn writes the
    /// snapshot, and a retry of that Turn reuses it instead of reading a Session
    /// that changed in between.
    ///
    /// `required`/`default` come from the Graph's own declaration, read back out
    /// of the admitted Run by node: `required: true` is always mounted and cannot
    /// be switched off, an optional one follows "the Session's explicit value,
    /// else its `default`, else on". A bare id in an older Graph is required, which
    /// is what a declaration has always meant.
    fn mounted_plugins(&self, request: &NodeExecutionRequest) -> Result<MountedPlugins, String> {
        let _guard = self
            .library_root
            .as_ref()
            .map(|root| {
                anchor_library::Library::new(root)
                    .catalog_read_guard()
                    .map_err(|error| format!("Plugin mount catalog is unavailable: {error}"))
            })
            .transpose()?;
        self.mounted_plugins_unlocked(request)
    }

    /// The async tool path supplies its cancellable publication lease; Op.run
    /// uses the nonblocking read-guard wrapper above.
    fn mounted_plugins_unlocked(
        &self,
        request: &NodeExecutionRequest,
    ) -> Result<MountedPlugins, String> {
        let key = &request.key;
        let admitted = self
            .run_store
            .load(&key.run_id)
            .map_err(|error| format!("load admitted Run Plugin bindings: {error}"))?
            .ok_or_else(|| {
                format!(
                    "Plugin mount resolution requires a durable admitted Run `{}`",
                    key.run_id
                )
            })?;
        let session_turn = self.session_turn(key)?;
        let frozen = match &session_turn {
            Some((session, turn)) => Some(freeze_session_switches(
                self.local_inputs.state_root(),
                session,
                turn,
                self.mounts.as_ref(),
            )?),
            None => None,
        };
        let switches = frozen
            .as_ref()
            .map(|frozen| frozen.switches.clone())
            .unwrap_or_default();
        let snapshot_unreadable = frozen
            .as_ref()
            .is_some_and(|frozen| frozen.snapshot_unreadable);
        let snapshot = frozen.as_ref().map(|frozen| &frozen.snapshot);
        if request.plugins.is_empty() && switches.is_empty() {
            return Ok(MountedPlugins {
                bindings: Vec::new(),
                network: request.network && session_turn.is_none(),
                reasons: Vec::new(),
                session: session_turn.map(|(session, _)| session),
                snapshot: snapshot.is_some(),
                snapshot_unreadable,
                by_declaration: Vec::new(),
                by_switch: Vec::new(),
            });
        }
        let mut reasons = Vec::new();
        let catalog = anchor_graph_host::FilePluginCatalog::new(self.plugin_library_root()?);
        // `declared` only scopes the reasons; the mounted set may name a Plugin
        // the Graph never declared, which is exactly the point.
        // A `required: true` declaration is an authorization, so only the
        // deployment's own pilot Graph may hand it out: that is the Graph an
        // operator authors and reviews. Any *other* Graph -- one an assistant
        // created or edited in this Session -- has its `required` downgraded to
        // optional, so the assistant cannot grant itself a Plugin the human cannot
        // switch off. It still mounts when the Session's switches say so.
        // The one authority the API's graph routes and the Session view also read.
        let configured_pilot_graph = crate::api::configured_pilot_graph();
        let declaring_graph =
            crate::application::metadata::load(self.local_inputs.state_root(), &key.run_id)
                .ok()
                .flatten()
                .map(|metadata| metadata.graph);
        let required_is_authoritative = declaring_graph.is_some()
            && declaring_graph.as_deref() == configured_pilot_graph.as_deref();
        if !required_is_authoritative
            && admitted
                .snapshot
                .nodes
                .iter()
                .any(|node| node.plugins.iter().any(|reference| reference.required))
        {
            reasons.push(
                "a `required: true` declaration from a Graph that is not the deployment's pilot Graph is treated as optional"
                    .to_owned(),
            );
        }
        let declared = admitted
            .snapshot
            .nodes
            .iter()
            .find(|node| node.id == key.node_id)
            .map(|node| {
                node.plugins
                    .iter()
                    .map(|reference| {
                        (
                            reference.id.as_str(),
                            reference.required && required_is_authoritative,
                            reference.default,
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut installed = BTreeMap::new();
        for id in declared
            .iter()
            .map(|(id, _, _)| *id)
            .chain(switches.keys().map(String::as_str))
        {
            if installed.contains_key(id) {
                continue;
            }
            let fake = self.tools.resolve_fixture_plugins(&[id.to_owned()]).is_ok();
            installed.insert(id.to_owned(), fake || catalog.definition(id).is_ok());
        }
        let mut resolved = plugin_mounts::resolve(snapshot, &switches, &declared, &installed);
        resolved.reasons.splice(0..0, reasons);
        if snapshot_unreadable {
            resolved.snapshot = false;
            resolved.reasons.push("no snapshot (unreadable)".to_owned());
        }
        let mut bindings = Vec::new();
        for id in resolved.mounts.keys() {
            // Always resolve current Library code. The Run stores an authorization
            // baseline; it does not pin executable bytes or dependency views.
            let binding = self
                .tools
                .resolve_fixture_plugins(std::slice::from_ref(id))
                .ok()
                .and_then(|mut bindings| bindings.pop())
                .map(Ok)
                .unwrap_or_else(|| {
                    catalog
                        .resolve(std::slice::from_ref(id))
                        .map_err(|error| format!("Plugin `{id}` is not available: {error}"))?
                        .pop()
                        .ok_or_else(|| format!("Plugin `{id}` is not available in this Library"))
                })?;
            if let Some(admitted_binding) = admitted.plugin_bindings.get(id)
                && !admitted_binding.same_authorization(&binding)
            {
                return Err(format!(
                    "Plugin `{id}` Host authorization changed since this Graph Run was admitted"
                ));
            }
            bindings.push(binding);
        }
        let network = if let Some(snapshot) = snapshot {
            crate::plugin_network::apply(
                self.plugin_library_root()?,
                &mut bindings,
                request.network && snapshot.network_allowed,
                &mut resolved.reasons,
            )?
        } else {
            request.network
        };
        Ok(MountedPlugins {
            bindings,
            network,
            reasons: resolved.reasons,
            session: session_turn.map(|(session, _)| session),
            snapshot: resolved.snapshot,
            snapshot_unreadable,
            by_declaration: resolved.by_declaration,
            by_switch: resolved.by_switch,
        })
    }
}

impl NodeHostResolver for HostIoResolver {
    fn prompt_images(&self, request: &NodeExecutionRequest) -> Result<Vec<NodeImage>, String> {
        let mut images = self.channel_inputs(&request.key)?.images;
        images.extend(
            self.pending_channel_inputs(&request.key)?
                .into_iter()
                .flat_map(|inputs| inputs.images),
        );
        Ok(images
            .into_iter()
            .map(|image| NodeImage {
                data: image.bytes,
                media_type: image.media_type,
            })
            .collect())
    }
    fn conversation_hint(
        &self,
        request: &NodeExecutionRequest,
    ) -> Result<Option<NodeConversationHint>, String> {
        self.conversation(&request.key)
            .map(|conversation| conversation.map(|value| NodeConversationHint { key: value.key }))
    }

    fn resolve_plugins(&self, ids: &[String]) -> Result<Vec<PluginBinding>, String> {
        let catalog = anchor_graph_host::FilePluginCatalog::new(self.plugin_library_root()?);
        ids.iter()
            .map(|id| {
                self.tools
                    .resolve_fixture_plugins(std::slice::from_ref(id))
                    .ok()
                    .and_then(|mut bindings| bindings.pop())
                    .map(Ok)
                    .unwrap_or_else(|| {
                        catalog
                            .resolve(std::slice::from_ref(id))
                            .map_err(|error| error.to_string())?
                            .pop()
                            .ok_or_else(|| format!("Plugin `{id}` is not installed in the Library"))
                    })
            })
            .collect()
    }

    fn workspace(&self, request: &NodeExecutionRequest) -> Result<PathBuf, String> {
        let workspace = self.prepare_node_workspace(&request.key, &request.input_commits)?;
        self.seed_assistant_workspace(&request.key, &workspace)?;
        Ok(workspace)
    }

    fn tools<'a>(&'a self, request: &'a NodeExecutionRequest) -> ToolResolution<'a> {
        Box::pin(async move {
            // The error type is named once, here: every `?` inside then converts a
            // host-side string into `ResolutionError::Host`, while the environment
            // sites keep their own cause.
            let resolved: Result<ResolvedNodeTools, ResolutionError> = async move {
                let environment = self.tool_environment()?;
                // The first fallible step names the error type, so every later `?`
                // converts a host-side reason into `ResolutionError::Host` while the
                // environment sites keep their own cause.
                let admitted = self
                    .run_store
                    .load(&request.key.run_id)
                    .map_err(|error| {
                        ResolutionError::Host(format!("load admitted Run Plugin bindings: {error}"))
                    })?
                    .ok_or_else(|| {
                        ResolutionError::Host(format!(
                            "Run `{}` has no durable Plugin binding record",
                            request.key.run_id
                        ))
                    })?;
                if admitted.graph_digest != request.key.graph_digest
                    || !admitted.plugin_bindings_initialized
                {
                    return Err(ResolutionError::Host(format!(
                        "Run `{}` has no matching initialized Plugin binding record",
                        request.key.run_id
                    )));
                }
                // The set this invocation actually mounts: the Graph's
                // defaults filtered by the Session's switches and frozen for
                // this Turn. Everything below uses this set, so there is one
                // filter point and not several.
                let mount_lease = match &self.library_root {
                    Some(root)
                        if !request.plugins.is_empty()
                            || self.session_turn(&request.key)?.is_some() =>
                    {
                        let access = crate::dependency_provision::DependencyAccess {
                            network: false,
                            cancellation: request.cancellation.clone(),
                        };
                        Some(
                            crate::application::plugins::plugin_publication_lease(
                                root,
                                &access,
                                "invocation mount selection",
                            )
                            .await
                            .map_err(|error| {
                                format!("Plugin mount selection is unavailable: {error:?}")
                            })?,
                        )
                    }
                    _ => None,
                };
                let mut mounted = self.mounted_plugins_unlocked(request)?;
                drop(mount_lease);
                Self::validate_invocation_plugins(&admitted, request, &mounted.bindings)?;
                let prepared = crate::plugin_view::prepare_plugins_for_owner_async(
                    mounted.bindings.clone(),
                    self.library_root.clone(),
                    self.tools.as_ref().clone(),
                    request.cancellation.clone(),
                    self.oauth_owner_for_run(&request.key.run_id)?
                        .map(|owner| (self.local_inputs.state_root().to_path_buf(), owner)),
                )
                .await
                .map_err(|error| ResolutionError::Graph(error.into_graph_error()))?;
                Self::validate_invocation_plugins(&admitted, request, &prepared.bindings)?;
                mounted.bindings = prepared.bindings;
                let workspace = self.workspace(request)?;
                let mut local_inputs = self.local_input_mounts(&request.key)?;
                let conversation = self.conversation(&request.key)?;
                if let Some(conversation) = &conversation
                    && let Some(mount) = self.previous_mount(&request.key, conversation)?
                {
                    local_inputs.push(mount);
                }
                // Everything this invocation mounts is granted on the invocation
                // itself: the Run policy is the deployment floor, not a list of
                // per-Run trees (a parent Run cannot know its children's Plugins or
                // environments).
                let plugin_mounts = prepared.mounts;
                let mut grants = local_inputs.clone();
                grants.extend(plugin_mounts.iter().cloned());
                let sandbox = std::sync::Arc::new(
                    environment
                        .grant_sandbox(&self.sandbox)?
                        .with_readonly_grants(&grants)
                        .map_err(|error| error.to_string())?,
                );
                let mut readonly_inputs = self
                    .artifacts
                    .input_mounts(
                        &request.input_commits,
                        &request.key.run_id,
                        &request.key.graph_digest,
                    )
                    .map_err(|error| error.to_string())?;
                readonly_inputs.extend(plugin_mounts);
                readonly_inputs.extend(local_inputs);
                let mut mcp = prepared.mcp;
                mcp.environment = environment.clone();
                mcp.startup_cancellation = Some(request.cancellation.clone());
                mcp.authorize_oauth(
                    self.oauth_owner_for_run(&request.key.run_id)?,
                    mounted.network,
                )
                .await?;
                let plugin_views = prepared.views;
                self.remember_plugin_observation(
                    &request.key,
                    plugin_views
                        .iter()
                        .filter_map(|(id, plugin)| {
                            plugin
                                .declaration_digest()
                                .map(|digest| (id.clone(), digest.to_owned()))
                        })
                        .collect(),
                    mounted
                        .bindings
                        .iter()
                        .map(|binding| (binding.id.clone(), binding.digest.clone()))
                        .collect(),
                    &mounted,
                );
                let plugins = self
                    .tools
                    .assemble_with_sandbox(
                        &mcp,
                        &mounted.bindings,
                        mounted.network,
                        crate::tool_host::SandboxLaunch {
                            sandbox: &sandbox,
                            workspace: &workspace,
                            readonly_inputs: &readonly_inputs,
                            plugin_views: &plugin_views,
                        },
                    )
                    .await?;
                // Output beyond the stream previews is retained host-side and mounted
                // read-only at /spill, so a model can read what was cut off.
                let spill = crate::node_tools::SpillDirectory::new(
                    self.local_inputs
                        .state_root()
                        .join("spill")
                        .join(&request.key.run_id)
                        .join(
                            workspace
                                .file_name()
                                .map(|name| name.to_string_lossy().into_owned())
                                .unwrap_or_else(|| request.key.node_id.clone()),
                        ),
                    "/spill",
                );
                let node_tools = crate::node_tools::NodeTools::new(
                    crate::channel_tools::wrap(
                        plugins,
                        &mounted.bindings,
                        request.key.clone(),
                        self.local_inputs.state_root().to_path_buf(),
                        conversation.as_ref().map(|value| value.reply_node.clone()),
                        workspace.clone(),
                        readonly_inputs.clone(),
                        request.cancellation.clone(),
                    ),
                    sandbox,
                    workspace,
                    readonly_inputs,
                    request.cancellation.clone(),
                )
                .with_spill(spill)
                .with_environment(environment)
                .with_network(mounted.network)
                .with_timeout_ceiling(crate::node_tools::command_timeout_ceiling(
                    request.wall_time_limit_seconds,
                ));
                let tools =
                    std::sync::Arc::new(node_tools) as std::sync::Arc<dyn anchor_runtime::ToolPort>;
                // Tools a node's completion and continuity depend on stay listed; the rest
                // (plugins, channel tools) becomes reachable on demand.
                let mut always_visible = vec![
                    crate::node_tools::RUN_TOOL_NAME.to_owned(),
                    crate::node_tools::READ_TOOL_NAME.to_owned(),
                    crate::node_tools::EDIT_TOOL_NAME.to_owned(),
                ];
                let tools = match conversation {
                    Some(conversation) => {
                        always_visible
                            .push(crate::node_host::conversation::HISTORY_TOOL.to_owned());
                        self.conversation_tools(tools, &request.key, conversation)
                    }
                    None => tools,
                };
                Ok::<_, ResolutionError>(ResolvedNodeTools {
                    tools: crate::tool_disclosure::maybe_wrap(
                        tools,
                        crate::tool_disclosure::always_visible(always_visible),
                    ),
                    plugins: mounted.bindings,
                })
            }
            .await;
            resolved
        })
    }
}

fn fact_stem(key: &InvocationKey) -> String {
    format!("nf1-{:x}", Sha256::digest(key.durable_key().as_bytes()))
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, GraphError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn load_host_completion_fact(
    root: &Path,
    key: &InvocationKey,
) -> Result<CompletionFact, GraphError> {
    let mut stems = vec![fact_stem(key)];
    // The former host admitted only single, simple node names. Preserve its
    // result/uncertainty facts without using graph paths as filesystem paths.
    if [&key.run_id, &key.graph_digest, &key.node_id]
        .iter()
        .all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        })
    {
        stems.push(key.durable_key().replace(':', "_"));
    }
    for stem in stems {
        if let Some(bytes) = read_optional(&root.join(format!("{stem}.json")))? {
            return serde_json::from_slice(&bytes)
                .map(CompletionFact::Completed)
                .map_err(|e| GraphError::CorruptRun(e.to_string()));
        }
        if let Some(bytes) = read_optional(&root.join(format!("{stem}.failed")))? {
            return String::from_utf8(bytes)
                .map(CompletionFact::Failed)
                .map_err(|e| GraphError::CorruptRun(e.to_string()));
        }
        if read_optional(&root.join(format!("{stem}.started")))?.is_some() {
            return Ok(CompletionFact::Uncertain(
                "node was durably started but has no terminal result; automatic replay is refused"
                    .into(),
            ));
        }
    }
    Ok(CompletionFact::NotStarted)
}

/// Read the deterministic routing marker written by the anchor-route Op helper.
/// The graph kernel remains responsible for checking that the selected route
/// is an actual outgoing edge, and for requiring a choice when there are
/// multiple exits.
fn read_op_route(stdout: &str) -> (String, Option<String>) {
    let lines = stdout.lines().collect::<Vec<_>>();
    let Some((index, first)) = lines
        .iter()
        .enumerate()
        .find(|(_, line)| !line.trim().is_empty())
    else {
        return (String::new(), None);
    };
    let Some(target) = first.trim().strip_prefix("ANCHOR_ROUTE:") else {
        return (stdout.trim().to_owned(), None);
    };
    let submission = lines[index + 1..].join("\n").trim().to_owned();
    (submission, Some(target.trim().to_owned()))
}

impl HostNodes {
    fn start(&self, key: &InvocationKey) -> Result<(), GraphError> {
        create_durable_directory(&self.facts_root)?;
        let path = self.facts_root.join(format!("{}.started", fact_stem(key)));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        file.write_all(b"started\n")?;
        file.sync_all()?;
        std::fs::File::open(&self.facts_root)?.sync_all()?;
        Ok(())
    }

    fn complete(
        &self,
        key: &InvocationKey,
        completion: NodeCompletion,
    ) -> Result<NodeExecutionOutcome, GraphError> {
        create_durable_directory(&self.facts_root)?;
        let bytes =
            serde_json::to_vec(&completion).map_err(|e| GraphError::CorruptRun(e.to_string()))?;
        write_durable(
            &self.facts_root.join(format!("{}.json", fact_stem(key))),
            &bytes,
        )?;
        Ok(NodeExecutionOutcome::Completed(completion))
    }

    async fn execute_node(
        &self,
        request: NodeExecutionRequest,
    ) -> Result<NodeExecutionOutcome, GraphError> {
        if request.kind != NodeKind::OpRun {
            return Err(GraphError::Unsupported(
                "HostNodes deterministic executor accepts Op.run only".into(),
            ));
        }
        let workspace = self
            .artifacts
            .prepare_workspace(&request.key, &request.input_commits)?;
        let local_inputs = self
            .io_resolver
            .local_input_mounts(&request.key)
            .map_err(GraphError::Unsupported)?;
        let mut readonly_inputs = self.artifacts.input_mounts(
            &request.input_commits,
            &request.key.run_id,
            &request.key.graph_digest,
        )?;
        readonly_inputs.extend(local_inputs.clone());
        if !request.plugins.is_empty() {
            return Err(GraphError::Unsupported(
                "Op.run cannot execute Plugins".into(),
            ));
        }
        let command = request
            .operation
            .as_ref()
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| GraphError::Unsupported("Op.run must be a command string".into()))?;
        // GraphRunner wraps the frozen Run input with upstream artifact metadata.
        // Business commands receive only the effective Graph input.
        let run_input = request.input.get("input").ok_or_else(|| {
            GraphError::Unsupported("Op.run request is missing effective Graph input".into())
        })?;
        let input = serde_json::to_string(run_input).map_err(|error| {
            GraphError::Unsupported(format!("Op.run input is not JSON: {error}"))
        })?;
        let argv =
            op::shell_command(command, &self.allowed_commands).map_err(GraphError::Unsupported)?;
        let route_helper = op::route_helper_mount().map_err(GraphError::Unsupported)?;
        let environment = self
            .io_resolver
            .tool_environment()
            .map_err(GraphError::Unsupported)?;
        let sandbox = environment
            .grant_sandbox(&self.sandbox)
            .map_err(GraphError::Unsupported)?
            .with_readonly_grants(&local_inputs)
            .and_then(|sandbox| sandbox.with_readonly_grants(std::slice::from_ref(&route_helper)))
            .map_err(|error| GraphError::Unsupported(error.to_string()))?;
        let network = self
            .io_resolver
            .mounted_plugins(&request)
            .map_err(GraphError::Unsupported)?
            .network;
        let mut sandbox_request = SandboxRequest {
            writable_inputs: Vec::new(),
            workspace,
            working_directory: None,
            command: argv,
            readonly_inputs,
            workspace_readonly: vec![],
            tool_dirs: vec![],
            devices: Vec::new(),
            environment: vec![
                anchor_runtime::SandboxEnvironment::new("ANCHOR_NODE", request.key.node_id.clone()),
                anchor_runtime::SandboxEnvironment::new("ANCHOR_ROUTES", request.routes.join(",")),
                anchor_runtime::SandboxEnvironment::new("ANCHOR_INPUT", input),
            ],
            network: if network {
                NetworkPolicy::Enabled
            } else {
                NetworkPolicy::Disabled
            },
            timeout: Duration::from_secs_f64(request.wall_time_limit_seconds.unwrap_or(3600.0)),
            max_output_bytes: 64 * 1024,
            spill: None,
            cancellation: request.cancellation.clone(),
        };
        // The deployment's operator-registered tool paths are this invocation's
        // authority: an Op.run reaches them by absolute path, and nothing else
        // grants them.
        op::add_route_helper(&mut sandbox_request, route_helper);
        environment.apply(&mut sandbox_request);
        self.start(&request.key)?;
        let result = match sandbox.run(sandbox_request).await {
            Ok(result) => result,
            Err(SandboxError::InvalidRequest(reason)) => {
                write_durable(
                    &self
                        .facts_root
                        .join(format!("{}.failed", fact_stem(&request.key))),
                    reason.as_bytes(),
                )?;
                return Ok(NodeExecutionOutcome::Failed { reason });
            }
            Err(error) => return Err(GraphError::Unsupported(error.to_string())),
        };
        match result.status {
            SandboxStatus::Completed if result.exit_code == Some(0) => {
                let (submission, route) = read_op_route(&result.stdout);
                self.complete(&request.key, NodeCompletion {
                    submission,
                    route,
                    model_requests: 0,
                    output: json!({"stdout":result.stdout,"stderr":result.stderr,"exit_code":result.exit_code}),
                })
            }
            SandboxStatus::Cancelled => Ok(NodeExecutionOutcome::Cancelled),
            _ => {
                let mut reason = format!("{} (exit_code={:?})", result.reason, result.exit_code);
                for output in [&result.stdout, &result.stderr] {
                    if !output.trim().is_empty() {
                        reason.push('\n');
                        reason.push_str(output.trim());
                    }
                }
                write_durable(
                    &self
                        .facts_root
                        .join(format!("{}.failed", fact_stem(&request.key))),
                    reason.as_bytes(),
                )?;
                Ok(NodeExecutionOutcome::Failed { reason })
            }
        }
    }
}

impl NodeExecutionPort for HostNodes {
    fn capabilities(&self) -> NodeExecutionCapabilities {
        NodeExecutionCapabilities {
            agent: self.goose_nodes.is_some(),
            op_run: true,
            host_operations: true,
            exact_provider_request_budget: false,
        }
    }
    fn resolve_plugins(&self, ids: &[String]) -> Result<Vec<PluginBinding>, GraphError> {
        self.io_resolver
            .resolve_plugins(ids)
            .map_err(GraphError::Unsupported)
    }
    fn completion_fact<'a>(
        &'a self,
        key: &'a InvocationKey,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<CompletionFact, GraphError>> + Send + 'a>>
    {
        Box::pin(async move {
            if let Some(fact) = self.assistant_completion_fact(key)? {
                return Ok(fact);
            }
            let host = load_host_completion_fact(&self.facts_root, key)?;
            if let CompletionFact::Completed(completion) = &host {
                self.settle_assistant_reply(key, completion)?;
            }
            if crate::run_data::legacy_invocation_exists(
                self.io_resolver.local_inputs.state_root(),
                key,
            )
            .map_err(GraphError::Unsupported)?
            {
                return Ok(CompletionFact::Uncertain(
                    "legacy Agent state is not resumable by the standard Goose Host".into(),
                ));
            }
            if let Some(nodes) = &self.goose_nodes {
                let goose = nodes.completion_fact(key)?;
                return Ok(match (&host, &goose) {
                    (CompletionFact::NotStarted, _) => goose,
                    (_, CompletionFact::NotStarted) => host,
                    _ if host == goose => host,
                    _ => CompletionFact::Uncertain("host and Goose node facts conflict".into()),
                });
            }
            Ok(host)
        })
    }
    fn record_recovery_decision(
        &self,
        _key: &InvocationKey,
        _attempt_id: i64,
        _decision: RecoveryDecision,
    ) -> Result<(), GraphError> {
        Err(GraphError::Unsupported(
            "legacy Agent recovery is disabled in the standard Goose Host".into(),
        ))
    }
    fn observed_plugin_views(&self, key: &InvocationKey) -> BTreeMap<String, String> {
        self.io_resolver.observed_plugin_views(key)
    }
    fn observed_plugin_digests(&self, key: &InvocationKey) -> BTreeMap<String, String> {
        self.io_resolver.observed_plugin_digests(key)
    }
    fn execute<'a>(
        &'a self,
        request: NodeExecutionRequest,
    ) -> Pin<
        Box<dyn std::future::Future<Output = Result<NodeExecutionOutcome, GraphError>> + Send + 'a>,
    > {
        if request.kind == NodeKind::OpHost {
            return Box::pin(self.execute_host_operation(request));
        }
        match crate::assistant::source(self.io_resolver.local_inputs.state_root(), &request.key) {
            Ok(Some(_)) => return Box::pin(self.execute_assistant_request(request)),
            Ok(None) => {}
            Err(error) => return Box::pin(async move { Err(GraphError::Unsupported(error)) }),
        }
        self.execute_regular(request)
    }
}

impl HostNodes {
    fn execute_regular<'a>(
        &'a self,
        request: NodeExecutionRequest,
    ) -> Pin<
        Box<dyn std::future::Future<Output = Result<NodeExecutionOutcome, GraphError>> + Send + 'a>,
    > {
        if request.kind == NodeKind::Agent {
            match crate::run_data::legacy_invocation_exists(
                self.io_resolver.local_inputs.state_root(),
                &request.key,
            ) {
                Ok(false) => {}
                Ok(true) => {
                    return Box::pin(async {
                        Err(GraphError::Unsupported(
                            "legacy Agent state cannot be executed as a Goose invocation".into(),
                        ))
                    });
                }
                Err(error) => return Box::pin(async move { Err(GraphError::Unsupported(error)) }),
            }
            if let Some(nodes) = &self.goose_nodes {
                return Box::pin(nodes.execute(request));
            }
            return Box::pin(async {
                Err(GraphError::Unsupported(
                    "Agent execution requires a configured Goose runtime".into(),
                ))
            });
        }
        Box::pin(async move {
            let cancellation = request.cancellation.clone();
            let timeout =
                Duration::try_from_secs_f64(request.wall_time_limit_seconds.unwrap_or(3600.0))
                    .map_err(|_| GraphError::Unsupported("invalid node wall time".into()))?;
            let cancel = async {
                while !cancellation.load(Ordering::Relaxed) {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            };
            tokio::select! {
                biased;
                _ = cancel => Ok(NodeExecutionOutcome::Cancelled),
                result = tokio::time::timeout(timeout, self.execute_node(request)) => result
                    .map_err(|_| GraphError::Unsupported("node wall time exceeded; unfinished effects are uncertain".into()))?,
            }
        })
    }
}

#[cfg(test)]
mod resolver_tests {
    use super::*;
    use anchor_runtime::graph::GraphSnapshot;
    use anchor_sandbox_bwrap::BubblewrapPolicy;
    use std::sync::atomic::AtomicBool;

    /// A Session mount source a test can fix, and then change under a Turn.
    struct FixedMounts(std::sync::Mutex<BTreeMap<String, bool>>);

    impl FixedMounts {
        fn new(entries: &[(&str, bool)]) -> Self {
            Self(std::sync::Mutex::new(
                entries
                    .iter()
                    .map(|(id, on)| ((*id).to_owned(), *on))
                    .collect(),
            ))
        }

        fn set(&self, id: &str, on: bool) {
            self.0.lock().unwrap().insert(id.to_owned(), on);
        }
    }

    impl PluginMountSource for FixedMounts {
        fn session_mounts(&self, _session: &str) -> Result<BTreeMap<String, bool>, String> {
            Ok(self.0.lock().unwrap().clone())
        }
    }

    fn fixed(entries: &[(&str, bool)]) -> std::sync::Arc<dyn PluginMountSource> {
        std::sync::Arc::new(FixedMounts::new(entries))
    }

    fn sandbox(workspace: &Path) -> std::sync::Arc<BubblewrapSandbox> {
        std::sync::Arc::new(
            BubblewrapSandbox::new(
                BubblewrapPolicy::new("bwrap", ["sh"]).authorize_workspace_root(workspace),
            )
            .unwrap(),
        )
    }

    fn library_with(root: &Path, ids: &[&str]) -> PathBuf {
        let library = root.join("library");
        std::fs::create_dir_all(library.join("plugins")).unwrap();
        for id in ids {
            let directory = library.join("plugins").join(id);
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(
                directory.join("plugin.json"),
                json!({"name": id}).to_string(),
            )
            .unwrap();
        }
        library
    }

    /// A Run in one pilot Session/Turn whose Graph declares `declared`, with
    /// `installed` present in the Library and `switches` set on the Session.
    /// `declared` is `(id, required, default)`: what the *Graph* says, which is
    /// the default the Session's switches are applied to.
    fn session_fixture(
        installed: &[&str],
        declared: &[(&str, bool, Option<bool>)],
        switches: &[(&str, bool)],
    ) -> (
        tempfile::TempDir,
        HostIoResolver,
        std::sync::Arc<FixedMounts>,
        NodeExecutionRequest,
    ) {
        let root = tempfile::tempdir().unwrap();
        let state = root.path().join("state");
        let workspace = root.path().join("workspaces");
        let artifacts = root.path().join("artifacts");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&artifacts).unwrap();
        let library = library_with(root.path(), installed);
        let declared_json = declared
            .iter()
            .map(|(id, required, default)| {
                let mut item = json!({"id": id, "required": required});
                if let Some(default) = default {
                    item["default"] = json!(default);
                }
                item
            })
            .collect::<Vec<_>>();
        let snapshot = GraphSnapshot::admit(json!({
            "objective":"session mounts","entry":"agent",
            "agents":{"worker":{"model":"fixture"}},
            "nodes":[{"id":"agent","agent":"worker","plugins":declared_json}],"edges":[]
        }))
        .unwrap();
        let mut record =
            GraphRunRecord::create_with_id(snapshot, json!({}), "session-mounts").unwrap();
        record.plugin_bindings = anchor_graph_host::FilePluginCatalog::new(&library)
            .resolve(
                &declared
                    .iter()
                    .map(|(id, _, _)| (*id).to_owned())
                    .collect::<Vec<_>>(),
            )
            .unwrap()
            .into_iter()
            .map(|binding| (binding.id.clone(), binding))
            .collect();
        record.plugin_bindings_initialized = true;
        let store = FileRunStore::new(state.join("runs"));
        store.save(&record).unwrap();
        let mut metadata = crate::application::RunMetadata::new(
            record.run_id.clone(),
            "session-mounts".into(),
            record.graph_digest.clone(),
            &library,
        )
        .unwrap();
        metadata.pilot = Some(crate::application::metadata::PilotRunSource {
            owner: "alice".into(),
            session: "session-one".into(),
            turn: "turn-one".into(),
        });
        crate::application::metadata::save(&state, &metadata).unwrap();
        let mounts = std::sync::Arc::new(FixedMounts::new(switches));
        let resolver = HostIoResolver::new(
            HostArtifacts::new(artifacts, workspace.clone()),
            sandbox(&workspace),
            std::sync::Arc::new(tool_host::PluginToolHost::new(Vec::<String>::new())),
            tool_host::McpToolConfig::default(),
            store,
            LocalInputs::new(state, None).unwrap(),
            mounts.clone(),
        )
        .with_library_root(Some(library));
        let request = NodeExecutionRequest {
            key: InvocationKey {
                run_id: record.run_id,
                graph_digest: record.graph_digest,
                node_id: "agent".into(),
                invocation: 1,
            },
            model: Some("fixture".into()),
            task: String::new(),
            instructions: String::new(),
            routes: Vec::new(),
            input: json!({}),
            input_commits: Vec::new(),
            plugins: record.plugin_bindings.values().cloned().collect(),
            max_provider_requests: None,
            wall_time_limit_seconds: Some(30.0),
            network: false,
            kind: NodeKind::Agent,
            operation: None,
            cancellation: std::sync::Arc::new(AtomicBool::new(false)),
        };
        (root, resolver, mounts, request)
    }

    fn mounted_ids(mounted: &MountedPlugins) -> Vec<String> {
        mounted
            .bindings
            .iter()
            .map(|binding| binding.id.clone())
            .collect()
    }

    fn seed_admitted_binding(
        resolver: &HostIoResolver,
        request: &mut NodeExecutionRequest,
        binding: PluginBinding,
    ) {
        let mut record = resolver
            .run_store
            .load(&request.key.run_id)
            .unwrap()
            .unwrap();
        record
            .plugin_bindings
            .insert(binding.id.clone(), binding.clone());
        resolver.run_store.save(&record).unwrap();
        request.plugins = vec![binding];
    }

    #[tokio::test]
    async fn managed_python_is_shared_by_agent_commands_and_op_without_dependency_plugin() {
        let _environment = crate::PROCESS_ENV_LOCK.lock().await;
        struct Clear(Vec<(&'static str, Option<std::ffi::OsString>)>);
        impl Drop for Clear {
            fn drop(&mut self) {
                for (name, value) in self.0.drain(..).rev() {
                    unsafe {
                        match value {
                            Some(value) => std::env::set_var(name, value),
                            None => std::env::remove_var(name),
                        }
                    }
                }
            }
        }
        let names = [
            "ANCHOR_DEPENDENCIES_PYTHON",
            "ANCHOR_DEPENDENCIES_UV",
            "ANCHOR_DEPENDENCIES_UV_SHA256",
            "ANCHOR_DEPENDENCIES_SOURCES",
            "ANCHOR_DEPENDENCIES_BUILD_ROOT",
            "ANCHOR_PILOT_PYTHON",
            "ANCHOR_PILOT_PYTHON_ENABLED",
            "ANCHOR_PILOT_RUSTUP",
            "ANCHOR_PILOT_RUSTUP_HOME",
            "ANCHOR_PILOT_CARGO_HOME",
        ];
        let _cleared = Clear(
            names
                .iter()
                .map(|name| (*name, std::env::var_os(name)))
                .collect(),
        );
        for name in names {
            unsafe { std::env::remove_var(name) }
        }
        let (_root, resolver, _mounts, mut request) = session_fixture(&[], &[], &[]);
        let state = resolver.local_inputs.state_root();
        let managed = state.join("managed");
        let python = managed.join("python/runtime/bin/python3");
        let source = [
            "/usr/bin/python3.12",
            "/usr/bin/python3.11",
            "/usr/bin/python3.10",
            "/usr/bin/python3.9",
            "/usr/bin/python3.8",
        ]
        .into_iter()
        .find(|path| std::path::Path::new(path).is_file())
        .expect("managed runtime fixture needs supported system Python");
        crate::goose_acp::pilot::directory(python.parent().unwrap()).unwrap();
        std::fs::copy(source, &python).unwrap();
        let config = managed.join("runtime-config.json");
        std::fs::write(
            &config,
            json!({"format":1,"python":python,"python_enabled":true,"sources":null,"rust":null})
                .to_string(),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(config, std::fs::Permissions::from_mode(0o600)).unwrap();
        let configuration = crate::runtime_configuration::RuntimeConfiguration::from_env(
            state,
            resolver.plugin_library_root().unwrap(),
            "bwrap".into(),
        )
        .unwrap();
        let resolver =
            resolver.with_runtime_configuration(Some(std::sync::Arc::new(configuration)));
        let tools = resolver.tools(&request).await.unwrap();
        let result=tools.tools.call("anchor_run",json!({"command":["python","-c","import sys; print(sys.executable); print('managed-agent-marker')"]})).await.unwrap();
        let text = serde_json::to_string(&result).unwrap();
        assert!(text.contains("managed-agent-marker"), "{text}");
        assert!(text.contains(python.to_str().unwrap()), "{text}");
        drop(tools);
        let nodes = HostNodes {
            control: crate::HostControl {
                cancellation: request.cancellation.clone(),
                pause: std::sync::Arc::new(AtomicBool::new(false)),
            },
            sandbox: resolver.sandbox.clone(),
            allowed_commands: vec!["sh".into()],
            artifacts: resolver.artifacts.clone(),
            facts_root: resolver.local_inputs.state_root().join("node-facts"),
            io_resolver: std::sync::Arc::new(resolver),
            goose_nodes: None,
        };
        request.kind = NodeKind::OpRun;
        request.operation = Some(json!(
            "python -c 'import sys; print(sys.executable); print(\"managed-op-marker\")'"
        ));
        request.input = json!({"input":{}});
        request.key.invocation = 2;
        let result = nodes.execute(request).await.unwrap();
        assert!(
            matches!(result,NodeExecutionOutcome::Completed(ref completion) if completion.output["stdout"].as_str().is_some_and(|stdout|stdout.contains("managed-op-marker") && stdout.contains(python.to_str().unwrap()))),
            "{result:?}"
        );
    }

    #[tokio::test]
    async fn an_installed_plugin_code_and_resource_update_is_used_by_the_next_call() {
        let (_root, resolver, _mounts, mut request) =
            session_fixture(&["review"], &[("review", true, None)], &[]);
        let library = resolver.plugin_library_root().unwrap();
        let catalog = anchor_graph_host::FilePluginCatalog::new(library);
        let admitted = catalog.resolve(&["review".into()]).unwrap().remove(0);
        seed_admitted_binding(&resolver, &mut request, admitted.clone());

        let plugin = library.join("plugins/review");
        std::fs::create_dir_all(plugin.join("skills/new")).unwrap();
        std::fs::write(plugin.join("skills/new/SKILL.md"), "current Skill").unwrap();
        let current = catalog.resolve(&["review".into()]).unwrap().remove(0);
        assert_ne!(current.digest, admitted.digest);
        assert_ne!(current.resources, admitted.resources);
        assert_eq!(current.authorization_digest, admitted.authorization_digest);

        let mounted = resolver.mounted_plugins(&request).unwrap();
        assert_eq!(mounted.bindings, vec![current.clone()]);
        let resolved = resolver.tools(&request).await.unwrap();
        assert_eq!(resolved.plugins, vec![current.clone()]);
        assert_eq!(
            resolver.observed_plugin_digests(&request.key),
            BTreeMap::from([("review".to_owned(), current.digest)])
        );
    }

    #[tokio::test]
    async fn node_tool_selection_waits_for_real_plugin_publication_and_returns_current_skills() {
        let (root, resolver, _mounts, request) =
            session_fixture(&["review"], &[("review", true, None)], &[]);
        let library_root = resolver.plugin_library_root().unwrap();
        let source = root.path().join("updated-plugin");
        std::fs::create_dir_all(source.join("skills/current")).unwrap();
        std::fs::write(
            source.join("plugin.json"),
            r#"{"name":"review","skills":"skills/"}"#,
        )
        .unwrap();
        std::fs::write(source.join("skills/current/SKILL.md"), "current generation").unwrap();
        let library = anchor_library::Library::new(library_root);
        let lease = library.install_lease().unwrap();
        let mut resolving = resolver.tools(&request);
        assert!(
            tokio::time::timeout(Duration::from_millis(40), &mut resolving)
                .await
                .is_err()
        );
        library
            .install_directory_locked("review", &source, true, &lease)
            .unwrap();
        let current = anchor_graph_host::FilePluginCatalog::new(library_root)
            .definition("review")
            .unwrap()
            .binding();
        drop(lease);
        let resolved = tokio::time::timeout(Duration::from_secs(5), resolving)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(resolved.plugins.as_slice(), std::slice::from_ref(&current));
        assert!(
            resolved.plugins[0]
                .resources
                .contains(&"skills/current/SKILL.md".to_owned())
        );
        assert_eq!(
            resolver.observed_plugin_digests(&request.key),
            BTreeMap::from([("review".to_owned(), current.digest)])
        );
    }

    #[test]
    fn a_plugin_authorization_change_still_fails_closed() {
        let (_root, resolver, _mounts, mut request) =
            session_fixture(&["review"], &[("review", true, None)], &[]);
        let library = resolver.plugin_library_root().unwrap();
        let admitted = anchor_graph_host::FilePluginCatalog::new(library)
            .resolve(&["review".into()])
            .unwrap()
            .remove(0);
        seed_admitted_binding(&resolver, &mut request, admitted);
        std::fs::write(
            library.join("plugins/review/plugin.json"),
            r#"{"name":"review","network":true}"#,
        )
        .unwrap();

        let error = resolver.mounted_plugins(&request).unwrap_err();
        assert!(error.contains("Host authorization changed"), "{error}");
    }

    #[test]
    fn a_session_mounted_plugin_the_graph_never_declared_is_mounted() {
        let (_root, resolver, _mounts, request) =
            session_fixture(&["side"], &[], &[("side", true)]);
        let mounted = resolver.mounted_plugins(&request).unwrap();
        assert_eq!(mounted_ids(&mounted), ["side"]);
        // The human's click authorized it, not the Graph: the binding came from
        // the Library, with the identity the catalog resolved.
        assert!(!mounted.bindings[0].digest.is_empty());
        assert_eq!(mounted.session.as_deref(), Some("session-one"));
        assert!(mounted.snapshot, "the Turn's switches are pinned");
    }

    #[tokio::test]
    async fn a_session_skill_returns_its_current_resources_with_the_frozen_tools() {
        let (_root, resolver, _mounts, request) =
            session_fixture(&["side"], &[], &[("side", true)]);
        let plugin = resolver.plugin_library_root().unwrap().join("plugins/side");
        std::fs::create_dir_all(plugin.join("skills/check")).unwrap();
        std::fs::write(
            plugin.join("skills/check/SKILL.md"),
            "read the current Skill",
        )
        .unwrap();
        assert!(request.plugins.is_empty());
        let resolved = resolver.tools(&request).await.unwrap();
        assert_eq!(resolved.plugins.len(), 1);
        assert_eq!(resolved.plugins[0].id, "side");
        assert!(
            resolved.plugins[0]
                .resources
                .contains(&"skills/check/SKILL.md".to_owned())
        );
        assert_eq!(
            resolver.observed_plugin_digests(&request.key),
            BTreeMap::from([("side".to_owned(), resolved.plugins[0].digest.clone())])
        );
    }

    #[test]
    fn a_session_switched_off_plugin_is_not_mounted() {
        let (_root, resolver, _mounts, request) = session_fixture(
            &["side", "kept"],
            &[("side", false, None), ("kept", true, None)],
            &[("side", false)],
        );
        let mounted = resolver.mounted_plugins(&request).unwrap();
        // An optional Plugin the Session switched off stays off; a required one
        // is mounted regardless of the switch.
        assert_eq!(mounted_ids(&mounted), ["kept"]);
        assert!(
            mounted
                .reasons
                .iter()
                .any(|reason| reason.contains("`side`") && reason.contains("switched off")),
            "{:?}",
            mounted.reasons
        );
    }

    /// An optional Plugin follows "the Session's explicit value, else its
    /// `default`, else on" -- and a required one ignores the Session entirely.
    #[test]
    fn an_optional_default_decides_until_the_session_says_otherwise() {
        // `default: true` and no Session value: mounted.
        let (_root, resolver, _mounts, request) =
            session_fixture(&["opt-on"], &[("opt-on", false, Some(true))], &[]);
        assert_eq!(
            mounted_ids(&resolver.mounted_plugins(&request).unwrap()),
            ["opt-on"]
        );

        // `default: false` and no Session value: not mounted.
        let (_root, resolver, _mounts, request) =
            session_fixture(&["opt-off"], &[("opt-off", false, Some(false))], &[]);
        let mounted = resolver.mounted_plugins(&request).unwrap();
        assert!(mounted_ids(&mounted).is_empty(), "{:?}", mounted.reasons);
        assert!(
            mounted
                .reasons
                .iter()
                .any(|reason| reason.contains("opt-off")),
            "{:?}",
            mounted.reasons
        );

        // The Session's explicit value beats a `default: false`.
        let (_root, resolver, _mounts, request) = session_fixture(
            &["opt-off"],
            &[("opt-off", false, Some(false))],
            &[("opt-off", true)],
        );
        assert_eq!(
            mounted_ids(&resolver.mounted_plugins(&request).unwrap()),
            ["opt-off"]
        );

        // ...and `default: true` does not survive an explicit off.
        let (_root, resolver, _mounts, request) = session_fixture(
            &["opt-on"],
            &[("opt-on", false, Some(true))],
            &[("opt-on", false)],
        );
        let mounted = resolver.mounted_plugins(&request).unwrap();
        assert!(mounted_ids(&mounted).is_empty(), "{:?}", mounted.reasons);
        assert!(
            mounted
                .reasons
                .iter()
                .any(|reason| reason.contains("switched off")),
            "{:?}",
            mounted.reasons
        );
    }

    /// Run one body with `ANCHOR_RUNNER_GRAPH_NAME` set: the configured pilot Graph
    /// is the only Graph whose `required: true` is an authorization.
    fn with_configured_pilot_graph<T>(graph: Option<&str>, body: impl FnOnce() -> T) -> T {
        let _guard = crate::PROCESS_ENV_LOCK.blocking_lock();
        let previous = std::env::var_os("ANCHOR_RUNNER_GRAPH_NAME");
        unsafe {
            match graph {
                Some(graph) => std::env::set_var("ANCHOR_RUNNER_GRAPH_NAME", graph),
                None => std::env::remove_var("ANCHOR_RUNNER_GRAPH_NAME"),
            }
        }
        let result = body();
        unsafe {
            match previous {
                Some(value) => std::env::set_var("ANCHOR_RUNNER_GRAPH_NAME", value),
                None => std::env::remove_var("ANCHOR_RUNNER_GRAPH_NAME"),
            }
        }
        result
    }

    #[test]
    fn a_required_plugin_from_the_pilot_graph_cannot_be_switched_off() {
        with_configured_pilot_graph(Some("session-mounts"), || {
            let (_root, resolver, _mounts, request) =
                session_fixture(&["must"], &[("must", true, None)], &[("must", false)]);
            let mounted = resolver.mounted_plugins(&request).unwrap();
            assert_eq!(mounted_ids(&mounted), ["must"]);
            assert!(
                !mounted
                    .reasons
                    .iter()
                    .any(|reason| reason.contains("`must`")),
                "a mounted Plugin needs no reason: {:?}",
                mounted.reasons
            );
        });
    }

    /// The assistant can create or edit a Graph in this Session, so a
    /// `required: true` in *another* Graph must not be an authorization: it is a
    /// default, and the Session's switch still wins.
    #[test]
    fn a_required_declaration_from_another_graph_is_only_a_default() {
        with_configured_pilot_graph(Some("the-pilot-graph"), || {
            let (_root, resolver, _mounts, request) =
                session_fixture(&["must"], &[("must", true, None)], &[("must", false)]);
            let mounted = resolver.mounted_plugins(&request).unwrap();
            assert!(
                mounted_ids(&mounted).is_empty(),
                "another Graph's required must not defeat the Session: {:?}",
                mounted.reasons
            );
            assert!(
                mounted
                    .reasons
                    .iter()
                    .any(|reason| reason.contains("treated as optional")),
                "{:?}",
                mounted.reasons
            );
            assert!(
                mounted
                    .reasons
                    .iter()
                    .any(|reason| reason.contains("switched off")),
                "{:?}",
                mounted.reasons
            );
        });
    }

    #[tokio::test]
    async fn an_uninstalled_plugin_is_not_mounted_and_says_so() {
        let (_root, resolver, _mounts, request) =
            session_fixture(&["installed"], &[], &[("ghost", true), ("installed", true)]);
        let mounted = resolver.mounted_plugins(&request).unwrap();
        assert_eq!(mounted_ids(&mounted), ["installed"]);
        assert!(
            mounted
                .reasons
                .iter()
                .any(|reason| reason.contains("`ghost`")
                    && reason.contains("is not available in this Library")),
            "{:?}",
            mounted.reasons
        );
        // The same reason reaches the Run's own record of what this call observed.
        resolver.tools(&request).await.unwrap();
        let observed = resolver.observed_plugin_views(&request.key);
        assert!(
            observed.get(MOUNT_EVIDENCE_KEY).is_some_and(|value| {
                value.contains("this Turn's snapshot")
                    && value.contains("`ghost` is not available in this Library")
                    // A Plugin mounted by a click is named as such, so "why is this
                    // mounted" is answerable without reading the absence of reasons.
                    && value.contains("by-switch=[installed]")
            }),
            "{observed:?}"
        );
    }

    #[test]
    fn a_turn_keeps_the_first_snapshot_when_the_session_switch_changes() {
        let (_root, resolver, mounts, request) = session_fixture(&["side"], &[], &[("side", true)]);
        assert_eq!(
            mounted_ids(&resolver.mounted_plugins(&request).unwrap()),
            ["side"]
        );
        mounts.set("side", false);
        // The Turn already started: the tool set must not change under it.
        assert_eq!(
            mounted_ids(&resolver.mounted_plugins(&request).unwrap()),
            ["side"]
        );
        let path = plugin_mounts::snapshot_path(
            resolver.local_inputs.state_root(),
            "session-one",
            "turn-one",
        );
        assert_eq!(
            plugin_mounts::read_snapshot(&path, "session-one", "turn-one")
                .unwrap()
                .mounts,
            BTreeMap::from([("side".to_owned(), true)])
        );
    }

    #[tokio::test]
    async fn resolver_returns_owned_anchor_tools_under_admitted_plugin_authority() {
        let root = tempfile::tempdir().unwrap();
        let workspace_root = root.path().join("workspaces");
        let artifacts_root = root.path().join("artifacts");
        std::fs::create_dir_all(&workspace_root).unwrap();
        std::fs::create_dir_all(&artifacts_root).unwrap();
        let sandbox = std::sync::Arc::new(
            BubblewrapSandbox::new(
                BubblewrapPolicy::new("bwrap", ["sh"])
                    .authorize_workspace_root(&workspace_root)
                    .authorize_readonly_input_root(&artifacts_root)
                    .authorize_readonly_destination_root("/in"),
            )
            .unwrap(),
        );
        let tools = tool_host::PluginToolHost::new(["fake-tools".to_owned()]);
        let bindings = tools
            .resolve_fixture_plugins(&["fake-tools".into()])
            .unwrap();
        let store = FileRunStore::new(root.path().join("runs"));
        let snapshot = anchor_runtime::graph::GraphSnapshot::admit(json!({
            "objective":"resolver test","entry":"agent","agents":{"worker":{"model":"fixture"}},
            "ops":{},"nodes":[{"id":"agent","agent":"worker","plugins":["fake-tools"]}],"edges":[]
        }))
        .unwrap();
        let mut admitted = GraphRunRecord::create(snapshot, serde_json::Value::Null).unwrap();
        admitted.plugin_bindings = bindings
            .iter()
            .cloned()
            .map(|b| (b.id.clone(), b))
            .collect();
        admitted.plugin_bindings_initialized = true;
        store.save(&admitted).unwrap();
        let resolver = HostIoResolver::new(
            HostArtifacts::new(artifacts_root, workspace_root),
            sandbox,
            std::sync::Arc::new(tools),
            tool_host::McpToolConfig::default(),
            store,
            LocalInputs::new(root.path().to_path_buf(), None).unwrap(),
            fixed(&[]),
        )
        .with_library_root(Some(root.path().to_path_buf()));

        assert_eq!(
            resolver.resolve_plugins(&["fake-tools".into()]).unwrap(),
            bindings
        );
        assert!(resolver.resolve_plugins(&["missing".into()]).is_err());

        let request = NodeExecutionRequest {
            key: InvocationKey {
                run_id: admitted.run_id,
                graph_digest: admitted.graph_digest,
                node_id: "agent".into(),
                invocation: 1,
            },
            model: Some("fixture".into()),
            task: "inspect inputs".into(),
            instructions: "use Anchor tools".into(),
            routes: Vec::new(),
            input: serde_json::Value::Null,
            input_commits: Vec::new(),
            plugins: bindings,
            max_provider_requests: None,
            wall_time_limit_seconds: Some(30.0),
            network: false,
            kind: NodeKind::Agent,
            operation: None,
            cancellation: std::sync::Arc::new(AtomicBool::new(false)),
        };
        let workspace = resolver.workspace(&request).unwrap();
        let tools = resolver.tools(&request).await.unwrap().tools;
        let names = tools
            .definitions()
            .into_iter()
            .map(|definition| definition.name.to_string())
            .collect::<Vec<_>>();
        assert!(names.iter().any(|name| name == tool_host::FAKE_ECHO_TOOL));
        assert!(
            names
                .iter()
                .any(|name| name == crate::node_tools::RUN_TOOL_NAME)
        );
        assert!(workspace.is_dir());

        let mut record = resolver
            .run_store
            .load(&request.key.run_id)
            .unwrap()
            .unwrap();
        record
            .plugin_bindings
            .get_mut("fake-tools")
            .unwrap()
            .authorization_digest = "sha256:drift".into();
        resolver.run_store.save(&record).unwrap();
        assert!(resolver.tools(&request).await.is_err());
    }

    #[tokio::test]
    async fn agent_tools_mount_plugins_without_a_run_policy_grant() {
        let root = tempfile::tempdir().unwrap();
        let state = root.path().join("state");
        let workspace = root.path().join("workspaces");
        let artifacts = root.path().join("artifacts");
        for path in [&workspace, &artifacts] {
            std::fs::create_dir_all(path).unwrap();
        }
        // A Library Plugin with one real resource file.
        let library = root.path().join("library");
        let plugin = library.join("plugins/demo");
        std::fs::create_dir_all(plugin.join("resources")).unwrap();
        std::fs::write(
            plugin.join("plugin.json"),
            json!({"name":"Demo"}).to_string(),
        )
        .unwrap();
        std::fs::write(plugin.join("resources/note.txt"), "plugin-resource").unwrap();
        let bindings = anchor_graph_host::PluginCatalog::resolve(
            &anchor_graph_host::FilePluginCatalog::new(&library),
            &["demo".into()],
        )
        .unwrap();
        assert_eq!(
            bindings[0].resources,
            vec!["plugin.json", "resources/note.txt"]
        );

        let snapshot = GraphSnapshot::admit(json!({
            "objective":"plugin mount", "entry":"agent",
            "agents":{"worker":{"model":"fixture"}},
            "nodes":[{"id":"agent","agent":"worker","plugins":["demo"]}],
            "edges":[]
        }))
        .unwrap();
        let mut record =
            GraphRunRecord::create_with_id(snapshot, json!({}), "agent-plugin-mount").unwrap();
        record.plugin_bindings = bindings
            .iter()
            .cloned()
            .map(|binding| (binding.id.clone(), binding))
            .collect();
        record.plugin_bindings_initialized = true;
        let store = FileRunStore::new(state.join("runs"));
        store.save(&record).unwrap();
        crate::application::metadata::save(
            &state,
            &crate::application::RunMetadata::new(
                record.run_id.clone(),
                "mount".into(),
                record.graph_digest.clone(),
                &library,
            )
            .unwrap(),
        )
        .unwrap();

        // The Run policy deliberately does not authorize the Library at all: the
        // invocation grants the Plugin mount it actually made. A Run cannot know
        // its children's Plugins in advance.
        let sandbox = std::sync::Arc::new(
            BubblewrapSandbox::new(
                BubblewrapPolicy::new("bwrap", ["sh"]).authorize_workspace_root(&workspace),
            )
            .unwrap(),
        );
        let resolver = HostIoResolver::new(
            HostArtifacts::new(artifacts, workspace),
            sandbox,
            std::sync::Arc::new(tool_host::PluginToolHost::new(Vec::<String>::new())),
            tool_host::McpToolConfig::default(),
            store,
            LocalInputs::new(state.clone(), None).unwrap(),
            fixed(&[]),
        )
        .with_library_root(Some(library));
        let request = NodeExecutionRequest {
            key: InvocationKey {
                run_id: record.run_id,
                graph_digest: record.graph_digest,
                node_id: "agent".into(),
                invocation: 1,
            },
            model: Some("fixture".into()),
            task: "read the plugin resource".into(),
            instructions: String::new(),
            routes: Vec::new(),
            input: json!({}),
            input_commits: Vec::new(),
            plugins: record.plugin_bindings.values().cloned().collect::<Vec<_>>(),
            max_provider_requests: None,
            wall_time_limit_seconds: Some(30.0),
            network: false,
            kind: NodeKind::Agent,
            operation: None,
            cancellation: std::sync::Arc::new(AtomicBool::new(false)),
        };
        let tools = resolver.tools(&request).await.unwrap().tools;
        let result = tools
            .call(
                crate::node_tools::RUN_TOOL_NAME,
                json!({"command":["sh","-c","cat /plugins/demo/resources/note.txt"]}),
            )
            .await
            .unwrap();
        assert_eq!(result[0].as_json().unwrap()["exit_code"], 0);
        assert_eq!(result[0].as_json().unwrap()["stdout"], "plugin-resource");
    }

    #[tokio::test]
    async fn agent_command_tools_receive_only_the_nodes_frozen_local_inputs() {
        let root = tempfile::tempdir().unwrap();
        let state = root.path().join("state");
        let config = root.path().join("operator-workspaces");
        let workspace = root.path().join("workspaces");
        let artifacts = root.path().join("artifacts");
        let private = root.path().join("private-inputs");
        for path in [&workspace, &artifacts, &private, &config.join("weekly")] {
            std::fs::create_dir_all(path).unwrap();
        }
        std::fs::write(private.join("history.txt"), "agent-local-evidence").unwrap();
        std::fs::write(
            config.join("weekly/local-inputs.json"),
            json!({"agent":{"history":private}}).to_string(),
        )
        .unwrap();
        let snapshot = GraphSnapshot::admit(json!({
            "objective":"agent local inputs", "entry":"agent", "agents":{"worker":{"model":"fixture"}},
            "nodes":[{"id":"agent","agent":"worker"},{"id":"other","agent":"worker"}],
            "edges":[{"from":"agent","to":"other"}]
        })).unwrap();
        let mut record =
            GraphRunRecord::create_with_id(snapshot, json!({}), "agent-local-inputs").unwrap();
        record.plugin_bindings_initialized = true;
        let local = LocalInputs::new(state.clone(), Some(config.clone())).unwrap();
        local
            .freeze(
                &record.run_id,
                &record.graph_digest,
                &record.snapshot,
                Some("weekly"),
                true,
            )
            .unwrap();
        let store = FileRunStore::new(state.join("runs"));
        store.save(&record).unwrap();
        crate::application::metadata::save(
            &state,
            &crate::application::RunMetadata::new(
                record.run_id.clone(),
                "weekly".into(),
                record.graph_digest.clone(),
                root.path(),
            )
            .unwrap(),
        )
        .unwrap();
        let sandbox = std::sync::Arc::new(
            BubblewrapSandbox::new(
                BubblewrapPolicy::new("bwrap", ["sh"])
                    .authorize_workspace_root(&workspace)
                    .authorize_readonly_input_root(&artifacts)
                    .authorize_readonly_destination_root("/in"),
            )
            .unwrap(),
        );
        let resolver = HostIoResolver::new(
            HostArtifacts::new(artifacts, workspace),
            sandbox,
            std::sync::Arc::new(tool_host::PluginToolHost::new(Vec::<String>::new())),
            tool_host::McpToolConfig::default(),
            store,
            local,
            fixed(&[]),
        );
        let mut request = NodeExecutionRequest {
            key: InvocationKey {
                run_id: record.run_id,
                graph_digest: record.graph_digest,
                node_id: "agent".into(),
                invocation: 1,
            },
            model: Some("fixture".into()),
            task: "read evidence".into(),
            instructions: String::new(),
            routes: Vec::new(),
            input: json!({}),
            input_commits: Vec::new(),
            plugins: Vec::new(),
            max_provider_requests: None,
            wall_time_limit_seconds: Some(30.0),
            network: false,
            kind: NodeKind::Agent,
            operation: None,
            cancellation: std::sync::Arc::new(AtomicBool::new(false)),
        };
        let tools = resolver.tools(&request).await.unwrap().tools;
        let result = tools.call(crate::node_tools::RUN_TOOL_NAME, json!({"command":["sh","-c","cat /local-inputs/history/history.txt; ! touch /local-inputs/history/forbidden"]})).await.unwrap();
        assert_eq!(result[0].as_json().unwrap()["exit_code"], 0);
        assert_eq!(
            result[0].as_json().unwrap()["stdout"],
            "agent-local-evidence"
        );
        assert!(!private.join("forbidden").exists());
        request.key.node_id = "other".into();
        let tools = resolver.tools(&request).await.unwrap().tools;
        let result = tools
            .call(
                crate::node_tools::RUN_TOOL_NAME,
                json!({"command":["sh","-c","test ! -e /local-inputs/history/history.txt"]}),
            )
            .await
            .unwrap();
        assert_eq!(result[0].as_json().unwrap()["exit_code"], 0);
        std::fs::write(
            config.join("weekly/local-inputs.json"),
            json!({"other":{"history":private}}).to_string(),
        )
        .unwrap();
        assert!(resolver.tools(&request).await.is_err());
    }
}
