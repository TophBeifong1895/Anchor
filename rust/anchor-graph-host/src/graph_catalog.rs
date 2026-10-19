//! Host catalog, admission and child execution lifecycle ports.

use crate::LoadedGraphBundle;
use anchor_kernel::Phase;
use anchor_runtime::Cancellation;
use anchor_runtime::graph::{CallIdentity, GraphCallOutcome, GraphError, GraphSnapshot, RunLease};
use serde_json::Value;
use std::{future::Future, pin::Pin};

/// Acquisition of the short Library lease protecting durable child references.
pub type PluginReferenceLeaseFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Box<dyn RunLease>, GraphError>> + Send + 'a>>;

pub trait GraphCatalog: Send + Sync {
    fn snapshot(&self, name: &str) -> Result<Option<GraphSnapshot>, GraphError>;

    /// Return the graph names for a durable Run, starting with its own Graph
    /// and following every Graph-call parent to the root. Implementations must
    /// fail closed when the durable ancestry cannot be proven.
    fn graph_ancestry(&self, _run_id: &str) -> Result<Vec<String>, GraphError> {
        Err(GraphError::Unsupported(
            "Graph call ancestry is not available from this catalog".into(),
        ))
    }

    /// Short admission lock shared with target-Graph mutation. This lock is
    /// released before child execution; child execution uses only its Run lease.
    fn lock_admission(&self, _name: &str) -> Result<Box<dyn RunLease>, GraphError> {
        Ok(Box::new(NoopLease))
    }

    /// Protect Plugin resolution through durable child admission against
    /// Library publication/deletion. Hosts share their publication lease here;
    /// it is released before model execution.
    fn lock_plugin_references<'a>(
        &'a self,
        _cancellation: Cancellation,
    ) -> PluginReferenceLeaseFuture<'a> {
        Box::pin(async { Ok(Box::new(NoopLease) as Box<dyn RunLease>) })
    }

    /// Resolve one immutable child bundle. Catalogs that only provide snapshots
    /// keep the provider-free adapter behavior; production catalogs should
    /// return the bundle's Plugin pins as well.
    fn bundle(&self, name: &str) -> Result<Option<LoadedGraphBundle>, GraphError> {
        self.snapshot(name)?
            .map(|snapshot| {
                let authoring_definition =
                    serde_json::to_value(&snapshot).map_err(GraphError::SnapshotDecode)?;
                Ok(LoadedGraphBundle {
                    authoring_definition,
                    snapshot,
                    plugins: Vec::new(),
                })
            })
            .transpose()
    }

    /// Persist API-visible identity metadata alongside a durable child Run.
    fn record_child_admission(
        &self,
        _run_id: &str,
        _graph: &str,
        _snapshot: &GraphSnapshot,
        _plugins: &[anchor_runtime::graph::PluginBinding],
        _identity: &CallIdentity,
        _mode: &str,
    ) -> Result<(), GraphError> {
        Ok(())
    }

    /// Resolve and authorize an existing channel Session. Returned context
    /// supplies the trusted Session identity and channel input for the child.
    fn prepare_session_call<'a>(
        &'a self,
        _identity: &'a CallIdentity,
        _session: &'a str,
        _graph: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Value, GraphError>> + Send + 'a>> {
        Box::pin(async {
            Err(GraphError::Unsupported(
                "Op.call session handoff is not supported by this host".into(),
            ))
        })
    }

    /// Persist trusted Session metadata before the child Run becomes durable.
    fn record_session_call(&self, _run_id: &str, _context: &Value) -> Result<(), GraphError> {
        Err(GraphError::Unsupported(
            "Op.call session handoff is not supported by this host".into(),
        ))
    }

    /// Delegate Session scheduling, execution and message delivery to its
    /// owning host. `None` certifies successful execution and delivery; the
    /// adapter then reads the completed child to export the ordinary result.
    fn session_call_outcome<'a>(
        &'a self,
        _run_id: &'a str,
        _mode: &'a str,
        _cancellation: Cancellation,
    ) -> Pin<Box<dyn Future<Output = Result<Option<GraphCallOutcome>, GraphError>> + Send + 'a>>
    {
        Box::pin(async {
            Err(GraphError::Unsupported(
                "Op.call session handoff is not supported by this host".into(),
            ))
        })
    }

    /// Register a wait child with the host's per-Run control registry. The
    /// returned token belongs to the child; the host may additionally link
    /// parent cancellation for wait-mode calls.
    fn begin_child_execution<'a>(
        &'a self,
        _run_id: &'a str,
        _graph: &'a str,
        parent_cancellation: Cancellation,
    ) -> Pin<Box<dyn Future<Output = Result<ChildRunControl, GraphError>> + Send + 'a>> {
        Box::pin(async move { Ok(ChildRunControl::linked_to_parent(parent_cancellation)) })
    }

    fn end_child_execution<'a>(
        &'a self,
        _run_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async {})
    }

    fn child_execution_finished<'a>(
        &'a self,
        _run_id: &'a str,
        _graph: &'a str,
        _phase: Phase,
    ) -> Pin<Box<dyn Future<Output = Result<(), GraphError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }

    /// Admit a detached child to host-owned background execution. The default
    /// standalone adapter keeps admission-only behavior.
    fn dispatch_detached<'a>(
        &'a self,
        _run_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), GraphError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }

    /// Verify that an existing durable child still has its API-visible source
    /// identity. This must not require the mutable Graph definition to exist.
    fn verify_child_identity(
        &self,
        _run_id: &str,
        _graph: &str,
        _snapshot: &GraphSnapshot,
        _identity: &CallIdentity,
        _mode: &str,
    ) -> Result<(), GraphError> {
        Ok(())
    }

    fn has_child_admission(&self, _run_id: &str) -> Result<bool, GraphError> {
        Ok(false)
    }

    /// A child Graph must still exist before a Run resumes into it.
    ///
    /// The Plugin bindings themselves are no longer compared: a Run uses the
    /// *current* Plugin (standard invariant 6), so a Plugin that changed while the
    /// Run was parked is not a reason to refuse — the next node call simply uses
    /// what is installed now, and the run record observes which view that was.
    fn verify_child_plugins(
        &self,
        graph: &str,
        _bindings: &[anchor_runtime::graph::PluginBinding],
    ) -> Result<(), GraphError> {
        self.bundle(graph)?
            .ok_or_else(|| GraphError::InvalidSnapshot(format!("unknown child graph `{graph}`")))?;
        Ok(())
    }
}

struct NoopLease;
impl RunLease for NoopLease {}

#[derive(Clone)]
pub struct ChildRunControl {
    pub cancellation: Cancellation,
}

impl ChildRunControl {
    pub fn linked_to_parent(cancellation: Cancellation) -> Self {
        Self { cancellation }
    }
}
