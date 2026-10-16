//! Differential test: the old push-model `GraphRunner` against the kernel with
//! `GraphValidator`, on thousands of random Graphs and route scripts.
//!
//! Both sides see the same Graph and the same deterministic route choices.
//! They must dispatch the same node invocations, in the same order, with the
//! same committed inputs, and end with the same Graph facts.

use std::sync::{Arc, Mutex};

use anchor_kernel::{
    Action, Authority, Evidence, Kernel, MemoryStore, Outcome, Phase, Principal, PrincipalKind,
    Submission,
};

use super::super::*;
use super::{GRAPH_VALIDATOR, GraphValidator, NodeSubmission, record_of, run_resource};

// ---------------------------------------------------------------------------
// Deterministic randomness
// ---------------------------------------------------------------------------

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

fn hash(parts: &[&str], seed: u64) -> u64 {
    let mut rng = Rng(seed);
    for part in parts {
        for byte in part.bytes() {
            rng.0 ^= u64::from(byte);
            rng.next();
        }
    }
    rng.next()
}

/// The route a node picks on a given invocation. Same answer on both sides.
fn route_for(seed: u64, node: &str, invocation: u64, routes: &[String]) -> Option<String> {
    let h = hash(&[node, &invocation.to_string()], seed);
    match routes.len() {
        0 => None,
        1 if h.is_multiple_of(2) => None,
        _ => Some(routes[(h % routes.len() as u64) as usize].clone()),
    }
}

fn random_graph(seed: u64) -> GraphSnapshot {
    let mut rng = Rng(seed);
    let n = 2 + rng.below(8) as usize;
    let module = n >= 4 && rng.chance(40);
    let ids: Vec<String> = (0..n)
        .map(|i| {
            if module && (1..=2).contains(&i) {
                format!("m/n{i}")
            } else {
                format!("n{i}")
            }
        })
        .collect();
    let mut edges = Vec::new();
    for i in 0..n {
        for j in 0..n {
            let p = if i == j {
                5
            } else if j > i {
                45
            } else {
                20
            };
            if rng.chance(p) {
                edges.push(GraphEdge {
                    from_node: ids[i].clone(),
                    to_node: ids[j].clone(),
                });
            }
        }
    }
    let agents = BTreeMap::from([(
        "worker".to_string(),
        AgentDefinition {
            model: "m".into(),
            instructions: "base".into(),
            network: false,
            max_steps: None,
            wall_time_limit_seconds: None,
            reads: vec![],
            writes: vec![],
        },
    )]);
    let nodes = ids
        .iter()
        .map(|id| GraphNode {
            id: id.clone(),
            agent: Some("worker".into()),
            op: None,
            input: None,
            plugins: vec![],
            // Bounded passes on every node: both sides must terminate.
            max_rounds: Some(1 + rng.below(4) as u32),
        })
        .collect();
    let mut module_rounds = BTreeMap::new();
    if module {
        module_rounds.insert("m".to_string(), 1 + rng.below(2) as u32);
    }
    GraphSnapshot {
        objective: "differential".into(),
        input: serde_json::json!({}),
        entry: ids[0].clone(),
        agents,
        ops: BTreeMap::new(),
        nodes,
        edges,
        module_rounds,
    }
}

// ---------------------------------------------------------------------------
// Old side: GraphRunner with scripted ports
// ---------------------------------------------------------------------------

#[derive(Default)]
struct MemStore(Mutex<Option<GraphRunRecord>>);
struct Lease;
impl RunLease for Lease {}
impl RunStore for MemStore {
    fn load(&self, id: &str) -> Result<Option<GraphRunRecord>, GraphError> {
        Ok(self.0.lock().unwrap().clone().filter(|r| r.run_id == id))
    }
    fn save(&self, r: &GraphRunRecord) -> Result<(), GraphError> {
        *self.0.lock().unwrap() = Some(r.clone());
        Ok(())
    }
    fn acquire_lease(&self, _: &str) -> Result<Box<dyn RunLease>, GraphError> {
        Ok(Box::new(Lease))
    }
}

struct Artifacts;
impl ArtifactPort for Artifacts {
    fn freeze<'a>(
        &'a self,
        key: &'a InvocationKey,
        _c: &'a NodeCompletion,
    ) -> Pin<Box<dyn Future<Output = Result<CommitRef, GraphError>> + Send + 'a>> {
        Box::pin(async move { Ok(commit_of(key)) })
    }
    fn resolve<'a>(
        &'a self,
        c: &'a CommitRef,
    ) -> Pin<Box<dyn Future<Output = Result<Value, GraphError>> + Send + 'a>> {
        Box::pin(async move { Ok(serde_json::json!({"commit": c.id})) })
    }
}

