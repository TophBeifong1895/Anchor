//! A Graph as the validator of a Run.
//!
//! The Run is a kernel task resource whose state is the frozen Graph and its
//! progress. The validator never calls anyone: [`open`](GraphValidator::open)
//! says which node invocations it would currently accept, and
//! [`judge`](GraphValidator::judge) applies a submitted result, chooses the
//! route and decides whether the Run has settled. Stop, pause, resume and
//! abandon are not here at all: they are the kernel's task lifecycle.
//!
//! Parallel regions are not a special execution path: a fanout opens one slot
//! per branch, and the join opens once every branch has completed. Whether a
//! scheduler runs the open slots in waves or as soon as they open is the
//! scheduler's choice, not the Graph's.
//!
//! Covered: serial routing, feedback edges, skipped paths, node and module
//! round limits, fanout/join. Not yet: Op.call, module scopes inside parallel
//! branches.

use anchor_kernel::{
    Action, Evidence, JudgeCtx, Judgement, Outcome, Phase, PrincipalKind, Reason, Resource, Slot,
    Submission, Validator, Verification,
};

use super::*;

/// The validator id a host installs this under.
pub const GRAPH_VALIDATOR: &str = "validator/graph";

pub struct GraphValidator;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchKind {
    /// Work for an executor: an Agent, Op.run or host operation.
    Work,
    /// Deterministic control, submitted only by host code.
    Fanout,
    Join,
}

/// One node invocation a Run would currently accept.
#[derive(Debug, Clone, PartialEq)]
pub struct Dispatch {
    pub node: GraphNode,
    pub invocation: u64,
    pub kind: DispatchKind,
    /// The parallel branch this invocation belongs to, if any.
    pub branch: Option<usize>,
    /// Committed inputs, computed after the same advance that chose the node:
    /// a stale selected edge retired by that advance is not an input.
    pub inputs: Vec<CommitRef>,
}

impl Dispatch {
    pub fn slot(&self) -> String {
        format!("{}#{}", self.node.id, self.invocation)
    }
}

/// Where a Run stands after applying everything known so far.
#[derive(Debug, Clone, PartialEq)]
pub enum Advance {
    Open(Vec<Dispatch>),
    /// No node can run again.
    Done,
    /// The Graph cannot continue, for example a parallel branch hit its
    /// round limit.
    Failed(String),
}

/// The content of a node submission. `commit` is the immutable artifact the
/// executor froze before submitting; the Graph never freezes anything.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeSubmission {
    pub summary: String,
    #[serde(default)]
    pub route: Option<String>,
    #[serde(default)]
    pub output: Value,
    #[serde(default)]
    pub model_requests: u64,
    pub commit: CommitRef,
    #[serde(default)]
    pub plugin_views: BTreeMap<String, String>,
    #[serde(default)]
    pub plugin_digests: BTreeMap<String, String>,
}

/// The kernel resource for a new Run.
pub fn run_resource(record: &GraphRunRecord, budget: u64) -> Result<Resource, GraphError> {
    if record
        .snapshot
        .ops
        .values()
        .any(|op| op.get("call").is_some())
    {
        return Err(GraphError::Unsupported(
            "Op.call is not admitted by the Graph validator yet".into(),
        ));
    }
    for region in record.snapshot.parallel_regions()?.values() {
        if region
            .branches
            .iter()
            .flatten()
            .any(|node| !scope_of(node).is_empty())
        {
            return Err(GraphError::Unsupported(
                "module scopes inside parallel branches are not admitted by the Graph validator yet"
                    .into(),
            ));
        }
    }
    let state = serde_json::to_value(record).map_err(GraphError::SnapshotDecode)?;
    Ok(Resource::new(format!("run/{}", record.run_id), GRAPH_VALIDATOR, state).task(budget))
}

pub fn record_of(resource: &Resource) -> Result<GraphRunRecord, GraphError> {
    serde_json::from_value(resource.state.clone()).map_err(GraphError::SnapshotDecode)
}

