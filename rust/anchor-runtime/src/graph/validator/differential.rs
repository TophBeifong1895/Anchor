//! Differential test: the old push-model `GraphRunner` against the kernel with
//! `GraphValidator`, on thousands of random Graphs and route scripts.
//!
//! Both sides see the same Graph and the same deterministic route choices.
//! They must dispatch the same node invocations, in the same order, with the
//! same committed inputs, and end with the same Graph facts.

use std::sync::{Arc, Mutex};

use anchor_kernel::{
    Action, Authority, Evidence, Kernel, MemoryStore, Outcome, Phase, Principal, PrincipalKind,
    Reason, Submission, Waiter,
};

use super::super::*;
use super::{
    CallSubmission, DispatchKind, GRAPH_VALIDATOR, GraphValidator, NodeSubmission, call_identity,
    record_of, run_resource,
};

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

/// prefix → fanout → 2–3 linear branches → join → suffix, with random back
/// edges from the suffix that re-enter the whole region, skip edges around
/// it, and round limits everywhere (so branches can hit their ceiling).
fn random_parallel_graph(seed: u64) -> GraphSnapshot {
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

/// What a child Run does, decided by the call identity on both sides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CallScript {
    /// wait: admitted, waited on, then completes.
    WaitThenComplete,
    /// wait or detach: the port answers with a completed child at once.
    CompleteAtOnce,
    /// detach: accepted.
    Detached,
    Failed,
    Uncertain,
}

