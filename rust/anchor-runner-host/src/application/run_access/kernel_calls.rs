//! The shared Graph-call admission adapter over Kernel-owned Runs. The
//! compatibility cursor below is local to one validated call and never saved.
use super::{ApplicationError, RunApplication, kernel_runs::KernelRuns};
use anchor_graph_host::{ChildExecutionPort, FilePluginCatalog, InProcessGraphHost};
use anchor_kernel::{Phase, Waiter};
use anchor_runtime::{Cancellation, graph::*};
use std::{collections::BTreeMap, future::Future, pin::Pin};

fn application_error(error: ApplicationError) -> GraphError {
    GraphError::CorruptRun(format!("Kernel Graph call: {error:?}"))
}

pub(crate) struct KernelCallNodes<'a, A, N> {
    application: &'a RunApplication,
    artifacts: &'a A,
    nodes: &'a N,
}
impl<'a, A, N> KernelCallNodes<'a, A, N> {
    pub(crate) fn new(application: &'a RunApplication, artifacts: &'a A, nodes: &'a N) -> Self {
        Self {
            application,
            artifacts,
            nodes,
        }
    }
}
impl<A: ArtifactPort, N: NodeExecutionPort> NodeExecutionPort for KernelCallNodes<'_, A, N> {
    fn capabilities(&self) -> NodeExecutionCapabilities {
        self.nodes.capabilities()
    }
    fn graph_call_port(&self) -> Option<&dyn GraphCallPort> {
        Some(self)
    }
    fn resolve_plugins(&self, ids: &[String]) -> Result<Vec<PluginBinding>, GraphError> {
        self.nodes.resolve_plugins(ids)
    }
    fn invocation_plugin_authority(&self, record: &GraphRunRecord) -> Result<bool, GraphError> {
        self.nodes.invocation_plugin_authority(record)
    }
    fn observed_plugin_views(&self, key: &InvocationKey) -> BTreeMap<String, String> {
        self.nodes.observed_plugin_views(key)
    }
    fn observed_plugin_digests(&self, key: &InvocationKey) -> BTreeMap<String, String> {
        self.nodes.observed_plugin_digests(key)
    }
    fn completion_fact<'a>(
        &'a self,
        key: &'a InvocationKey,
    ) -> Pin<Box<dyn Future<Output = Result<CompletionFact, GraphError>> + Send + 'a>> {
        self.nodes.completion_fact(key)
    }
    fn record_recovery_decision(
        &self,
        key: &InvocationKey,
        attempt: i64,
        decision: RecoveryDecision,
    ) -> Result<(), GraphError> {
        self.nodes.record_recovery_decision(key, attempt, decision)
    }
    fn execute<'a>(
        &'a self,
        request: NodeExecutionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<NodeExecutionOutcome, GraphError>> + Send + 'a>> {
        self.nodes.execute(request)
    }
}

struct KernelCallStore<'a> {
    application: &'a RunApplication,
    parent: GraphRunRecord,
}
impl RunStore for KernelCallStore<'_> {
    fn load(&self, id: &str) -> Result<Option<GraphRunRecord>, GraphError> {
        if id == self.parent.run_id {
            return Ok(Some(self.parent.clone()));
        }
        self.application.load_run(id).map_err(application_error)
    }
    fn save(&self, record: &GraphRunRecord) -> Result<(), GraphError> {
        // InProcessGraphHost writes only admission here. The executor submits
        // all later progress/lifecycle changes through the Kernel.
        if self
            .application
            .load_run(&record.run_id)
            .map_err(application_error)?
            .is_some()
        {
            return Err(GraphError::RunConflict);
        }
        self.application
            .kernel_runs(true)
            .map_err(application_error)?
            .expect("enabled")
            .create(record, u64::MAX)
            .map_err(application_error)
    }
    fn acquire_lease(&self, id: &str) -> Result<Box<dyn RunLease>, GraphError> {
        self.application.store().acquire_lease(id)
    }
}

