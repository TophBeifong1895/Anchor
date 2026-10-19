//! Execute and freeze one open slot. This adapter never advances Graph facts.
use super::*;
use crate::graph::validator::{DispatchKind, NodeSubmission, SettleSubmission};
use anchor_kernel::Evidence;

pub(super) type StampedSubmission = (Principal, u64, Submission);

impl<S: Store, A: ArtifactPort + ?Sized, N: NodeExecutionPort + ?Sized>
    KernelScheduler<'_, S, A, N>
{
    pub(super) async fn execute_slot(
        &self,
        resource: Resource,
        dispatch: Dispatch,
    ) -> Result<Option<StampedSubmission>, GraphError> {
        let record = record_of(&resource)?;
        let key = InvocationKey {
            run_id: record.run_id.clone(),
            graph_digest: record.graph_digest.clone(),
            node_id: dispatch.node.id.clone(),
            invocation: dispatch.invocation,
        };
        let outcome = match dispatch.kind {
            DispatchKind::Fanout => NodeExecutionOutcome::Completed(
                control_completion::fanout_completion(&record, &key)?,
            ),
            DispatchKind::Join => {
                NodeExecutionOutcome::Completed(control_completion::join_completion(&record))
            }
            DispatchKind::Call => {
                return self.execute_call(&resource, &dispatch).await.map(Some);
            }
            DispatchKind::Work => match self.nodes.completion_fact(&key).await? {
                CompletionFact::Completed(c) => NodeExecutionOutcome::Completed(c),
                CompletionFact::Failed(reason) => NodeExecutionOutcome::Failed { reason },
                CompletionFact::Uncertain(reason) => {
                    return Ok(Some(self.observation_stop(
                        &resource,
                        &key,
                        reason,
                        Value::Null,
                    )));
                }
                CompletionFact::NotStarted | CompletionFact::Resumable => {
                    let cursor = self.cursor(&record, &dispatch, key.clone()).await?;
                    if let Some(boundary) = self.boundary {
                        boundary.before_dispatch(&resource.id)?;
                    }
                    if self.load(&resource.id)?.phase() != Some(&Phase::Executing) {
                        return Ok(None);
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
                        "Yielded completion requires the assistant slice".into(),
                    ));
                }
            },
        };
        match outcome {
            NodeExecutionOutcome::Completed(c) => {
                let context = ArtifactFreezeContext {
                    kind: match dispatch.kind {
                        DispatchKind::Fanout => ArtifactKind::Fanout,
                        DispatchKind::Join => ArtifactKind::Join,
                        _ => ArtifactKind::Node,
                    },
                    input_commits: dispatch.inputs.clone(),
                };
                let commit = self
                    .artifacts
                    .freeze_with_context(&key, &c, &context)
                    .await?;
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
                Ok(Some((
                    Principal::new(
                        if dispatch.kind == DispatchKind::Work {
                            EXECUTOR_ID
                        } else {
                            HOST_ID
                        },
                        if dispatch.kind == DispatchKind::Work {
                            PrincipalKind::Agent
                        } else {
                            PrincipalKind::Code
                        },
                    ),
                    cost,
                    Submission::new(&resource.id, resource.version, Action::Write)
                        .slot(dispatch.slot())
                        .content(serde_json::to_value(content).map_err(GraphError::RunDecode)?)
                        .evidence(evidence),
                )))
            }
            NodeExecutionOutcome::Failed { reason } => Ok(Some((
                Principal::new(HOST_ID, PrincipalKind::Code),
                0,
                Submission::new(&resource.id, resource.version, Action::Settle)
                    .slot(dispatch.slot())
                    .content(
                        serde_json::to_value(SettleSubmission { reason })
                            .map_err(GraphError::RunDecode)?,
                    ),
            ))),
            NodeExecutionOutcome::WaitingRecovery { attempts } => Ok(Some(self.observation_stop(
                &resource,
                &key,
                "unknown external tool effects".into(),
                serde_json::to_value(attempts).map_err(GraphError::RunDecode)?,
            ))),
            NodeExecutionOutcome::Cancelled | NodeExecutionOutcome::Suspended => Ok(None),
            unsupported => Err(GraphError::Unsupported(format!(
                "{unsupported:?} requires a later runtime slice"
            ))),
        }
    }

    pub(super) async fn cursor(
        &self,
        record: &GraphRunRecord,
        dispatch: &Dispatch,
        key: InvocationKey,
    ) -> Result<RunCursor, GraphError> {
        dispatch_cursor(self.artifacts, record, dispatch, key).await
    }

    fn observation_stop(
        &self,
        resource: &Resource,
        key: &InvocationKey,
        reason: String,
        attempts: Value,
    ) -> StampedSubmission {
        (Principal::new(HOST_ID, PrincipalKind::Code), 0,
            Submission::new(&resource.id, resource.version, Action::Request)
                .content(serde_json::json!({"control":"stop","reason":reason,"invocation":key,"attempts":attempts}))
                .evidence(Evidence::Observation { id: format!("node-completion/{}", key.durable_key()) }))
    }
}

/// Resolve one validator dispatch through the same provenance/input adapter
/// used by execution and by Host call admission validation.
pub async fn dispatch_cursor<A: ArtifactPort + ?Sized>(
    artifacts: &A,
    record: &GraphRunRecord,
    dispatch: &Dispatch,
    key: InvocationKey,
) -> Result<RunCursor, GraphError> {
    let mut resolved = Vec::new();
    for commit in &dispatch.inputs {
        resolved.push(artifacts.resolve(commit).await?);
    }
    let mut prepared_input = serde_json::json!({"input":record.input,"committed_inputs":resolved});
    if let Some(index) = dispatch.branch {
        let activation = record.parallel.as_ref().expect("parallel dispatch");
        prepared_input["parallel_activation"] = serde_json::json!(activation.activation_id);
        prepared_input["branch_id"] = serde_json::json!(activation.branches[index].branch_id);
    }
    Ok(RunCursor {
        node_id: dispatch.node.id.clone(),
        key,
        input_commits: dispatch.inputs.clone(),
        prepared_input,
    })
}