fn call_script(key: &str, mode: &str, seed: u64) -> CallScript {
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

fn child_of(key: &str) -> String {
    format!("child-{}", &key[..12])
}

fn child_output(key: &str, seed: u64) -> Value {
    serde_json::json!({"answer": hash(&[key, "out"], seed) % 1000, "secret": "child business result"})
}

/// Random serial Graphs with one or two nodes turned into Op.call (wait or
/// detach), each keeping at most one exit.
fn random_call_graph(seed: u64) -> GraphSnapshot {
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
    calls: ScriptedCalls,
}

#[derive(Default)]
struct ScriptedCalls {
    seed: u64,
    /// Identities whose child has been admitted and is still running.
    waiting: Mutex<BTreeSet<String>>,
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

// ---------------------------------------------------------------------------
// New side: kernel + GraphValidator + a trivial scheduler
// ---------------------------------------------------------------------------

struct AllowAll;
impl Authority for AllowAll {
    fn permits(&self, _: &Principal, _: Action, _: &str) -> bool {
        true
    }
}

/// Waves: take every slot open now, submit each, then look again. This
/// mirrors the old Runner's barrier so the two can be compared step by step.
/// Parallel submissions to one Run race on its version; a stale submission is
/// returned and simply resubmitted against the current state.
fn run_new(record: GraphRunRecord, seed: u64) -> NewRun {
    let mut kernel = Kernel::new(MemoryStore::default(), Box::new(AllowAll)).unwrap();
    kernel
        .install(GRAPH_VALIDATOR, Arc::new(GraphValidator))
        .unwrap();
    let resource = run_resource(&record, u64::MAX).unwrap();
    let id = resource.id.clone();
    kernel.create(resource).unwrap();
    let agent = Principal::new("executor", PrincipalKind::Agent);
    let host = Principal::new("host", PrincipalKind::Code);
    let mut trace = Trace::new();
    let mut resubmissions = 0;
    let mut late = BTreeSet::new();
    for _ in 0..10_000 {
        let resource = kernel.load(&id).unwrap().unwrap();
        if resource.is_settled() {
            break;
        }
        if let Some(Phase::Waiting {
            waiter: Waiter::Child { .. },
        }) = resource.phase()
        {
            // The child settled: host code releases the parent (kernel law).
            let resume = Submission::new(&id, resource.version, Action::Request)
                .content(serde_json::json!({"control": "resume"}));
            assert!(kernel.submit(&host, 0, resume).unwrap().is_accepted());
            continue;
        }
        let wave = GraphValidator::frontier(&resource).unwrap();
        let open = kernel.open(&id).unwrap();
        assert_eq!(
            open.iter().map(|s| s.id.clone()).collect::<Vec<_>>(),
            wave.iter().map(|d| d.slot()).collect::<Vec<_>>()
        );
        if wave.is_empty() {
            break;
        }
        let state = record_of(&resource).unwrap();
        for dispatch in wave {
            let node = dispatch.node.id.clone();
            let principal = if dispatch.kind == DispatchKind::Work {
                trace.push((
                    node.clone(),
                    dispatch.invocation,
                    dispatch.inputs.iter().map(|c| c.id.clone()).collect(),
                ));
                &agent
            } else {
                &host
            };
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
            let route = match dispatch.kind {
                DispatchKind::Work => route_for(seed, &node, dispatch.invocation, &routes),
                _ => None,
            };
            let content = if dispatch.kind == DispatchKind::Call {
                let key = call_identity(&state, &dispatch).unwrap().durable_key();
                let mode = state.snapshot.ops[dispatch.node.op.as_ref().unwrap()]["call"]["mode"]
                    .as_str()
                    .unwrap()
                    .to_owned();
                let child = child_of(&key);
                let bound = state.graph_calls.contains_key(&key);
                serde_json::to_value(match (call_script(&key, &mode, seed), bound) {
                    (CallScript::WaitThenComplete, false) => CallSubmission::Bound {
                        child_run_id: child,
                    },
                    (CallScript::WaitThenComplete | CallScript::CompleteAtOnce, _) => {
                        CallSubmission::Completed {
                            child_run_id: child,
                            // For a detach this carries the child's business
                            // result; the validator must not let it through.
                            output: child_output(&key, seed),
                            commit: commit.clone(),
                        }
                    }
                    (CallScript::Detached, _) => CallSubmission::Completed {
                        child_run_id: child,
                        output: Value::Null,
                        commit: commit.clone(),
                    },
                    (CallScript::Failed, _) => CallSubmission::Failed {
                        child_run_id: Some(child),
                        reason: "child refused".into(),
                    },
                    (CallScript::Uncertain, _) => CallSubmission::Uncertain {
                        child_run_id: None,
                        reason: "admission unknown".into(),
                    },
                })
                .unwrap()
            } else {
                serde_json::to_value(NodeSubmission {
                    summary: format!("{node} #{}", dispatch.invocation),
                    route,
                    output: serde_json::json!({"node": node, "invocation": dispatch.invocation}),
                    model_requests: 1,
                    commit: commit.clone(),
                    plugin_views: BTreeMap::new(),
                    plugin_digests: BTreeMap::new(),
                })
                .unwrap()
            };
            // An executor submits against the version it was dispatched at;
            // only after a stale return does it re-read.
            let mut version = resource.version;
            loop {
                let verdict = kernel
                    .submit(
                        principal,
                        1,
                        Submission::new(&id, version, Action::Write)
                            .slot(dispatch.slot())
                            .content(content.clone())
                            .evidence(Evidence::Resource {
                                id: commit.id.clone(),
                                version: 0,
                            }),
                    )
                    .unwrap();
                if verdict.reason == Reason::StaleVersion {
                    resubmissions += 1;
                    version = kernel.load(&id).unwrap().unwrap().version;
                    continue;
                }
                // S6: a sibling finishing after the Run failed is refused and
                // stays in the Run's history; it is not merged into results.
                if verdict.reason == Reason::TaskSettled {
                    late.insert((node.clone(), dispatch.invocation));
                    break;
                }
                assert!(verdict.is_accepted(), "{node}: {verdict:?}");
                break;
            }
        }
    }
    let resource = kernel.load(&id).unwrap().unwrap();
    // Every late refusal must be in the Run's history with its evidence.
    let refused_in_history = kernel
        .history(&id)
        .unwrap()
        .iter()
        .filter(|r| r.verdict.reason == Reason::TaskSettled && !r.submission.evidence.is_empty())
        .count();
    assert_eq!(refused_in_history, late.len());
    NewRun {
        trace,
        record: record_of(&resource).unwrap(),
        phase: resource.phase().cloned().unwrap(),
        resubmissions,
        late,
    }
}

struct NewRun {
    trace: Trace,
    record: GraphRunRecord,
    phase: Phase,
    resubmissions: u64,
    late: BTreeSet<(String, u64)>,
}

fn result_set(record: &GraphRunRecord) -> BTreeSet<(String, u64)> {
    record
        .results
        .values()
        .flatten()
        .map(|r| (r.node_id.clone(), r.key.invocation))
        .collect()
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
    graph_calls: BTreeMap<String, GraphCallRecord>,
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
        graph_calls: r.graph_calls.clone(),
    }
}

/// An invocation counted as started with no result is a phantom.
fn remove_phantoms(facts: &mut Facts) -> u64 {
    let mut phantoms = 0;
    for (node, invocation) in facts.invocations.iter_mut() {
        let last = facts
            .results
            .get(node)
            .and_then(|r| r.iter().map(|x| x.0).max())
            .unwrap_or(0);
        if *invocation > last {
            phantoms += *invocation - last;
            *facts.passes.get_mut(node).expect("passes") -= *invocation - last;
            *invocation = last;
        }
    }
    facts.invocations.retain(|_, v| *v > 0);
    facts.passes.retain(|_, v| *v > 0);
    phantoms
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
    failed: u64,
    parallel_activations: u64,
    resubmissions: u64,
    phantom_invocations: u64,
    late_refusals: u64,
    calls_waited: u64,
    calls_detached: u64,
    call_failed: u64,
    call_unknown: u64,
}

async fn check(snapshot: GraphSnapshot, seed: u64, tally: &mut Tally) {
    let Ok(record) =
        GraphRunRecord::create_with_id(snapshot.clone(), serde_json::json!({}), format!("r{seed}"))
    else {
        tally.rejected_graphs += 1;
        return;
    };
    let (old_trace, old) = run_old(record.clone(), seed).await;
    let NewRun {
        trace: new_trace,
        record: new,
        phase,
        resubmissions,
        late,
    } = run_new(record, seed);
    assert_eq!(
        old_trace, new_trace,
        "dispatch trace differs, seed {seed}, graph {snapshot:#?}"
    );
    let call_ended_it = old.graph_calls.values().any(|c| {
        matches!(
            c.status,
            GraphCallStatus::Failed | GraphCallStatus::Uncertain
        )
    });
    if old.status == RunStatus::Failed && !call_ended_it {
        // S6: fail fast. Old merged results = new merged results + late
        // refusals, disjointly. Ceiling reasons are both recorded.
        let new_results = result_set(&new);
        assert!(new_results.is_disjoint(&late), "seed {seed}");
        let union = new_results.union(&late).cloned().collect::<BTreeSet<_>>();
        assert_eq!(
            result_set(&old),
            union,
            "Failed results differ, seed {seed}"
        );
        assert!(
            !old.ceased.is_empty() && !new.ceased.is_empty(),
            "seed {seed}"
        );
        assert!(
            matches!(
                phase,
                Phase::Settled {
                    result: Outcome::Failed { .. }
                }
            ),
            "seed {seed}: {phase:?}"
        );
        tally.failed += 1;
        tally.late_refusals += late.len() as u64;
        let mut old_facts = facts(&old);
        tally.phantom_invocations += remove_phantoms(&mut old_facts);
    } else {
        assert!(late.is_empty(), "seed {seed}");
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
            (
                RunStatus::Failed,
                Phase::Settled {
                    result: Outcome::Failed { .. },
                },
            ) if call_ended_it => tally.call_failed += 1,
            // S7: an unknown admission settles as unknown, not as failed.
            (
                RunStatus::Failed,
                Phase::Settled {
                    result: Outcome::Unknown { .. },
                },
            ) if call_ended_it => tally.call_unknown += 1,
            other => panic!("terminal state differs, seed {seed}: {other:?}"),
        }
    }
    tally.cases += 1;
    tally.resubmissions += resubmissions;
    for call in new.graph_calls.values() {
        match call.status {
            GraphCallStatus::Completed => tally.calls_waited += 1,
            GraphCallStatus::Detached => {
                tally.calls_detached += 1;
                assert!(
                    call.output
                        .as_ref()
                        .is_some_and(|o| o.get("secret").is_none()),
                    "a detach leaked the child's result, seed {seed}"
                );
            }
            _ => {}
        }
    }
    tally.parallel_activations += new.results.get("f").map(|r| r.len() as u64).unwrap_or(0);
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
        check(random_graph(seed), seed, &mut tally).await;
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

#[tokio::test]
async fn parallel_regions_match_the_runner_on_random_graphs() {
    let mut tally = Tally::default();
    for seed in 0..5_000 {
        check(random_parallel_graph(seed), seed, &mut tally).await;
    }
    eprintln!("differential parallel: {tally:?}");
    assert!(tally.cases > 4_500, "{tally:?}");
    assert!(
        tally.parallel_activations > 5_000
            && tally.resubmissions > 1_000
            && tally.phantom_invocations > 0
            && tally.late_refusals > 0,
        "{tally:?}"
    );
    assert!(
        tally.completed > 100 && tally.failed > 100,
        "completed and branch-ceiling failures exercised: {tally:?}"
    );
}

#[tokio::test]
async fn graph_calls_match_the_runner_on_random_graphs() {
    let mut tally = Tally::default();
    for seed in 0..5_000 {
        check(random_call_graph(seed), seed, &mut tally).await;
    }
    eprintln!("differential calls: {tally:?}");
    assert!(tally.cases > 4_500, "{tally:?}");
    assert!(
        tally.calls_waited > 500
            && tally.calls_detached > 500
            && tally.call_failed > 100
            && tally.call_unknown > 100,
        "{tally:?}"
    );
}
