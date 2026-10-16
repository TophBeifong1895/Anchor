//! Recursive Graph Call admission, execution and result projection over shared Kernel ports.

use crate::graph_catalog::{ChildRunControl, GraphCatalog};
use crate::plugin_catalog::PluginCatalog;
use anchor_runtime::Cancellation;
use anchor_runtime::graph::{
    ArtifactPort, CallFileSelection, CallIdentity, CommitRef, GraphCallOutcome, GraphCallPort,
    GraphError, GraphRunRecord, GraphRunner, GraphSnapshot, InvocationKey,
    NodeExecutionCapabilities, NodeExecutionOutcome, NodeExecutionPort, NodeExecutionRequest,
    RunControl, RunStatus, RunStore,
};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, future::Future, pin::Pin};

/// Adapts GraphCallPort on top of shared kernel ports. Instantiate one host
/// around the same store/artifact/node/control ports used by the outer Runner.
pub struct InProcessGraphHost<'a, S, A, N, C> {
    catalog: &'a dyn GraphCatalog,
    store: &'a S,
    artifacts: &'a A,
    nodes: &'a N,
    plugin_catalog: Option<&'a dyn PluginCatalog>,
    _control: std::marker::PhantomData<fn() -> C>,
}

struct BoundNodes<'a, N> {
    nodes: &'a N,
    plugin_catalog: Option<&'a dyn PluginCatalog>,
    graph_calls: &'a dyn GraphCallPort,
}

impl<N: NodeExecutionPort> NodeExecutionPort for BoundNodes<'_, N> {
    fn capabilities(&self) -> NodeExecutionCapabilities {
        self.nodes.capabilities()
    }
    fn resolve_plugins(
        &self,
        ids: &[String],
    ) -> Result<Vec<anchor_runtime::graph::PluginBinding>, GraphError> {
        match self.plugin_catalog {
            Some(catalog) => catalog.resolve(ids),
            None => self.nodes.resolve_plugins(ids),
        }
    }
    fn observed_plugin_views(
        &self,
        key: &anchor_runtime::graph::InvocationKey,
    ) -> std::collections::BTreeMap<String, String> {
        self.nodes.observed_plugin_views(key)
    }
    fn observed_plugin_digests(
        &self,
        key: &anchor_runtime::graph::InvocationKey,
    ) -> std::collections::BTreeMap<String, String> {
        self.nodes.observed_plugin_digests(key)
    }
    fn completion_fact<'b>(
        &'b self,
        key: &'b anchor_runtime::graph::InvocationKey,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<anchor_runtime::graph::CompletionFact, GraphError>>
                + Send
                + 'b,
        >,
    > {
        self.nodes.completion_fact(key)
    }
    fn execute<'b>(
        &'b self,
        request: NodeExecutionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<NodeExecutionOutcome, GraphError>> + Send + 'b>> {
        self.nodes.execute(request)
    }
    fn graph_call_port(&self) -> Option<&dyn GraphCallPort> {
        Some(self.graph_calls)
    }
}

struct ChildControl {
    control: ChildRunControl,
}

impl ChildControl {
    fn new(control: ChildRunControl) -> Self {
        Self { control }
    }
}

impl RunControl for ChildControl {
    fn pause_requested(&self) -> bool {
        self.control
            .pause
            .load(std::sync::atomic::Ordering::Acquire)
    }
    fn stop_requested(&self) -> bool {
        self.control
            .cancellation
            .load(std::sync::atomic::Ordering::Acquire)
    }
    fn cancellation(&self) -> Cancellation {
        self.control.cancellation.clone()
    }
}

impl<'a, S, A, N, C> InProcessGraphHost<'a, S, A, N, C> {
    pub fn new(
        catalog: &'a dyn GraphCatalog,
        store: &'a S,
        artifacts: &'a A,
        nodes: &'a N,
        _control: &'a C,
    ) -> Self {
        Self {
            catalog,
            store,
            artifacts,
            nodes,
            plugin_catalog: None,
            _control: std::marker::PhantomData,
        }
    }