fn commit_of(key: &InvocationKey) -> CommitRef {
    CommitRef {
        id: key.durable_key(),
        node_id: key.node_id.clone(),
        invocation: key.invocation,
    }
}

type Trace = Vec<(String, u64, Vec<String>)>;

struct Scripted {
    seed: u64,
    trace: Mutex<Trace>,
}
impl NodeExecutionPort for Scripted {
    fn capabilities(&self) -> NodeExecutionCapabilities {
        NodeExecutionCapabilities {
            agent: true,
            op_run: true,
            host_operations: false,
            exact_provider_request_budget: true,
        }
    }
    fn completion_fact<'a>(
        &'a self,
        _key: &'a InvocationKey,
    ) -> Pin<Box<dyn Future<Output = Result<CompletionFact, GraphError>> + Send + 'a>> {
        Box::pin(async { Ok(CompletionFact::NotStarted) })
    }
    fn execute<'a>(
        &'a self,
        request: NodeExecutionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<NodeExecutionOutcome, GraphError>> + Send + 'a>> {
        Box::pin(async move {
            let key = request.key.clone();
            self.trace.lock().unwrap().push((
                key.node_id.clone(),
                key.invocation,
                request.input_commits.iter().map(|c| c.id.clone()).collect(),
            ));
            Ok(NodeExecutionOutcome::Completed(NodeCompletion {
                submission: format!("{} #{}", key.node_id, key.invocation),
                route: route_for(self.seed, &key.node_id, key.invocation, &request.routes),
                model_requests: 1,
                output: serde_json::json!({"node": key.node_id, "invocation": key.invocation}),
            }))
        })
    }
}

struct NoControl(crate::Cancellation);
impl RunControl for NoControl {
    fn pause_requested(&self) -> bool {
        false
    }
    fn stop_requested(&self) -> bool {
        false
    }
    fn cancellation(&self) -> crate::Cancellation {
        self.0.clone()
    }
}

async fn run_old(record: GraphRunRecord, seed: u64) -> (Trace, GraphRunRecord) {
    let store = MemStore::default();
    let nodes = Scripted {
        seed,
        trace: Mutex::default(),
    };
    let control = NoControl(Arc::new(std::sync::atomic::AtomicBool::new(false)));
    let done = GraphRunner::new(&store, &Artifacts, &nodes, &control)
        .run(record)
        .await
        .expect("old runner");
    (nodes.trace.into_inner().unwrap(), done)
}

// ---------------------------------------------------------------------------
// New side: kernel + GraphValidator + a trivial scheduler
// ---------------------------------------------------------------------------

struct AllowAll;
impl Authority for AllowAll {
    fn permits(&self, _: &Principal, _: Action, _: &str) -> bool {
        true
    }
}

fn run_new(record: GraphRunRecord, seed: u64) -> (Trace, GraphRunRecord, Phase) {
    let mut kernel = Kernel::new(MemoryStore::default(), Box::new(AllowAll)).unwrap();
    kernel
        .install(GRAPH_VALIDATOR, Arc::new(GraphValidator))
        .unwrap();
    let resource = run_resource(&record, u64::MAX).unwrap();
    let id = resource.id.clone();
    kernel.create(resource).unwrap();
    let agent = Principal::new("executor", PrincipalKind::Agent);
    let mut trace = Trace::new();
    for _ in 0..10_000 {
        let slots = kernel.open(&id).unwrap();
        let Some(slot) = slots.first() else { break };
        let resource = kernel.load(&id).unwrap().unwrap();
        let state = record_of(&resource).unwrap();
        let dispatch = GraphValidator::current(&resource)
            .unwrap()
            .expect("open slot");
        assert_eq!(slot.id, dispatch.slot());
        let node = dispatch.node.id.clone();
        let inputs = dispatch.inputs.clone();
        trace.push((
            node.clone(),
            dispatch.invocation,
            inputs.iter().map(|c| c.id.clone()).collect(),
        ));
        let routes = state
            .snapshot
            .edges
            .iter()
            .filter(|e| e.from_node == node)
            .map(|e| e.to_node.clone())
            .collect::<Vec<_>>();
        let commit = commit_of(&InvocationKey {
            run_id: state.run_id.clone(),
            graph_digest: state.graph_digest.clone(),
            node_id: node.clone(),
            invocation: dispatch.invocation,
        });
        let content = NodeSubmission {
            summary: format!("{node} #{}", dispatch.invocation),
            route: route_for(seed, &node, dispatch.invocation, &routes),
            output: serde_json::json!({"node": node, "invocation": dispatch.invocation}),
            model_requests: 1,
            commit: commit.clone(),
            plugin_views: BTreeMap::new(),
            plugin_digests: BTreeMap::new(),
        };
        let verdict = kernel
            .submit(
                &agent,
                1,
                Submission::new(&id, resource.version, Action::Write)
                    .slot(slot.id.clone())
                    .content(serde_json::to_value(content).unwrap())
                    .evidence(Evidence::Resource {
                        id: commit.id,
                        version: 0,
                    }),
            )
            .unwrap();
        assert!(verdict.is_accepted(), "{verdict:?}");
    }
    let resource = kernel.load(&id).unwrap().unwrap();
    let phase = resource.phase().cloned().unwrap();
    (trace, record_of(&resource).unwrap(), phase)
}