/// Committed inputs of a dispatch: the selected incoming edges' source results.
fn dispatch_inputs(record: &GraphRunRecord, node_id: &str) -> Vec<CommitRef> {
    record
        .snapshot
        .edges
        .iter()
        .filter(|e| e.to_node == node_id)
        .filter_map(|e| {
            let decision = record
                .decided
                .get(&edge_key(&e.from_node, &e.to_node))
                .filter(|d| d.selected)?;
            let sequence = if decision.result_sequence == 0 {
                decision.sequence
            } else {
                decision.result_sequence
            };
            record
                .results
                .get(&e.from_node)?
                .iter()
                .find(|r| r.sequence == sequence)
                .map(|r| r.commit.clone())
        })
        .collect()
}

fn op_of<'a>(record: &'a GraphRunRecord, node: &GraphNode) -> Option<&'a Value> {
    node.op
        .as_ref()
        .and_then(|name| record.snapshot.ops.get(name))
}

/// Advance as far as the Graph allows, applying round limits and skipped
/// paths on the way. Mutates exactly what the Runner mutated before dispatch.
pub(crate) fn prepare(record: &mut GraphRunRecord) -> Result<Advance, GraphError> {
    loop {
        propagate_inactive_edges(record)?;
        if let Some(activation) = &record.parallel
            && activation
                .branches
                .iter()
                .any(|b| b.status != ParallelBranchStatus::Completed)
        {
            return parallel_frontier(record);
        }
        let Some(node) = next_node(record)? else {
            return Ok(Advance::Done);
        };
        let scope = scope_of(&node.id);
        let prior_sequence = record
            .results
            .get(&node.id)
            .and_then(|results| results.last())
            .map(|r| r.sequence)
            .unwrap_or(0);
        let enters_scope = !scope.is_empty()
            && record
                .snapshot
                .edges
                .iter()
                .filter(|e| e.to_node == node.id && !e.from_node.starts_with(&format!("{scope}/")))
                .any(|e| {
                    record
                        .decided
                        .get(&edge_key(&e.from_node, &e.to_node))
                        .is_some_and(|d| d.selected && d.sequence > prior_sequence)
                });
        if enters_scope && let Some(limit) = record.snapshot.module_rounds.get(&scope).copied() {
            let next = record.module_activations.get(&scope).copied().unwrap_or(0) + 1;
            if next > u64::from(limit) {
                record.ceased.insert(format!("{scope}@{limit}"));
                refuse_module_activation(record, &scope, &node.id)?;
                continue;
            }
            record.module_activations.insert(scope.clone(), next);
            let members = record
                .snapshot
                .nodes
                .iter()
                .filter(|m| m.id.starts_with(&format!("{scope}/")))
                .map(|m| m.id.clone())
                .collect::<Vec<_>>();
            for member in members {
                record.passes.remove(&member);
            }
            let nested = record
                .module_activations
                .keys()
                .filter(|n| n.starts_with(&format!("{scope}/")))
                .cloned()
                .collect::<Vec<_>>();
            for n in nested {
                record.module_activations.remove(&n);
            }
        }
        if let Some(limit) = node.max_rounds
            && record.passes.get(&node.id).copied().unwrap_or(0) >= u64::from(limit)
        {
            record.ceased.insert(format!("{}@{limit}", node.id));
            refuse_outgoing(record, &node.id);
            continue;
        }
        let invocation = record.invocations.get(&node.id).copied().unwrap_or(0) + 1;
        let op = op_of(record, &node);
        let is_join = op.is_some_and(|op| op.get("join").is_some())
            && record
                .parallel
                .as_ref()
                .is_some_and(|a| a.join_node == node.id);
        let (kind, inputs) = if op.is_some_and(|op| op.get("fanout").is_some()) {
            (DispatchKind::Fanout, dispatch_inputs(record, &node.id))
        } else if is_join {
            let inputs = record
                .parallel
                .as_ref()
                .expect("join activation")
                .branches
                .iter()
                .flat_map(|b| b.completed.iter().cloned())
                .collect();
            (DispatchKind::Join, inputs)
        } else {
            (DispatchKind::Work, dispatch_inputs(record, &node.id))
        };
        return Ok(Advance::Open(vec![Dispatch {
            node,
            invocation,
            kind,
            branch: None,
            inputs,
        }]));
    }
}

