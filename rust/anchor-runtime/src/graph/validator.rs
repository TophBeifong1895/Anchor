//! A Graph as the validator of a Run.
//!
//! The Run is a kernel task resource whose state is the frozen Graph and its
//! progress. The validator never calls anyone: [`open`](GraphValidator::open)
//! says which node invocation it would currently accept, and
//! [`judge`](GraphValidator::judge) applies a submitted result, chooses the
//! route and decides whether the Run has settled. Stop, pause, resume and
//! abandon are not here at all: they are the kernel's task lifecycle.
//!
//! This first slice covers serial routing, feedback edges, skipped paths,
//! node and module round limits. Fanout/join and Op.call are not admitted yet.

use anchor_kernel::{
    Action, Evidence, JudgeCtx, Judgement, Outcome, Phase, Reason, Resource, Slot, Submission,
    Validator, Verification,
};

use super::*;

/// The validator id a host installs this under.
pub const GRAPH_VALIDATOR: &str = "validator/graph";

pub struct GraphValidator;

/// The node invocation a Run would currently accept.
#[derive(Debug, Clone, PartialEq)]
pub struct Dispatch {
    pub node: GraphNode,
    pub invocation: u64,
    /// Committed inputs, computed after the same advance that chose the node:
    /// a stale selected edge retired by that advance is not an input.
    pub inputs: Vec<CommitRef>,
}

impl Dispatch {
    pub fn slot(&self) -> String {
        format!("{}#{}", self.node.id, self.invocation)
    }
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
        .parallel_regions()?
        .values()
        .next()
        .is_some()
        || record
            .snapshot
            .ops
            .values()
            .any(|op| op.get("call").is_some())
    {
        return Err(GraphError::Unsupported(
            "fanout/join and Op.call are not admitted by the Graph validator yet".into(),
        ));
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

/// Advance to the next dispatchable node, applying round limits and skipped
/// paths on the way. Mutates exactly what the Runner mutated before dispatch.
pub(crate) fn prepare(record: &mut GraphRunRecord) -> Result<Option<Dispatch>, GraphError> {
    loop {
        propagate_inactive_edges(record)?;
        let Some(node) = next_node(record)? else {
            return Ok(None);
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
        let inputs = dispatch_inputs(record, &node.id);
        return Ok(Some(Dispatch {
            node,
            invocation,
            inputs,
        }));
    }
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
    /// The dispatch the Run would currently accept, computed without mutating
    /// the stored state (the same computation reruns when it is judged).
    pub fn current(resource: &Resource) -> Result<Option<Dispatch>, GraphError> {
        let mut record = record_of(resource)?;
        prepare(&mut record)
    }
}

impl Validator for GraphValidator {
    fn open(&self, resource: &Resource) -> Vec<Slot> {
        match Self::current(resource) {
            Ok(Some(dispatch)) => vec![Slot {
                id: dispatch.slot(),
                action: Action::Write,
            }],
            _ => Vec::new(),
        }
    }

    fn judge(&self, ctx: &JudgeCtx, s: &Submission) -> Judgement {
        let Ok(mut record) = record_of(ctx.resource) else {
            return Judgement::refused(Reason::Unclassified);
        };
        let Ok(Some(dispatch)) = prepare(&mut record) else {
            return Judgement::refused(Reason::SlotClosed);
        };
        if s.action != Action::Write || s.slot.as_deref() != Some(dispatch.slot().as_str()) {
            return Judgement::refused(Reason::SlotClosed);
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
        // An invalid route is fixable by the proposer: returned, not a failed Run.
        let Ok(chosen) = select_route(&completion, &routes) else {
            return Judgement::returned(Reason::RouteInvalid);
        };
        record
            .invocations
            .insert(node_id.clone(), dispatch.invocation);
        *record.passes.entry(node_id.clone()).or_default() += 1;
        record.sequence += 1;
        let result_sequence = record.sequence;
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
                commit: content.commit,
                sequence: result_sequence,
            });
        record.sequence += 1;
        for target in routes {
            record.decided.insert(
                edge_key(&node_id, &target),
                EdgeDecision {
                    selected: Some(&target) == chosen.as_ref(),
                    sequence: record.sequence,
                    source_invocation: dispatch.invocation,
                    result_sequence,
                },
            );
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
            Ok(Some(_)) => (record, None),
            Ok(None) if ahead.ceased.is_empty() => (
                ahead,
                Some(Phase::Settled {
                    result: Outcome::Done,
                }),
            ),
            Ok(None) => {
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