/// (invocation, sequence, route, commit id) of one result.
type ResultFact = (u64, u64, Option<String>, String);

/// The Graph facts both models must agree on.
#[derive(Debug, PartialEq)]
struct Facts {
    results: BTreeMap<String, Vec<ResultFact>>,
    decided: BTreeMap<String, EdgeDecision>,
    ceased: BTreeSet<String>,
    invocations: BTreeMap<String, u64>,
    passes: BTreeMap<String, u64>,
    module_activations: BTreeMap<String, u64>,
    sequence: u64,
}

fn facts(r: &GraphRunRecord) -> Facts {
    Facts {
        results: r
            .results
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    v.iter()
                        .map(|x| {
                            (
                                x.key.invocation,
                                x.sequence,
                                x.completion.route.clone(),
                                x.commit.id.clone(),
                            )
                        })
                        .collect(),
                )
            })
            .collect(),
        decided: r.decided.clone(),
        ceased: r.ceased.clone(),
        invocations: r.invocations.clone(),
        passes: r.passes.clone(),
        module_activations: r.module_activations.clone(),
        sequence: r.sequence,
    }
}

#[derive(Default, Debug)]
struct Tally {
    cases: u64,
    rejected_graphs: u64,
    completed: u64,
    round_limited: u64,
    dispatches: u64,
    longest: usize,
    module_activated: u64,
    module_limited: u64,
    skipped_paths: u64,
}

async fn check(seed: u64, tally: &mut Tally) {
    let snapshot = random_graph(seed);
    let Ok(record) =
        GraphRunRecord::create_with_id(snapshot.clone(), serde_json::json!({}), format!("r{seed}"))
    else {
        tally.rejected_graphs += 1;
        return;
    };
    let (old_trace, old) = run_old(record.clone(), seed).await;
    let (new_trace, new, phase) = run_new(record, seed);
    assert_eq!(
        old_trace, new_trace,
        "dispatch trace differs, seed {seed}, graph {snapshot:#?}"
    );
    assert_eq!(facts(&old), facts(&new), "Graph facts differ, seed {seed}");
    match (old.status, &phase) {
        (
            RunStatus::Completed,
            Phase::Settled {
                result: Outcome::Done,
            },
        ) => tally.completed += 1,
        (
            RunStatus::Stopped,
            Phase::Settled {
                result: Outcome::Escalated { .. },
            },
        ) if !old.ceased.is_empty() => tally.round_limited += 1,
        other => panic!("terminal state differs, seed {seed}: {other:?}"),
    }
    tally.cases += 1;
    tally.module_activated += u64::from(!new.module_activations.is_empty());
    tally.module_limited += u64::from(new.ceased.iter().any(|c| c.starts_with("m@")));
    tally.skipped_paths += u64::from(
        new.snapshot
            .nodes
            .iter()
            .any(|n| !new.invocations.contains_key(&n.id)),
    );
    tally.dispatches += new_trace.len() as u64;
    tally.longest = tally.longest.max(new_trace.len());
}

#[tokio::test]
async fn kernel_with_graph_validator_matches_the_runner_on_random_graphs() {
    let mut tally = Tally::default();
    for seed in 0..10_000 {
        check(seed, &mut tally).await;
    }
    eprintln!("differential: {tally:?}");
    assert!(tally.cases > 9_000, "{tally:?}");
    assert!(
        tally.module_activated > 500 && tally.module_limited > 50 && tally.skipped_paths > 1000,
        "{tally:?}"
    );
    assert!(
        tally.completed > 100 && tally.round_limited > 100,
        "both endings exercised: {tally:?}"
    );
}
