//! Deterministic Graph fixtures shared by Runtime differential and Host probe tests.
use super::*;
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Deterministic randomness
// ---------------------------------------------------------------------------

pub struct Rng(u64);
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

pub fn hash(parts: &[&str], seed: u64) -> u64 {
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
pub fn route_for(seed: u64, node: &str, invocation: u64, routes: &[String]) -> Option<String> {
    let h = hash(&[node, &invocation.to_string()], seed);
    match routes.len() {
        0 => None,
        1 if h.is_multiple_of(2) => None,
        _ => Some(routes[(h % routes.len() as u64) as usize].clone()),
    }
}

pub fn failure_for(seed: u64, node: &str, invocation: u64) -> Option<String> {
    (hash(&[node, &invocation.to_string(), "failure"], seed) % 100 < 3)
        .then(|| format!("fixture failure {node} #{invocation}"))
}

pub fn random_graph(seed: u64) -> GraphSnapshot {
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

/// prefix → fanout → 2–3 linear branches → join → suffix, with random back
/// edges from the suffix that re-enter the whole region, skip edges around
/// it, and round limits everywhere (so branches can hit their ceiling).
pub fn random_parallel_graph(seed: u64) -> GraphSnapshot {
    let mut snapshot = random_graph(seed ^ 0xA5A5);
    let mut rng = Rng(seed);
    let prefix = 1 + rng.below(2) as usize;
    let branches = 2 + rng.below(2) as usize;
    let suffix = rng.below(3) as usize;
    let mut ids = Vec::new();
    let p: Vec<String> = (0..prefix).map(|i| format!("p{i}")).collect();
    let b: Vec<Vec<String>> = (0..branches)
        .map(|i| {
            (0..1 + rng.below(3) as usize)
                .map(|j| format!("b{i}_{j}"))
                .collect()
        })
        .collect();
    let s: Vec<String> = (0..suffix).map(|i| format!("s{i}")).collect();
    ids.extend(p.iter().cloned());
    ids.push("f".into());
    ids.extend(b.iter().flatten().cloned());
    ids.push("j".into());
    ids.extend(s.iter().cloned());
    let edge = |a: &str, z: &str| GraphEdge {
        from_node: a.into(),
        to_node: z.into(),
    };
    let mut edges = Vec::new();
    for w in p.windows(2) {
        edges.push(edge(&w[0], &w[1]));
    }
    edges.push(edge(p.last().unwrap(), "f"));
    for chain in &b {
        edges.push(edge("f", &chain[0]));
        for w in chain.windows(2) {
            edges.push(edge(&w[0], &w[1]));
        }
        edges.push(edge(chain.last().unwrap(), "j"));
    }
    if let Some(first) = s.first() {
        edges.push(edge("j", first));
    }
    for (i, a) in s.iter().enumerate() {
        for (k, z) in s.iter().enumerate() {
            if k > i && rng.chance(50) {
                edges.push(edge(a, z));
            }
        }
        for z in &p {
            if rng.chance(30) {
                edges.push(edge(a, z));
            }
        }
    }
    if let (Some(a), Some(z)) = (p.first(), s.last())
        && rng.chance(30)
    {
        edges.push(edge(a, z));
    }
    snapshot.entry = p[0].clone();
    snapshot.module_rounds.clear();
    snapshot.ops = BTreeMap::from([
        (
            "fan".to_string(),
            serde_json::json!({"fanout": {"join": "j"}}),
        ),
        ("join".to_string(), serde_json::json!({"join": {}})),
    ]);
    snapshot.nodes = ids
        .iter()
        .map(|id| GraphNode {
            id: id.clone(),
            agent: (id != "f" && id != "j").then(|| "worker".into()),
            op: match id.as_str() {
                "f" => Some("fan".into()),
                "j" => Some("join".into()),
                _ => None,
            },
            input: None,
            plugins: vec![],
            max_rounds: Some(1 + rng.below(3) as u32),
        })
        .collect();
    snapshot.edges = edges;
    snapshot
}

/// Keep the original parallel population unchanged and additionally exercise
/// shared and independent module entries. A module's ceiling is deliberately
/// above the bounded fanout's maximum activations; ceiling failures have
/// separate deterministic coverage with the exact S17/S18 history assertions.
pub fn random_parallel_module_graph(seed: u64) -> GraphSnapshot {
    let mut snapshot = random_parallel_graph(seed);
    let renamed = snapshot
        .nodes
        .iter()
        .filter(|node| node.id.starts_with('b'))
        .map(|node| {
            let scope = if seed.is_multiple_of(2) {
                "m".to_owned()
            } else {
                format!("m{}", node.id.split('_').next().unwrap())
            };
            (node.id.clone(), format!("{scope}/{}", node.id))
        })
        .collect::<BTreeMap<_, _>>();
    for node in &mut snapshot.nodes {
        if let Some(id) = renamed.get(&node.id) {
            node.id = id.clone();
            snapshot.module_rounds.insert(scope_of(id), 100);
        }
    }
    for edge in &mut snapshot.edges {
        if let Some(id) = renamed.get(&edge.from_node) {
            edge.from_node = id.clone();
        }
        if let Some(id) = renamed.get(&edge.to_node) {
            edge.to_node = id.clone();
        }
    }
    snapshot
}

/// What a child Run does, decided by the call identity on both sides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallScript {
    /// wait: admitted, waited on, then completes.
    WaitThenComplete,
    /// wait or detach: the port answers with a completed child at once.
    CompleteAtOnce,
    /// detach: accepted.
    Detached,
    Failed,
    Uncertain,
}

pub fn call_script(key: &str, mode: &str, seed: u64) -> CallScript {
    let h = hash(&[key], seed) % 100;
    match (mode, h) {
        ("wait", 0..50) => CallScript::WaitThenComplete,
        ("wait", 50..75) => CallScript::CompleteAtOnce,
        ("detach", 0..60) => CallScript::Detached,
        ("detach", 60..80) => CallScript::CompleteAtOnce,
        (_, 80..90) | ("wait", 75..80) => CallScript::Failed,
        _ => CallScript::Uncertain,
    }
}

pub fn child_of(key: &str) -> String {
    format!("child-{}", &key[..12])
}

pub fn child_output(key: &str, seed: u64) -> Value {
    serde_json::json!({"answer": hash(&[key, "out"], seed) % 1000, "secret": "child business result"})
}

/// Random serial Graphs with one or two nodes turned into Op.call (wait or
/// detach), each keeping at most one exit.
pub fn random_call_graph(seed: u64) -> GraphSnapshot {
    let mut snapshot = random_graph(seed ^ 0x5A5A);
    let mut rng = Rng(seed);
    snapshot.ops = BTreeMap::from([
        (
            "callw".to_string(),
            serde_json::json!({"call": {"graph": "child", "mode": "wait"}}),
        ),
        (
            "calld".to_string(),
            serde_json::json!({"call": {"graph": "child", "mode": "detach"}}),
        ),
    ]);
    let count = 1 + rng.below(2) as usize;
    for _ in 0..count {
        let index = rng.below(snapshot.nodes.len() as u64) as usize;
        let id = snapshot.nodes[index].id.clone();
        snapshot.nodes[index].agent = None;
        snapshot.nodes[index].op = Some(if rng.chance(55) { "callw" } else { "calld" }.into());
        let mut kept = false;
        snapshot.edges.retain(|e| {
            if e.from_node != id {
                return true;
            }
            !std::mem::replace(&mut kept, true)
        });
    }
    snapshot
}

// ---------------------------------------------------------------------------
// Old side: GraphRunner with scripted ports
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct MemStore {
    record: Mutex<Option<GraphRunRecord>>,
}
pub struct Lease;
impl RunLease for Lease {}
impl RunStore for MemStore {
    fn load(&self, id: &str) -> Result<Option<GraphRunRecord>, GraphError> {
        Ok(self
            .record
            .lock()
            .unwrap()
            .clone()
            .filter(|r| r.run_id == id))
    }
    fn save(&self, r: &GraphRunRecord) -> Result<(), GraphError> {
        *self.record.lock().unwrap() = Some(r.clone());
        Ok(())
    }
    fn acquire_lease(&self, _: &str) -> Result<Box<dyn RunLease>, GraphError> {
        Ok(Box::new(Lease))
    }
}

pub struct Artifacts;
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

pub fn commit_of(key: &InvocationKey) -> CommitRef {
    CommitRef {
        id: key.durable_key(),
        node_id: key.node_id.clone(),
        invocation: key.invocation,
    }
}

pub type Trace = Vec<(String, u64, Vec<String>)>;

pub struct Scripted {
    pub seed: u64,
    pub inject_failures: bool,
    pub trace: Mutex<Trace>,
    pub calls: ScriptedCalls,
}

#[derive(Default)]
pub struct ScriptedCalls {
    pub seed: u64,
    /// Identities whose child has been admitted and is still running.
    pub waiting: Mutex<BTreeSet<String>>,
}

impl GraphCallPort for ScriptedCalls {
    fn call<'a>(
        &'a self,
        identity: &'a CallIdentity,
        spec: &'a Value,
        _input: &'a Value,
        _input_commits: &'a [CommitRef],
        _cancellation: crate::Cancellation,
    ) -> Pin<Box<dyn Future<Output = Result<GraphCallOutcome, GraphError>> + Send + 'a>> {
        Box::pin(async move {
            let key = identity.durable_key();
            let mode = spec["mode"].as_str().unwrap_or_default();
            let child = child_of(&key);
            Ok(match call_script(&key, mode, self.seed) {
                CallScript::WaitThenComplete
                    if self.waiting.lock().unwrap().insert(key.clone()) =>
                {
                    GraphCallOutcome::Waiting {
                        child_run_id: child,
                    }
                }
                CallScript::WaitThenComplete | CallScript::CompleteAtOnce => {
                    GraphCallOutcome::Completed {
                        child_run_id: child,
                        output: child_output(&key, self.seed),
                    }
                }
                CallScript::Detached => GraphCallOutcome::Detached {
                    child_run_id: child,
                },
                CallScript::Failed => GraphCallOutcome::Failed {
                    child_run_id: Some(child),
                    reason: "child refused".into(),
                },
                CallScript::Uncertain => GraphCallOutcome::Uncertain {
                    child_run_id: None,
                    reason: "admission unknown".into(),
                },
            })
        })
    }
}
impl NodeExecutionPort for Scripted {
    fn graph_call_port(&self) -> Option<&dyn GraphCallPort> {
        Some(&self.calls)
    }
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
            if self.inject_failures
                && let Some(reason) = failure_for(self.seed, &key.node_id, key.invocation)
            {
                return Ok(NodeExecutionOutcome::Failed { reason });
            }
            Ok(NodeExecutionOutcome::Completed(NodeCompletion {
                submission: format!("{} #{}", key.node_id, key.invocation),
                route: route_for(self.seed, &key.node_id, key.invocation, &request.routes),
                model_requests: 1,
                output: serde_json::json!({"node": key.node_id, "invocation": key.invocation}),
            }))
        })
    }
}

