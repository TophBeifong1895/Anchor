//! Execute and freeze one open slot. This adapter never advances Graph facts.
use super::*;
use crate::graph::validator::{DispatchKind, NodeSubmission, SettleSubmission, routes_of};
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
                    return Ok(Some(observation_stop(&resource, &key, reason, Value::Null)));
                }
                CompletionFact::NotStarted | CompletionFact::Resumable => {
                    let input = self.input(&record, &dispatch, key.clone()).await?;
                    if let Some(boundary) = self.boundary {
                        boundary.before_dispatch(&resource.id)?;
                    }
                    if self.load(&resource.id)?.phase() != Some(&Phase::Executing) {
                        return Ok(None);
                    }
                    let request = match authorized_execution_request(
                        &record,
                        &input,
                        self.nodes,
                        self.cancellation.clone(),
                    ) {
                        Ok(request) => request,
                        Err(error) => {
                            return self
                                .known_failure(&resource, &dispatch, error.to_string(), 0)
                                .map(Some);
                        }
                    };
                    self.nodes.execute(request).await?
                }
                CompletionFact::Yielded { reason, route } => {
                    NodeExecutionOutcome::Yielded { reason, route }
                }
            },
        };
        let (outcome, interruption) = match outcome {
            NodeExecutionOutcome::Yielded { reason, route } => {
                if dispatch.branch.is_some() {
                    return self
                        .known_failure(
                            &resource,
                            &dispatch,
                            "interruption cannot complete a parallel branch".into(),
                            0,
                        )
                        .map(Some);
                }
                let completion = interruption_completion(&reason, route);
                // Use the same routing function as the validator before publishing an immutable control artifact.
                match select_route(&completion, &routes_of(&record, &key.node_id)) {
                    Ok(_) => (NodeExecutionOutcome::Completed(completion), Some(reason)),
                    Err(error) => (
                        NodeExecutionOutcome::Failed {
                            reason: error.to_string(),
                        },
                        None,
                    ),
                }
            }
            outcome => (outcome, None),
        };
        match outcome {
            NodeExecutionOutcome::Completed(c) => {
                if dispatch.kind == DispatchKind::Work
                    && let Err(error) = select_route(&c, &routes_of(&record, &key.node_id))
                {
                    return self.known_failure(
                        &resource,
                        &dispatch,
                        error.to_string(),
                        c.model_requests,
                    ).map(Some);
                }
                let context = ArtifactFreezeContext {
                    kind: match dispatch.kind {
                        _ if interruption.is_some() => ArtifactKind::Interruption,
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
                    interruption: interruption.clone(),
                    plugin_views: self.nodes.observed_plugin_views(&key),
                    plugin_digests: self.nodes.observed_plugin_digests(&key),
                };
                Ok(Some((
                    Principal::new(
                        if dispatch.kind == DispatchKind::Work && interruption.is_none() {
                            EXECUTOR_ID
                        } else {
                            HOST_ID
                        },
                        if dispatch.kind == DispatchKind::Work && interruption.is_none() {
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
            NodeExecutionOutcome::Failed { reason } => self
                .known_failure(&resource, &dispatch, reason, 0)
                .map(Some),
            NodeExecutionOutcome::WaitingRecovery { attempts } => Ok(Some(observation_stop(
                &resource,
                &key,
                "unknown external tool effects".into(),
                serde_json::to_value(attempts).map_err(GraphError::RunDecode)?,
            ))),
            NodeExecutionOutcome::BudgetExhausted { model_requests } => {
                let (principal, _, submission) = observation_stop(
                    &resource, &key, "budget".into(), Value::Null,
                );
                Ok(Some((principal, model_requests, submission)))
            }
            NodeExecutionOutcome::Interrupted { reason } => Ok(Some(observation_stop(
                &resource, &key, reason, Value::Null,
            ))),
            NodeExecutionOutcome::Aborted => Ok(Some((
                // This port outcome attests an explicit operator decision;
                // Code cannot manufacture a Run abandonment on its own.
                Principal::new(OPERATOR_ID, PrincipalKind::Human),
                0,
                Submission::new(&resource.id, resource.version, Action::Request)
                    .content(serde_json::json!({"control":"abandon","reason":format!("{} aborted by operator", key.node_id)}))
                    .evidence(Evidence::Observation { id: format!("operator-decision/{}", key.durable_key()) }),
            ))),
            NodeExecutionOutcome::Cancelled | NodeExecutionOutcome::Suspended => Ok(None),
            NodeExecutionOutcome::Yielded { .. } => unreachable!("normalized above"),
        }
    }

    fn known_failure(
        &self,
        resource: &Resource,
        dispatch: &Dispatch,
        reason: String,
        model_requests: u64,
    ) -> Result<StampedSubmission, GraphError> {
        Ok((
            Principal::new(HOST_ID, PrincipalKind::Code),
            model_requests,
            Submission::new(&resource.id, resource.version, Action::Settle)
                .slot(dispatch.slot())
                .content(
                    serde_json::to_value(SettleSubmission { reason })
                        .map_err(GraphError::RunDecode)?,
                ),
        ))
    }

    pub(super) async fn input(
        &self,
        record: &GraphRunRecord,
        dispatch: &Dispatch,
        key: InvocationKey,
    ) -> Result<DispatchInput, GraphError> {
        dispatch_input(self.artifacts, record, dispatch, key).await
    }
}

/// Describe an execution observation without changing Run facts or releasing a
/// wait. Explicit recovery and the scheduler persist the same Code evidence.
pub fn observation_stop(
    resource: &Resource,
    key: &InvocationKey,
    reason: String,
    attempts: Value,
) -> (Principal, u64, Submission) {
    (Principal::new(HOST_ID, PrincipalKind::Code), 0,
        Submission::new(&resource.id, resource.version, Action::Request)
            .content(serde_json::json!({"control":"stop","reason":reason,"invocation":key,"attempts":attempts}))
            .evidence(Evidence::Observation { id: format!("node-completion/{}", key.durable_key()) }))
}

/// Resolve one validator dispatch through the same provenance/input adapter
/// used by execution and by Host call admission validation.
pub async fn dispatch_input<A: ArtifactPort + ?Sized>(
    artifacts: &A,
    record: &GraphRunRecord,
    dispatch: &Dispatch,
    key: InvocationKey,
) -> Result<DispatchInput, GraphError> {
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
    Ok(DispatchInput {
        node_id: dispatch.node.id.clone(),
        key,
        input_commits: dispatch.inputs.clone(),
        prepared_input,
        branch: dispatch.branch,
    })
}