impl<A: ArtifactPort, N: NodeExecutionPort> GraphCallPort for KernelCallNodes<'_, A, N> {
    fn call<'a>(
        &'a self,
        identity: &'a CallIdentity,
        spec: &'a serde_json::Value,
        input: &'a serde_json::Value,
        commits: &'a [CommitRef],
        cancellation: Cancellation,
    ) -> Pin<Box<dyn Future<Output = Result<GraphCallOutcome, GraphError>> + Send + 'a>> {
        Box::pin(async move {
            let owner = self
                .application
                .kernel_owner(&identity.parent_run_id)
                .map_err(application_error)?
                .ok_or_else(|| GraphError::CorruptRun("call parent is not Kernel-owned".into()))?;
            let resource = owner
                .load(&identity.parent_run_id)
                .map_err(application_error)?
                .ok_or_else(|| GraphError::CorruptRun("missing Kernel call parent".into()))?;
            let mut parent = KernelRuns::record(&resource).map_err(application_error)?;
            let dispatch = validator::GraphValidator::frontier(&resource)?
                .into_iter()
                .find(|d| {
                    d.kind == validator::DispatchKind::Call
                        && d.node.id == identity.node_id
                        && d.invocation == identity.invocation
                })
                .ok_or_else(|| {
                    GraphError::CorruptRun("call is not a current Kernel dispatch".into())
                })?;
            if validator::call_identity(&parent, &dispatch)? != *identity
                || dispatch.inputs != commits
            {
                return Err(GraphError::CorruptRun(
                    "call identity or provenance differs from Kernel dispatch".into(),
                ));
            }
            let key = InvocationKey {
                run_id: parent.run_id.clone(),
                graph_digest: parent.graph_digest.clone(),
                node_id: identity.node_id.clone(),
                invocation: identity.invocation,
            };
            let cursor =
                scheduler::dispatch_cursor(self.artifacts, &parent, &dispatch, key).await?;
            if cursor.prepared_input != *input {
                return Err(GraphError::CorruptRun(
                    "call input differs from Kernel dispatch".into(),
                ));
            }
            let defer = !parent.graph_calls.contains_key(&identity.durable_key());
            parent.cursor = Some(cursor);
            let store = KernelCallStore {
                application: self.application,
                parent,
            };
            let catalog = crate::execution::RunnerGraphCatalog {
                root: self.application.catalog_root.clone(),
                state: self.application.data_root.clone(),
                application: Some(self.application.clone()),
            };
            let plugins =
                FilePluginCatalog::new(crate::library_root(&self.application.catalog_root));
            let control = super::new_control();
            let host =
                InProcessGraphHost::new(&catalog, &store, self.artifacts, self.nodes, &control)
                    .with_plugin_catalog(&plugins)
                    .with_child_executor(self, defer);
            match host
                .call(identity, spec, input, commits, cancellation)
                .await
            {
                Err(GraphError::InvalidSnapshot(reason)) => {
                    // Prove absence under the admission lock. An error after
                    // child admission cannot authorize a new child or replay.
                    let id = anchor_graph_host::child_run_id(identity);
                    let _lease = store.acquire_lease(&id)?;
                    if self
                        .application
                        .load_run(&id)
                        .map_err(application_error)?
                        .is_none()
                        && self
                            .application
                            .metadata(&id)
                            .map_err(application_error)?
                            .is_none()
                    {
                        Err(GraphError::CallAdmissionFailed(reason))
                    } else {
                        Err(GraphError::InvalidSnapshot(reason))
                    }
                }
                Ok(GraphCallOutcome::Failed {
                    child_run_id: Some(id),
                    reason,
                }) => {
                    // The compatibility record has one legacy Failed label;
                    // the Kernel Unknown fact remains authoritative.
                    let unknown = self
                        .application
                        .kernel_owner(&id)
                        .map_err(application_error)?
                        .map(|owner| owner.load(&id))
                        .transpose()
                        .map_err(application_error)?
                        .flatten()
                        .is_some_and(|resource| {
                            matches!(
                                resource.phase(),
                                Some(Phase::Settled {
                                    result: anchor_kernel::Outcome::Unknown { .. }
                                })
                            )
                        });
                    Ok(if unknown {
                        GraphCallOutcome::Uncertain {
                            child_run_id: Some(id),
                            reason,
                        }
                    } else {
                        GraphCallOutcome::Failed {
                            child_run_id: Some(id),
                            reason,
                        }
                    })
                }
                outcome => outcome,
            }
        })
    }
}