/// One slot per unfinished branch: the branch's next node.
fn parallel_frontier(record: &mut GraphRunRecord) -> Result<Advance, GraphError> {
    let activation = record.parallel.clone().expect("active parallel region");
    let mut open = Vec::new();
    for (index, branch) in activation.branches.iter().enumerate() {
        if branch.status == ParallelBranchStatus::Completed {
            continue;
        }
        let node_id = &branch.nodes[branch.next_index];
        let node = record
            .snapshot
            .nodes
            .iter()
            .find(|n| &n.id == node_id)
            .cloned()
            .expect("validated parallel branch node");
        if let Some(limit) = node.max_rounds
            && record.passes.get(&node.id).copied().unwrap_or(0) >= u64::from(limit)
        {
            record.ceased.insert(format!("{}@{limit}", node.id));
            refuse_outgoing(record, &node.id);
            return Ok(Advance::Failed(format!(
                "parallel branch `{}` exceeded node round ceiling `{limit}`",
                node.id
            )));
        }
        let invocation = record.invocations.get(&node.id).copied().unwrap_or(0) + 1;
        let inputs = dispatch_inputs(record, &node.id);
        open.push(Dispatch {
            node,
            invocation,
            kind: DispatchKind::Work,
            branch: Some(index),
            inputs,
        });
    }
    Ok(Advance::Open(open))
}

fn refuse_outgoing(record: &mut GraphRunRecord, node_id: &str) {
    record.sequence += 1;
    let invocation = record.invocations.get(node_id).copied().unwrap_or(0);
    let targets = record
        .snapshot
        .edges
        .iter()
        .filter(|e| e.from_node == node_id)
        .map(|e| e.to_node.clone())
        .collect::<Vec<_>>();
    for target in targets {
        record.decided.insert(
            edge_key(node_id, &target),
            EdgeDecision {
                selected: false,
                sequence: record.sequence,
                source_invocation: invocation,
                result_sequence: 0,
            },
        );
    }
}

impl GraphValidator {
    /// The dispatches the Run would currently accept, computed without
    /// mutating the stored state (the same computation reruns when judged).
    pub fn frontier(resource: &Resource) -> Result<Vec<Dispatch>, GraphError> {
        let mut record = record_of(resource)?;
        Ok(match prepare(&mut record)? {
            Advance::Open(open) => open,
            Advance::Done | Advance::Failed(_) => Vec::new(),
        })
    }
}

impl Validator for GraphValidator {
    fn open(&self, resource: &Resource) -> Vec<Slot> {
        Self::frontier(resource)
            .unwrap_or_default()
            .iter()
            .map(|d| Slot {
                id: d.slot(),
                action: Action::Write,
            })
            .collect()
    }