    /// Configure the filesystem (or other) catalog used to pin Plugin identities.
    /// The current bindings of the Plugins one bundle references.
    ///
    /// A bundle carries ids only, so this is the one resolution path: the Plugin
    /// catalog the host configured (the Library's `plugins/<id>`).
    fn resolve_bundle_plugins(
        &self,
        ids: &[String],
    ) -> Result<Vec<anchor_runtime::graph::PluginBinding>, GraphError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let catalog = self.plugin_catalog.ok_or_else(|| {
            GraphError::Unsupported(
                "a called Graph references Plugins, and no Plugin catalog is configured for child Runs"
                    .to_owned(),
            )
        })?;
        catalog.resolve(ids)
    }

    pub fn with_plugin_catalog(mut self, catalog: &'a dyn PluginCatalog) -> Self {
        self.plugin_catalog = Some(catalog);
        self
    }
}

impl<S: RunStore, A: ArtifactPort, N: NodeExecutionPort, C: RunControl> NodeExecutionPort
    for InProcessGraphHost<'_, S, A, N, C>
{
    fn capabilities(&self) -> NodeExecutionCapabilities {
        self.nodes.capabilities()
    }
    fn resolve_plugins(
        &self,
        ids: &[String],
    ) -> Result<Vec<anchor_runtime::graph::PluginBinding>, GraphError> {
        match self.plugin_catalog {
            Some(catalog) => catalog.resolve(ids),
            None => self.nodes.resolve_plugins(ids),
        }
    }
    fn observed_plugin_views(
        &self,
        key: &anchor_runtime::graph::InvocationKey,
    ) -> std::collections::BTreeMap<String, String> {
        self.nodes.observed_plugin_views(key)
    }
    fn observed_plugin_digests(
        &self,
        key: &anchor_runtime::graph::InvocationKey,
    ) -> std::collections::BTreeMap<String, String> {
        self.nodes.observed_plugin_digests(key)
    }
    fn completion_fact<'b>(
        &'b self,
        key: &'b anchor_runtime::graph::InvocationKey,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<anchor_runtime::graph::CompletionFact, GraphError>>
                + Send
                + 'b,
        >,
    > {
        self.nodes.completion_fact(key)
    }
    fn execute<'b>(
        &'b self,
        request: NodeExecutionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<NodeExecutionOutcome, GraphError>> + Send + 'b>> {
        self.nodes.execute(request)
    }
    fn graph_call_port(&self) -> Option<&dyn GraphCallPort> {
        Some(self)
    }
}

