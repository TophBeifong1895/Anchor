//! Serial Kernel path used by the Host concept probe. The Kernel owns all
//! progress and lifecycle facts; this adapter only executes and submits.
use super::validator::{DispatchKind, GraphValidator, NodeSubmission, SettleSubmission, record_of};
use super::*;
use anchor_kernel::{
    Action, Evidence, Kernel, Phase, Principal, PrincipalKind, Reason, Resource, Store, Submission,
    Verdict,
};

pub const EXECUTOR_ID: &str = "kernel-probe/executor";
pub const HOST_ID: &str = "kernel-probe/host";
pub const OPERATOR_ID: &str = "kernel-probe/operator";

pub fn validate_probe(record: &GraphRunRecord) -> Result<(), GraphError> {
    for op in record.snapshot.ops.values() {
        if ["fanout", "join", "call", "host"]
            .iter()
            .any(|kind| op.get(kind).is_some())
        {
            return Err(GraphError::Unsupported("P probe supports serial Work only; fanout/join, Op.call and assistant host operations require a later slice".into()));
        }
    }
    Ok(())
}

pub fn kernel_error(error: anchor_kernel::KernelError) -> GraphError {
    GraphError::Io(io::Error::other(error.to_string()))
}

/// Optional Host read observation; called only after an accepted submission.
/// A failed observer cannot undo the accepted Kernel fact.
pub trait SubmissionObserver: Send + Sync {
    fn accepted(&self, resource_id: &str) -> Result<(), GraphError>;
}

/// Host control events are handled between committed node results and the next
/// dispatch. This does not inspect the executor's cancellation or pause flags.
pub trait DispatchBoundary: Send + Sync {
    fn before_dispatch(&self, resource_id: &str) -> Result<(), GraphError>;
}

/// Charge a unit of work once, including stale/late refusals. Repeated stale
/// submissions carry zero additional cost (K0.1/D1).
pub fn submit_with_retry<S: Store>(
    kernel: &Kernel<S>,
    principal: &Principal,
    mut cost: u64,
    mut submission: Submission,
    observer: Option<&dyn SubmissionObserver>,
) -> Result<Verdict, GraphError> {
    loop {
        let verdict = kernel
            .submit(principal, cost, submission.clone())
            .map_err(kernel_error)?;
        if verdict.reason != Reason::StaleVersion {
            if verdict.is_accepted()
                && let Some(observer) = observer
                && let Err(error) = observer.accepted(&submission.target)
            {
                eprintln!(
                    "Host update observation unavailable for {}: {error}",
                    submission.target
                );
            }
            return Ok(verdict);
        }
        cost = 0;
        submission.seen = kernel
            .load(&submission.target)
            .map_err(kernel_error)?
            .ok_or_else(|| GraphError::CorruptRun("Kernel Run disappeared".into()))?
            .version;
    }
}

pub struct KernelScheduler<'a, S: Store, A: ArtifactPort + ?Sized, N: NodeExecutionPort + ?Sized> {
    pub kernel: &'a Kernel<S>,
    pub observer: Option<&'a dyn SubmissionObserver>,
    pub boundary: Option<&'a dyn DispatchBoundary>,
    pub artifacts: &'a A,
    pub nodes: &'a N,
    pub cancellation: crate::Cancellation,
}