    fn judge(&self, ctx: &JudgeCtx, s: &Submission) -> Judgement {
        let Ok(mut record) = record_of(ctx.resource) else {
            return Judgement::refused(Reason::Unclassified);
        };
        let Ok(Advance::Open(open)) = prepare(&mut record) else {
            return Judgement::refused(Reason::SlotClosed);
        };
        let Some(dispatch) = open
            .into_iter()
            .find(|d| s.slot.as_deref() == Some(d.slot().as_str()))
        else {
            return Judgement::refused(Reason::SlotClosed);
        };
        if s.action != Action::Write {
            return Judgement::refused(Reason::SlotClosed);
        }
        // Control nodes are deterministic host code; no proposer can fake one.
        if dispatch.kind != DispatchKind::Work && ctx.principal.kind != PrincipalKind::Code {
            return Judgement::refused(Reason::Unauthorized);
        }
        let Ok(content) = serde_json::from_value::<NodeSubmission>(s.content.clone()) else {
            return Judgement::returned(Reason::ContentInvalid);
        };
        let node_id = dispatch.node.id.clone();
        if content.commit.node_id != node_id || content.commit.invocation != dispatch.invocation {
            return Judgement::returned(Reason::EvidenceMissing);
        }
        let completion = NodeCompletion {
            submission: content.summary,
            route: content.route,
            model_requests: content.model_requests,
            output: content.output,
        };
        let routes = record
            .snapshot
            .edges
            .iter()
            .filter(|e| e.from_node == node_id)
            .map(|e| e.to_node.clone())
            .collect::<Vec<_>>();
        if dispatch.branch.is_some() && routes.len() != 1 {
            return Judgement::refused(Reason::RouteInvalid);
        }
        let chosen = if dispatch.kind == DispatchKind::Fanout {
            None
        } else {
            // An invalid route is fixable by the proposer: returned, not a failed Run.
            match select_route(&completion, &routes) {
                Ok(route) => route,
                Err(_) => return Judgement::returned(Reason::RouteInvalid),
            }
        };
        record
            .invocations
            .insert(node_id.clone(), dispatch.invocation);
        *record.passes.entry(node_id.clone()).or_default() += 1;
        record.sequence += 1;
        let result_sequence = record.sequence;
        let commit = content.commit.clone();
        record
            .results
            .entry(node_id.clone())
            .or_default()
            .push(RunResult {
                node_id: node_id.clone(),
                key: InvocationKey {
                    run_id: record.run_id.clone(),
                    graph_digest: record.graph_digest.clone(),
                    node_id: node_id.clone(),
                    invocation: dispatch.invocation,
                },
                plugin_views: content.plugin_views,
                plugin_digests: content.plugin_digests,
                completion,
                interruption: None,
                commit: commit.clone(),
                sequence: result_sequence,
            });
        record.sequence += 1;
        for target in &routes {
            record.decided.insert(
                edge_key(&node_id, target),
                EdgeDecision {
                    selected: dispatch.kind == DispatchKind::Fanout
                        || Some(target) == chosen.as_ref(),
                    sequence: record.sequence,
                    source_invocation: dispatch.invocation,
                    result_sequence,
                },
            );
        }
        match (dispatch.kind, dispatch.branch) {
            (DispatchKind::Fanout, _) => {
                let Ok(regions) = record.snapshot.parallel_regions() else {
                    return Judgement::refused(Reason::Unclassified);
                };
                let region = regions[&node_id].clone();
                let activation_id =
                    format!("{}:{}:{}", record.run_id, node_id, dispatch.invocation);
                record.parallel = Some(ParallelActivation {
                    activation_id: activation_id.clone(),
                    fanout_node: node_id.clone(),
                    join_node: region.join,
                    fanout_invocation: dispatch.invocation,
                    branches: region
                        .branches
                        .into_iter()
                        .enumerate()
                        .map(|(index, nodes)| ParallelBranchRecord {
                            branch_id: format!("{activation_id}:branch:{index}"),
                            entry: nodes[0].clone(),
                            nodes,
                            next_index: 0,
                            status: ParallelBranchStatus::Ready,
                            cursor: None,
                            completed: Vec::new(),
                            error: None,
                        })
                        .collect(),
                });
            }
            (DispatchKind::Join, _) => record.parallel = None,
            (DispatchKind::Work, Some(index)) => {
                let branch = &mut record.parallel.as_mut().expect("parallel").branches[index];
                branch.completed.push(commit);
                branch.next_index += 1;
                branch.status = if branch.next_index == branch.nodes.len() {
                    ParallelBranchStatus::Completed
                } else {
                    ParallelBranchStatus::Ready
                };
            }
            (DispatchKind::Work, None) => {}
        }
        // Look ahead without persisting dispatch-time effects for the next
        // node; persist the look-ahead only when it shows the Run is over.
        let mut ahead = record.clone();
        let next = prepare(&mut ahead);
        let verification = if s
            .evidence
            .iter()
            .any(|e| matches!(e, Evidence::Resource { .. }))
        {
            Verification::Evidenced
        } else {
            Verification::Asserted
        };
        let (record, phase) = match next {
            Ok(Advance::Open(_)) => (record, None),
            Ok(Advance::Done) if ahead.ceased.is_empty() => (
                ahead,
                Some(Phase::Settled {
                    result: Outcome::Done,
                }),
            ),
            Ok(Advance::Done) => {
                let detail = format!(
                    "round limit reached: {}",
                    ahead.ceased.iter().cloned().collect::<Vec<_>>().join(", ")
                );
                (
                    ahead,
                    Some(Phase::Settled {
                        result: Outcome::Escalated { detail },
                    }),
                )
            }
            Ok(Advance::Failed(detail)) => (
                ahead,
                Some(Phase::Settled {
                    result: Outcome::Failed {
                        reason: Reason::Unclassified,
                        detail,
                    },
                }),
            ),
            Err(_) => return Judgement::refused(Reason::Unclassified),
        };
        let Ok(state) = serde_json::to_value(&record) else {
            return Judgement::refused(Reason::Unclassified);
        };
        let judgement = Judgement::accept(state, verification);
        match phase {
            Some(phase) => judgement.with_phase(phase),
            None => judgement,
        }
    }
}

#[cfg(test)]
mod differential;