impl<A: ArtifactPort, N: NodeExecutionPort> ChildExecutionPort for KernelCallNodes<'_, A, N> {
    fn execute<'a>(
        &'a self,
        child: GraphRunRecord,
        cancellation: Cancellation,
    ) -> Pin<Box<dyn Future<Output = Result<GraphRunRecord, GraphError>> + Send + 'a>> {
        Box::pin(async move {
            let owner = self
                .application
                .kernel_owner(&child.run_id)
                .map_err(application_error)?
                .ok_or_else(|| {
                    GraphError::CorruptRun("child admission is not Kernel-owned".into())
                })?;
            let resource = owner
                .load(&child.run_id)
                .map_err(application_error)?
                .expect("owned child");
            // A Human wait remains parked. Child waits are observed by the
            // scheduler; no human release is manufactured by a parent call.
            if matches!(
                resource.phase(),
                Some(Phase::Waiting {
                    waiter: Waiter::Human { .. }
                })
            ) {
                return Ok(child);
            }
            let metadata = self
                .application
                .metadata(&child.run_id)
                .map_err(application_error)?
                .ok_or_else(|| GraphError::CorruptRun("missing child metadata".into()))?;
            let mut control = super::new_control();
            control.cancellation = cancellation;
            let prepared = self
                .application
                .prepare_resume(&child, &metadata, control.clone())
                .map_err(application_error)?;
            let (artifacts, nodes) = prepared.kernel_ports(self.application.clone());
            let nodes = KernelCallNodes::new(self.application, &artifacts, &nodes);
            let resource = owner
                .drive(&child.run_id, &artifacts, &nodes, control.cancellation)
                .await?;
            KernelRuns::record(&resource).map_err(application_error)
        })
    }
}

impl RunApplication {
    pub(crate) async fn kernel_recheck_child(&self, id: &str) -> Result<(), ApplicationError> {
        let owner = self.kernel_owner(id)?.ok_or(ApplicationError::Missing)?;
        let resource = owner.load(id)?.ok_or(ApplicationError::Missing)?;
        let Some(Phase::Waiting {
            waiter: Waiter::Child { task },
        }) = resource.phase()
        else {
            return Ok(());
        };
        let child = task
            .strip_prefix("run/")
            .ok_or_else(|| ApplicationError::Storage("invalid child wait target".into()))?;
        let Some(child_owner) = self.kernel_owner(child)? else {
            return Ok(());
        };
        if child_owner
            .load(child)?
            .is_some_and(|resource| resource.is_settled())
        {
            self.kernel_child_finished(child).await?;
        }
        Ok(())
    }

    pub(crate) async fn kernel_dispatch_detached(&self, id: &str) -> Result<(), ApplicationError> {
        let owner = self.kernel_owner(id)?.ok_or(ApplicationError::Missing)?;
        if owner.active(id) {
            return Ok(());
        }
        let resource = owner.load(id)?.ok_or(ApplicationError::Missing)?;
        if resource.phase() != Some(&Phase::Executing) {
            return Ok(());
        }
        let record = KernelRuns::record(&resource)?;
        let metadata = self.metadata(id)?.ok_or(ApplicationError::Missing)?;
        let source = metadata.graph_call.as_ref().ok_or_else(|| {
            ApplicationError::Invalid("detached child has no Graph call source".into())
        })?;
        if source.mode != "detach" || metadata.trigger_source != "graph_call" {
            return Err(ApplicationError::Invalid(
                "Run is not an admitted detached child".into(),
            ));
        }
        let control = super::new_control();
        let prepared = self.prepare_resume(&record, &metadata, control.clone())?;
        self.spawn_kernel(record, prepared, control)
    }

    pub(crate) fn kernel_child_finished<'a>(
        &'a self,
        id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), ApplicationError>> + Send + 'a>> {
        Box::pin(async move {
            let Some(owner) = self.kernel_owner(id)? else {
                return Ok(());
            };
            if !owner
                .load(id)?
                .is_some_and(|resource| resource.is_settled())
            {
                return Ok(());
            }
            let Some(metadata) = self.metadata(id)? else {
                return Ok(());
            };
            if metadata
                .session_call
                .as_ref()
                .is_some_and(|call| call.status == "pending")
            {
                return Ok(());
            }
            let Some(source) = metadata.graph_call else {
                return Ok(());
            };
            if source.mode != "wait" {
                return Ok(());
            }
            let Some(parent) = self.kernel_owner(&source.parent_run)? else {
                return Ok(());
            };
            if parent.active(&source.parent_run) {
                return Ok(());
            }
            let resource = parent
                .load(&source.parent_run)?
                .ok_or(ApplicationError::Missing)?;
            if resource.phase()
                != Some(&Phase::Waiting {
                    waiter: Waiter::Child {
                        task: format!("run/{id}"),
                    },
                })
            {
                return Ok(());
            }
            let record = KernelRuns::record(&resource)?;
            if !record.graph_calls.values().any(|call| {
                call.identity.node_id == source.node
                    && call.identity.invocation == source.invocation
                    && call.child_run_id.as_deref() == Some(id)
            }) {
                return Err(ApplicationError::Conflict(
                    "child completion differs from parent binding".into(),
                ));
            }
            let metadata = self
                .metadata(&source.parent_run)?
                .ok_or(ApplicationError::Missing)?;
            let control = super::new_control();
            let prepared = self.prepare_resume(&record, &metadata, control.clone())?;
            self.spawn_kernel(record, prepared, control)
        })
    }
}