impl<S: RunStore, A: ArtifactPort, N: NodeExecutionPort, C: RunControl> GraphCallPort
    for InProcessGraphHost<'_, S, A, N, C>
{
    fn call<'b>(
        &'b self,
        identity: &'b CallIdentity,
        spec: &'b Value,
        input: &'b Value,
        input_commits: &'b [CommitRef],
        cancellation: Cancellation,
    ) -> Pin<Box<dyn Future<Output = Result<GraphCallOutcome, GraphError>> + Send + 'b>> {
        Box::pin(async move {
            let PreparedCall {
                call_spec,
                session_context,
                child_input,
            } = self.prepare_call(identity, spec, input).await?;
            let graph_name = call_spec.graph.as_str();
            let mode = call_spec.mode.as_str();
            let file_selections = call_spec.file_selections();
            let run_id = child_run_id(identity);
            let child_lease = self
                .store
                .acquire_lease(&run_id)
                .map_err(|e| GraphError::Unsupported(format!("acquire child lease: {e}")))?;
            let existing = self
                .store
                .load(&run_id)
                .map_err(|e| GraphError::Unsupported(format!("load child Run: {e}")))?;
            let mut child = if let Some(existing) = existing {
                self.verify_existing_child(&existing, &run_id, identity, &call_spec, &child_input)?;
                self.artifacts
                    .stage_call_inputs(&run_id, input_commits, &file_selections)
                    .await?;
                existing
            } else {
                // Serialize only the short child admission against mutations to
                // the target Graph. Existing children never consult this catalog
                // definition unless their pinned Plugin resources need checking.
                let _admission_lease = self.catalog.lock_admission(graph_name)?;
                // Recheck after taking the target lock in case another call won
                // the same identity between the first read and admission.
                if let Some(existing) = self.store.load(&run_id)? {
                    self.verify_existing_child(
                        &existing,
                        &run_id,
                        identity,
                        &call_spec,
                        &child_input,
                    )?;
                    self.artifacts
                        .stage_call_inputs(&run_id, input_commits, &file_selections)
                        .await?;
                    existing
                } else {
                    if self.catalog.has_child_admission(&run_id)? {
                        return Err(GraphError::CorruptRun(
                            "child admission metadata exists but its durable Run record is missing"
                                .into(),
                        ));
                    }
                    let bundle = self
                        .catalog
                        .bundle(graph_name)
                        .map_err(|e| GraphError::Unsupported(format!("load child bundle: {e}")))?
                        .ok_or_else(|| {
                            GraphError::InvalidSnapshot(format!(
                                "unknown child graph `{graph_name}`"
                            ))
                        })?;
                    let snapshot = bundle.snapshot;
                    let _plugin_lease = if bundle.plugins.is_empty() {
                        None
                    } else {
                        Some(
                            self.catalog
                                .lock_plugin_references(cancellation.clone())
                                .await?,
                        )
                    };
                    // A bundle references Plugins by id; the current code and view
                    // live in the Library, which is the only place this layer
                    // resolves them from.
                    let referenced = self.resolve_bundle_plugins(&bundle.plugins)?;
                    // Stage the read-only `/in/call` bundle before the child
                    // becomes durable. A crash can therefore never leave a
                    // Ready child (including a startup-recovered detach child)
                    // without the inputs its frozen identity selected.
                    self.artifacts
                        .stage_call_inputs(&run_id, input_commits, &file_selections)
                        .await?;
                    let mut record = GraphRunRecord::create(snapshot.clone(), child_input)?;
                    record.run_id = run_id.clone();
                    record.plugin_bindings = referenced
                        .iter()
                        .cloned()
                        .map(|plugin| (plugin.id.clone(), plugin))
                        .collect();
                    record.plugin_bindings_initialized = true;
                    self.persist_child_admission(
                        &record,
                        identity,
                        graph_name,
                        mode,
                        session_context.as_ref(),
                    )?;
                    record
                }
            };
            drop(child_lease);
            if session_context.is_some() {
                if let Some(outcome) = self
                    .catalog
                    .session_call_outcome(&run_id, mode, cancellation.clone())
                    .await?
                {
                    return Ok(outcome);
                }
                child = self.store.load(&run_id)?.ok_or_else(|| {
                    GraphError::CorruptRun("completed Session child Run is missing".into())
                })?;
                if child.status != RunStatus::Completed {
                    return Err(GraphError::CorruptRun(
                        "Session host confirmed delivery without a completed child Run".into(),
                    ));
                }
            }
            // FileRunStore's record is the durable identity → child admission:
            // deterministic ID plus frozen snapshot/input are checked on retry.
            if mode == "detach" && session_context.is_none() {
                self.catalog.dispatch_detached(&run_id).await?;
                return Ok(GraphCallOutcome::Detached {
                    child_run_id: run_id,
                });
            }
            // A durable child that already reached a terminal or recovery state
            // is never re-entered: its own facts decide the parent outcome and
            // re-running it could replay an unknown side effect. Only the
            // non-terminal states are resumed along the same child Run.
            let resumable = session_context.is_none()
                && matches!(
                    child.status,
                    RunStatus::Ready
                        | RunStatus::Running
                        | RunStatus::WaitingCall
                        | RunStatus::Paused
                        | RunStatus::Stopped
                        | RunStatus::BudgetStopped
                );
            if resumable {
                if cancellation.load(std::sync::atomic::Ordering::Acquire) {
                    return Ok(GraphCallOutcome::Waiting {
                        child_run_id: run_id,
                    });
                }
                child = self
                    .execute_wait_child(child, &run_id, graph_name, cancellation)
                    .await?;
            }
            self.project_child_outcome(child, run_id, identity, &call_spec)
                .await
        })
    }
}

struct PreparedCall {
    call_spec: CallSpec,
    session_context: Option<Value>,
    child_input: Value,
}