pub struct NoControl(crate::Cancellation);
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

pub async fn run_old(
    record: GraphRunRecord,
    seed: u64,
    inject_failures: bool,
) -> (Trace, GraphRunRecord) {
    let store = MemStore::default();
    let nodes = Scripted {
        seed,
        inject_failures: inject_failures && !record.snapshot.ops.contains_key("callw"),
        trace: Mutex::default(),
        calls: ScriptedCalls {
            seed,
            ..Default::default()
        },
    };
    let control = NoControl(Arc::new(std::sync::atomic::AtomicBool::new(false)));
    let mut done = record;
    // A waiting parent is re-entered by its host once the child settles.
    for _ in 0..1_000 {
        done = GraphRunner::new(&store, &Artifacts, &nodes, &control)
            .run(done)
            .await
            .expect("old runner");
        if done.status != RunStatus::WaitingCall {
            break;
        }
    }
    (nodes.trace.into_inner().unwrap(), done)
}

/// (invocation, sequence, route, commit id) of one result.
pub type ResultFact = (u64, u64, Option<String>, String);

/// The Graph facts both models must agree on.
#[derive(Debug, PartialEq)]
pub struct Facts {
    results: BTreeMap<String, Vec<ResultFact>>,
    decided: BTreeMap<String, EdgeDecision>,
    ceased: BTreeSet<String>,
    invocations: BTreeMap<String, u64>,
    passes: BTreeMap<String, u64>,
    module_activations: BTreeMap<String, u64>,
    sequence: u64,
    graph_calls: BTreeMap<String, GraphCallRecord>,
}

pub fn facts(r: &GraphRunRecord) -> Facts {
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
        graph_calls: r.graph_calls.clone(),
    }
}
