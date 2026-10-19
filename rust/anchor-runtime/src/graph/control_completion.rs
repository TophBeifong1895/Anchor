//! Canonical deterministic control completions shared by both execution adapters.
use super::*;

pub(super) fn fanout_completion(
    record: &GraphRunRecord,
    key: &InvocationKey,
) -> Result<NodeCompletion, GraphError> {
    let region = record
        .snapshot
        .parallel_regions()?
        .get(&key.node_id)
        .cloned()
        .expect("admitted fanout region");
    let activation_id = format!("{}:{}:{}", record.run_id, key.node_id, key.invocation);
    Ok(NodeCompletion {
        submission: format!("fanout activation {activation_id}"),
        route: None,
        model_requests: 0,
        output: serde_json::json!({
            "activation_id":activation_id,
            "fanout":region.fanout,
            "join":region.join,
            "branches":region.branches,
        }),
    })
}

pub(super) fn join_completion(record: &GraphRunRecord) -> NodeCompletion {
    let activation = record.parallel.as_ref().expect("paired join activation");
    let branches = activation
        .branches
        .iter()
        .map(|branch| {
            let nodes = branch
                .completed
                .iter()
                .map(|commit| {
                    let result = record
                        .results
                        .get(&commit.node_id)
                        .and_then(|results| results.iter().find(|result| result.commit == *commit))
                        .expect("activation validation binds branch result");
                    serde_json::json!({"node":commit.node_id,"commit":commit,
                                "files":[],"summary":result.completion.submission,
                                "output":result.completion.output})
                })
                .collect::<Vec<_>>();
            serde_json::json!({"branch_id":branch.branch_id,"entry":branch.entry,
                            "output":branch.nodes.last(),"status":"completed","nodes":nodes})
        })
        .collect::<Vec<_>>();
    let manifest = serde_json::json!({"activation_id":activation.activation_id,
                    "fanout":activation.fanout_node,"join":activation.join_node,"branches":branches});
    NodeCompletion {
        submission: serde_json::to_string(&manifest).unwrap_or_default(),
        route: None,
        model_requests: 0,
        output: manifest,
    }
}