impl<S: RunStore, A: ArtifactPort, N: NodeExecutionPort, C: RunControl>
    InProcessGraphHost<'_, S, A, N, C>
{
    async fn prepare_call(
        &self,
        identity: &CallIdentity,
        spec: &Value,
        input: &Value,
    ) -> Result<PreparedCall, GraphError> {
        let parent = self
            .store
            .load(&identity.parent_run_id)
            .map_err(|e| GraphError::Unsupported(format!("load parent Run for call: {e}")))?
            .ok_or_else(|| GraphError::CorruptRun("Graph call parent Run is missing".into()))?;
        let cursor = parent.cursor.as_ref().ok_or_else(|| {
            GraphError::CorruptRun("Graph call parent has no active cursor".into())
        })?;
        if parent.graph_digest != identity.parent_graph_digest
            || cursor.node_id != identity.node_id
            || cursor.key.invocation != identity.invocation
            || cursor.prepared_input != *input
        {
            return Err(GraphError::CorruptRun(
                "Graph call identity does not match durable parent cursor".into(),
            ));
        }
        let parent_node = parent
            .snapshot
            .nodes
            .iter()
            .find(|node| node.id == identity.node_id)
            .ok_or_else(|| {
                GraphError::CorruptRun("Graph call node is missing from parent snapshot".into())
            })?;
        let frozen_spec = parent_node
            .op
            .as_ref()
            .and_then(|op_name| parent.snapshot.ops.get(op_name))
            .and_then(|op| op.get("call"))
            .ok_or_else(|| GraphError::CorruptRun("parent cursor is not an Op.call node".into()))?;
        let spec_digest = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(frozen_spec).map_err(GraphError::SnapshotDecode)?)
        );
        if frozen_spec != spec || identity.call_spec_digest != spec_digest {
            return Err(GraphError::CorruptRun(
                "Graph call spec does not match frozen parent snapshot".into(),
            ));
        }
        // A typed, `deny_unknown_fields` decode rejects malformed
        // input/input_map/files/result and any stray field before the host
        // can create a child Run or run a child side effect.
        let call_spec: CallSpec = serde_json::from_value(spec.clone()).map_err(|error| {
            GraphError::InvalidSnapshot(format!("invalid op.call spec: {error}"))
        })?;
        if !matches!(call_spec.mode.as_str(), "wait" | "detach") {
            return Err(GraphError::InvalidSnapshot(
                "op.call.mode must be wait or detach".into(),
            ));
        }
        let graph_name = call_spec.graph.as_str();
        if self
            .catalog
            .graph_ancestry(&identity.parent_run_id)?
            .iter()
            .any(|ancestor| ancestor == graph_name)
        {
            return Err(GraphError::InvalidSnapshot(format!(
                "recursive Graph call to `{graph_name}` is not allowed"
            )));
        }
        let session_context = if let Some(session) = call_spec.session.as_deref() {
            let context = self
                .catalog
                .prepare_session_call(identity, session, graph_name)
                .await?;
            if context.get("session").and_then(Value::as_str) != Some(session)
                || !context.get("channel").is_some_and(Value::is_object)
            {
                return Err(GraphError::CorruptRun(
                    "session call context does not match the authorized Session".into(),
                ));
            }
            Some(context)
        } else {
            None
        };
        let mut child_input = call_spec.child_input(input)?;
        if let Some(context) = session_context.as_ref() {
            let values = child_input
                .as_object_mut()
                .expect("child_input returns an object");
            values.insert("session".into(), context["session"].clone());
            values.insert("channel".into(), context["channel"].clone());
        }
        Ok(PreparedCall {
            call_spec,
            session_context,
            child_input,
        })
    }

    fn verify_existing_child(
        &self,
        existing: &GraphRunRecord,
        run_id: &str,
        identity: &CallIdentity,
        call_spec: &CallSpec,
        child_input: &Value,
    ) -> Result<(), GraphError> {
        let graph_name = call_spec.graph.as_str();
        let mode = call_spec.mode.as_str();
        if existing.input != expected_input(&existing.snapshot, child_input) {
            return Err(GraphError::RunConflict);
        }
        self.catalog.verify_child_identity(
            run_id,
            graph_name,
            &existing.snapshot,
            identity,
            mode,
        )?;
        if !existing.plugin_bindings.is_empty() {
            let pins = existing
                .plugin_bindings
                .values()
                .cloned()
                .collect::<Vec<_>>();
            self.catalog.verify_child_plugins(graph_name, &pins)?;
        }
        Ok(())
    }

    fn persist_child_admission(
        &self,
        record: &GraphRunRecord,
        identity: &CallIdentity,
        graph_name: &str,
        mode: &str,
        session_context: Option<&Value>,
    ) -> Result<(), GraphError> {
        let run_id = &record.run_id;
        self.catalog
            .record_child_admission(
                run_id,
                graph_name,
                &record.snapshot,
                &record.plugin_bindings.values().cloned().collect::<Vec<_>>(),
                identity,
                mode,
            )
            .map_err(|e| {
                GraphError::Unsupported(format!("persist child identity metadata: {e}"))
            })?;
        if let Some(context) = session_context {
            self.catalog.record_session_call(run_id, context)?;
        }
        self.store
            .save(record)
            .map_err(|e| GraphError::Unsupported(format!("persist child Run: {e}")))?;
        Ok(())
    }

    async fn execute_wait_child(
        &self,
        child: GraphRunRecord,
        run_id: &str,
        graph_name: &str,
        cancellation: Cancellation,
    ) -> Result<GraphRunRecord, GraphError> {
        let child_control = ChildControl::new(
            self.catalog
                .begin_child_execution(run_id, graph_name, cancellation)
                .await?,
        );
        let child_nodes = BoundNodes {
            nodes: self.nodes,
            plugin_catalog: self.plugin_catalog,
            graph_calls: self,
        };
        let execution = GraphRunner::new(self.store, self.artifacts, &child_nodes, &child_control)
            .run(child)
            .await;
        // Registration ends on success, storage/lease errors and cooperative cancellation.
        // Preserve the Runner's error and all durable recovery/unknown-effect facts.
        self.catalog.end_child_execution(run_id).await;
        let child =
            execution.map_err(|e| GraphError::Unsupported(format!("execute child Run: {e}")))?;
        self.catalog
            .child_execution_finished(run_id, graph_name, child.status)
            .await?;
        Ok(child)
    }

    async fn project_child_outcome(
        &self,
        child: GraphRunRecord,
        run_id: String,
        identity: &CallIdentity,
        call_spec: &CallSpec,
    ) -> Result<GraphCallOutcome, GraphError> {
        let graph_name = call_spec.graph.as_str();
        match child.status {
            RunStatus::Completed => {
                let output = if let Some(selector) = call_spec.result.as_ref() {
                    let result = child
                        .results
                        .get(&selector.node)
                        .and_then(|results| results.iter().max_by_key(|r| r.sequence))
                        .ok_or_else(|| {
                            GraphError::CorruptRun(format!(
                                "called Graph `{graph_name}` produced no result node `{}`",
                                selector.node
                            ))
                        })?;
                    let call_key = InvocationKey {
                        run_id: identity.parent_run_id.clone(),
                        graph_digest: identity.parent_graph_digest.clone(),
                        node_id: identity.node_id.clone(),
                        invocation: identity.invocation,
                    };
                    let copied = self
                        .artifacts
                        .export_call_result_files(&call_key, &result.commit, &selector.files)
                        .await?;
                    serde_json::json!({
                        "graph": graph_name,
                        "run": child.run_id,
                        "mode": "wait",
                        "status": "completed",
                        "summary": result.completion.submission,
                        "result": {
                            "node": result.node_id,
                            "commit": result.commit,
                            "files": copied,
                        },
                    })
                } else {
                    child
                        .results
                        .values()
                        .flatten()
                        .max_by_key(|result| result.sequence)
                        .map(|result| result.completion.output.clone())
                        .ok_or_else(|| {
                            GraphError::CorruptRun("completed child has no result".into())
                        })?
                };
                Ok(GraphCallOutcome::Completed {
                    child_run_id: run_id,
                    output,
                })
            }
            RunStatus::WaitingCall
            | RunStatus::WaitingRecovery
            | RunStatus::BudgetStopped
            | RunStatus::Paused
            | RunStatus::Stopped => Ok(GraphCallOutcome::Waiting {
                child_run_id: run_id,
            }),
            RunStatus::Aborted => Ok(GraphCallOutcome::Failed {
                child_run_id: Some(run_id),
                reason: child.error.unwrap_or_else(|| "child Graph failed".into()),
            }),
            // A known failure clears its cursor; a retained cursor means the
            // child could not prove its terminal fact (unknown side effect
            // or an unproven completion). Report that as Uncertain so the
            // parent records it and never treats the call as a settled
            // known failure it may replay.
            RunStatus::Failed if child.cursor.is_some() => Ok(GraphCallOutcome::Uncertain {
                child_run_id: Some(run_id),
                reason: child
                    .error
                    .unwrap_or_else(|| "child Graph failed without a proven terminal fact".into()),
            }),
            RunStatus::Failed => Ok(GraphCallOutcome::Failed {
                child_run_id: Some(run_id),
                reason: child.error.unwrap_or_else(|| "child Graph failed".into()),
            }),
            RunStatus::Ready | RunStatus::Running => Ok(GraphCallOutcome::Uncertain {
                child_run_id: Some(run_id),
                reason: format!("child Run returned nonterminal status {:?}", child.status),
            }),
        }
    }
}