impl<S: Store, A: ArtifactPort + ?Sized, N: NodeExecutionPort + ?Sized>
    KernelScheduler<'_, S, A, N>
{
    pub async fn run(&self, id: &str) -> Result<Resource, GraphError> {
        loop {
            let resource = self
                .kernel
                .load(id)
                .map_err(kernel_error)?
                .ok_or_else(|| GraphError::CorruptRun(format!("missing Kernel Run {id}")))?;
            if resource.phase() != Some(&Phase::Executing) {
                if let Some(boundary) = self.boundary {
                    boundary.before_dispatch(id)?;
                }
                return Ok(resource);
            }
            let record = record_of(&resource)?;
            validate_probe(&record)?;
            let frontier = GraphValidator::frontier(&resource)?;
            if frontier.len() != 1 || frontier[0].kind != DispatchKind::Work {
                return Err(GraphError::Unsupported(
                    "P probe requires one open Work slot".into(),
                ));
            }
            let dispatch = &frontier[0];
            let key = InvocationKey {
                run_id: record.run_id.clone(),
                graph_digest: record.graph_digest.clone(),
                node_id: dispatch.node.id.clone(),
                invocation: dispatch.invocation,
            };
            let outcome = match self.nodes.completion_fact(&key).await? {
                CompletionFact::Completed(c) => NodeExecutionOutcome::Completed(c),
                CompletionFact::Failed(reason) => NodeExecutionOutcome::Failed { reason },
                CompletionFact::Uncertain(reason) => {
                    self.park(&resource, &key, reason, Value::Null)?;
                    continue;
                }
                CompletionFact::NotStarted | CompletionFact::Resumable => {
                    let mut resolved = Vec::new();
                    for commit in &dispatch.inputs {
                        resolved.push(self.artifacts.resolve(commit).await?);
                    }
                    let cursor = RunCursor {
                        node_id: dispatch.node.id.clone(),
                        key: key.clone(),
                        input_commits: dispatch.inputs.clone(),
                        prepared_input: serde_json::json!({"input":record.input,"committed_inputs":resolved}),
                    };
                    if let Some(boundary) = self.boundary {
                        boundary.before_dispatch(id)?;
                        let current =
                            self.kernel.load(id).map_err(kernel_error)?.ok_or_else(|| {
                                GraphError::CorruptRun("missing Kernel Run at dispatch".into())
                            })?;
                        if current.phase() != Some(&Phase::Executing) {
                            return Ok(current);
                        }
                    }
                    let request = authorized_execution_request(
                        &record,
                        &cursor,
                        self.nodes,
                        self.cancellation.clone(),
                    )?;
                    self.nodes.execute(request).await?
                }
                CompletionFact::Yielded { .. } => {
                    return Err(GraphError::Unsupported(
                        "P probe: Yielded completion requires the assistant slice".into(),
                    ));
                }
            };
            let (principal, cost, submission) = match outcome {
                NodeExecutionOutcome::Completed(c) => {
                    let commit = self.artifacts.freeze(&key, &c).await?;
                    let evidence = Evidence::Resource {
                        id: commit.id.clone(),
                        version: 0,
                    };
                    let cost = c.model_requests;
                    let content = NodeSubmission {
                        summary: c.submission,
                        route: c.route,
                        output: c.output,
                        model_requests: c.model_requests,
                        commit,
                        interruption: None,
                        plugin_views: self.nodes.observed_plugin_views(&key),
                        plugin_digests: self.nodes.observed_plugin_digests(&key),
                    };
                    (
                        Principal::new(EXECUTOR_ID, PrincipalKind::Agent),
                        cost,
                        Submission::new(id, resource.version, Action::Write)
                            .slot(dispatch.slot())
                            .content(serde_json::to_value(content).map_err(GraphError::RunDecode)?)
                            .evidence(evidence),
                    )
                }
                NodeExecutionOutcome::Failed { reason } => (
                    Principal::new(HOST_ID, PrincipalKind::Code),
                    0,
                    Submission::new(id, resource.version, Action::Settle)
                        .slot(dispatch.slot())
                        .content(
                            serde_json::to_value(SettleSubmission { reason })
                                .map_err(GraphError::RunDecode)?,
                        ),
                ),
                NodeExecutionOutcome::WaitingRecovery { attempts } => {
                    self.park(
                        &resource,
                        &key,
                        "unknown external tool effects".into(),
                        serde_json::to_value(attempts).map_err(GraphError::RunDecode)?,
                    )?;
                    continue;
                }
                NodeExecutionOutcome::Cancelled | NodeExecutionOutcome::Suspended => {
                    return self
                        .kernel
                        .load(id)
                        .map_err(kernel_error)?
                        .ok_or_else(|| GraphError::CorruptRun("missing Kernel Run".into()));
                }
                unsupported => {
                    return Err(GraphError::Unsupported(format!(
                        "P probe: {unsupported:?} requires a later runtime slice"
                    )));
                }
            };
            let verdict =
                submit_with_retry(self.kernel, &principal, cost, submission, self.observer)?;
            if !verdict.is_accepted()
                && !matches!(verdict.reason, Reason::TaskWaiting | Reason::TaskSettled)
            {
                return Err(GraphError::Unsupported(format!(
                    "P probe submission refused: {:?}",
                    verdict.reason
                )));
            }
            if let Some(boundary) = self.boundary {
                boundary.before_dispatch(id)?;
            }
        }
    }

    fn park(
        &self,
        resource: &Resource,
        key: &InvocationKey,
        reason: String,
        attempts: Value,
    ) -> Result<(), GraphError> {
        let submission = Submission::new(&resource.id, resource.version, Action::Request)
            .content(serde_json::json!({"control":"stop","reason":reason,"invocation":key,"attempts":attempts}))
            .evidence(Evidence::Observation { id: format!("node-completion/{}", key.durable_key()) });
        let verdict = submit_with_retry(
            self.kernel,
            &Principal::new(HOST_ID, PrincipalKind::Code),
            0,
            submission,
            self.observer,
        )?;
        if verdict.is_accepted()
            || matches!(verdict.reason, Reason::TaskSettled | Reason::TaskWaiting)
        {
            Ok(())
        } else {
            Err(GraphError::Unsupported(format!(
                "P observation stop refused: {:?}",
                verdict.reason
            )))
        }
    }
}