fn child_run_id(identity: &CallIdentity) -> String {
    format!("call-{}", identity.durable_key())
}

/// Strict, typed view of a frozen `ops.<name>.call` object. Unknown fields are
/// rejected so any future/unrecognized capability cannot be
/// silently ignored before a child Run or side effect is created.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CallSpec {
    graph: String,
    mode: String,
    #[serde(default)]
    session: Option<String>,
    #[serde(default)]
    input: Option<Value>,
    #[serde(default)]
    input_map: BTreeMap<String, String>,
    #[serde(default)]
    files: Vec<CallSpecFile>,
    #[serde(default)]
    result: Option<CallSpecResult>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CallSpecFile {
    node: String,
    path: String,
    #[serde(rename = "as")]
    alias: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CallSpecResult {
    node: String,
    #[serde(default)]
    files: Vec<String>,
}

impl CallSpec {
    /// Resolve the `input` constants merged with the explicit `input_map`
    /// pointers. Pointers are evaluated with RFC6901 semantics against the
    /// parent Run's own input (the frozen `input` field of the prepared input),
    /// matching the contract that only explicitly selected data crosses the
    /// boundary.
    fn child_input(&self, prepared_input: &Value) -> Result<Value, GraphError> {
        let source = prepared_input.get("input").unwrap_or(&Value::Null);
        let mut values = match &self.input {
            Some(value) if value.is_object() => value.clone(),
            Some(_) => {
                return Err(GraphError::InvalidSnapshot(
                    "op.call input must be an object".into(),
                ));
            }
            None => serde_json::json!({}),
        };
        let target = values
            .as_object_mut()
            .expect("checked object input before mapping");
        for (key, pointer) in &self.input_map {
            let value = source.pointer(pointer).ok_or_else(|| {
                GraphError::InvalidSnapshot(format!(
                    "op.call input_map pointer `{pointer}` is not visible from the parent input"
                ))
            })?;
            target.insert(key.clone(), value.clone());
        }
        Ok(values)
    }

    fn file_selections(&self) -> Vec<CallFileSelection> {
        self.files
            .iter()
            .map(|file| CallFileSelection {
                node: file.node.clone(),
                path: file.path.clone(),
                alias: file.alias.clone(),
            })
            .collect()
    }
}

fn expected_input(snapshot: &GraphSnapshot, provided: &Value) -> Value {
    // GraphRunRecord::create applies the kernel's input merge rules. Keep this
    // helper aligned with the currently supported object merge behavior.
    fn merge(base: &Value, over: &Value) -> Value {
        match (base, over) {
            (Value::Object(a), Value::Object(b)) => {
                let mut result = a.clone();
                for (key, value) in b {
                    let merged = result
                        .get(key)
                        .filter(|old| old.is_object() && value.is_object())
                        .map(|old| merge(old, value))
                        .unwrap_or_else(|| value.clone());
                    result.insert(key.clone(), merged);
                }
                Value::Object(result)
            }
            (_, Value::Null) => base.clone(),
            (_, value) => value.clone(),
        }
    }
    merge(&snapshot.input, provided)
}

#[cfg(test)]
#[path = "graph_call_spec_tests.rs"]
mod spec_tests;

#[cfg(test)]
#[path = "graph_call_session_tests.rs"]
mod session_tests;
