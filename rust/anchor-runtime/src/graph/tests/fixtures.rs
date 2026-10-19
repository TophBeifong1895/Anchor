#[derive(Default)]
struct MemoryArtifacts {
    values: Mutex<BTreeMap<String, Value>>,
    freezes: Mutex<usize>,
}
impl ArtifactPort for MemoryArtifacts {
    fn freeze<'a>(
        &'a self,
        key: &'a InvocationKey,
        c: &'a NodeCompletion,
    ) -> Pin<Box<dyn Future<Output = Result<CommitRef, GraphError>> + Send + 'a>> {
        Box::pin(async move {
            let id = key.durable_key();
            let mut values = self.values.lock().unwrap();
            if let Some(existing) = values.get(&id) {
                if existing != &c.output {
                    return Err(GraphError::CorruptRun("conflicting freeze retry".into()));
                }
            } else {
                values.insert(id.clone(), c.output.clone());
                *self.freezes.lock().unwrap() += 1;
            }
            Ok(CommitRef {
                id,
                node_id: key.node_id.clone(),
                invocation: key.invocation,
            })
        })
    }
    fn resolve<'a>(
        &'a self,
        c: &'a CommitRef,
    ) -> Pin<Box<dyn Future<Output = Result<Value, GraphError>> + Send + 'a>> {
        Box::pin(async move {
            self.values
                .lock()
                .unwrap()
                .get(&c.id)
                .cloned()
                .ok_or_else(|| GraphError::CorruptRun("missing commit".into()))
        })
    }
}
#[derive(Default)]
struct FakeNodes {
    facts: Mutex<BTreeMap<String, CompletionFact>>,
    calls: Mutex<Vec<NodeExecutionRequest>>,
    budget_once: Mutex<bool>,
    budget_on_node: Option<String>,
    uncertain: Mutex<bool>,
    caps_budget_off: bool,
    cancel_once: Mutex<bool>,
    op_run: bool,
    fail_once: Mutex<Option<String>>,
    interrupt_once: Mutex<BTreeMap<String, String>>,
    invalid_route_once: Mutex<bool>,
    recovery_once: Mutex<bool>,
    delays_ms: Mutex<BTreeMap<String, u64>>,
    completed_order: Mutex<Vec<String>>,
    active_calls: std::sync::atomic::AtomicUsize,
    max_active_calls: std::sync::atomic::AtomicUsize,
    graph_call_port: Option<Arc<FakeGraphCalls>>,
    resolved_plugins: Mutex<Vec<PluginBinding>>,
    /// What a node call observed: the dependency view it actually used, by Plugin
    /// id. Empty means "this host observed nothing", which is not a failure.
    observed_views: Mutex<BTreeMap<String, BTreeMap<String, String>>>,
    observed_digests: Mutex<BTreeMap<String, BTreeMap<String, String>>>,
}
impl NodeExecutionPort for FakeNodes {
    fn capabilities(&self) -> NodeExecutionCapabilities {
        NodeExecutionCapabilities {
            agent: true,
            op_run: self.op_run,
            host_operations: false,
            exact_provider_request_budget: !self.caps_budget_off,
        }
    }
    fn graph_call_port(&self) -> Option<&dyn GraphCallPort> {
        self.graph_call_port
            .as_deref()
            .map(|port| port as &dyn GraphCallPort)
    }
    fn observed_plugin_views(&self, key: &InvocationKey) -> BTreeMap<String, String> {
        // Reading consumes it, exactly as the host's resolver does: one entry per
        // call, not one per call ever made.
        self.observed_views
            .lock()
            .unwrap()
            .remove(&key.durable_key())
            .unwrap_or_default()
    }
    fn observed_plugin_digests(&self, key: &InvocationKey) -> BTreeMap<String, String> {
        self.observed_digests
            .lock()
            .unwrap()
            .remove(&key.durable_key())
            .unwrap_or_default()
    }
    fn resolve_plugins(&self, ids: &[String]) -> Result<Vec<PluginBinding>, GraphError> {
        let bindings = self.resolved_plugins.lock().unwrap();
        ids.iter()
            .map(|id| {
                bindings
                    .iter()
                    .find(|binding| &binding.id == id)
                    .cloned()
                    .ok_or_else(|| GraphError::Unsupported(format!("unknown Plugin `{id}`")))
            })
            .collect()
    }
    fn completion_fact<'a>(
        &'a self,
        key: &'a InvocationKey,
    ) -> Pin<Box<dyn Future<Output = Result<CompletionFact, GraphError>> + Send + 'a>> {
        Box::pin(async move {
            if *self.uncertain.lock().unwrap() {
                return Ok(CompletionFact::Uncertain("fake unknown".into()));
            }
            Ok(self
                .facts
                .lock()
                .unwrap()
                .get(&key.durable_key())
                .cloned()
                .unwrap_or(CompletionFact::NotStarted))
        })
    }
    fn execute<'a>(
        &'a self,
        request: NodeExecutionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<NodeExecutionOutcome, GraphError>> + Send + 'a>> {
        Box::pin(async move {
            if self
                .facts
                .lock()
                .unwrap()
                .get(&request.key.durable_key())
                .is_some_and(|fact| !matches!(fact, CompletionFact::Resumable))
            {
                return Err(GraphError::CorruptRun(
                    "duplicate dispatch after completion fact".into(),
                ));
            }
            self.calls.lock().unwrap().push(request.clone());
            let active = self
                .active_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                + 1;
            self.max_active_calls
                .fetch_max(active, std::sync::atomic::Ordering::SeqCst);
            let delay = self
                .delays_ms
                .lock()
                .unwrap()
                .get(&request.key.node_id)
                .copied()
                .unwrap_or_default();
            if delay > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
            }
            {
                let mut recovery = self.recovery_once.lock().unwrap();
                if *recovery {
                    *recovery = false;
                    self.active_calls
                        .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                    return Ok(NodeExecutionOutcome::WaitingRecovery {
                        attempts: vec![RecoveryAttempt {
                            attempt_id: 17,
                            step: 4,
                            tool: "publish".into(),
                            started_at: "2026-10-03T00:00:00Z".into(),
                        }],
                    });
                }
            }
            if let Some(reason) = self.fail_once.lock().unwrap().take() {
                self.facts.lock().unwrap().insert(
                    request.key.durable_key(),
                    CompletionFact::Failed(reason.clone()),
                );
                self.active_calls
                    .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                return Ok(NodeExecutionOutcome::Failed { reason });
            }
            if let Some(reason) = self
                .interrupt_once
                .lock()
                .unwrap()
                .remove(&request.key.node_id)
            {
                self.facts
                    .lock()
                    .unwrap()
                    .insert(request.key.durable_key(), CompletionFact::Resumable);
                self.active_calls
                    .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                return Ok(NodeExecutionOutcome::Interrupted { reason });
            }
            {
                let mut budget = self.budget_once.lock().unwrap();
                if *budget
                    && self
                        .budget_on_node
                        .as_deref()
                        .is_none_or(|node| node == request.key.node_id)
                {
                    *budget = false;
                    self.active_calls
                        .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                    return Ok(NodeExecutionOutcome::BudgetExhausted { model_requests: 2 });
                }
            }
            {
                let mut cancel = self.cancel_once.lock().unwrap();
                if *cancel {
                    *cancel = false;
                    request.cancellation.store(true, Ordering::Relaxed);
                    self.active_calls
                        .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                    return Ok(NodeExecutionOutcome::Cancelled);
                }
            }
            let route = {
                let mut invalid = self.invalid_route_once.lock().unwrap();
                if *invalid {
                    *invalid = false;
                    Some("unlisted-route".into())
                } else {
                    request.routes.first().cloned()
                }
            };
            let completion = NodeCompletion {
                submission: format!("done:{}", request.key.node_id),
                route,
                model_requests: 1,
                output: serde_json::json!({"node":request.key.node_id,"input":request.input}),
            };
            self.facts.lock().unwrap().insert(
                request.key.durable_key(),
                CompletionFact::Completed(completion.clone()),
            );
            self.completed_order
                .lock()
                .unwrap()
                .push(request.key.node_id.clone());
            self.active_calls
                .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            Ok(NodeExecutionOutcome::Completed(completion))
        })
    }
}

#[derive(Default)]
struct FakeGraphCalls {
    outcomes: Mutex<Vec<GraphCallOutcome>>,
    identities: Mutex<Vec<String>>,
}
impl GraphCallPort for FakeGraphCalls {
    fn call<'a>(
        &'a self,
        identity: &'a CallIdentity,
        _spec: &'a Value,
        _input: &'a Value,
        _input_commits: &'a [CommitRef],
        _cancellation: crate::Cancellation,
    ) -> Pin<Box<dyn Future<Output = Result<GraphCallOutcome, GraphError>> + Send + 'a>> {
        Box::pin(async move {
            self.identities.lock().unwrap().push(identity.durable_key());
            if self.outcomes.lock().unwrap().is_empty() {
                return Err(GraphError::CorruptRun(
                    "fake GraphCallPort exhausted".into(),
                ));
            }
            Ok(self.outcomes.lock().unwrap().remove(0))
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CrashPoint {
    NotStartedFactCheck,
    NodeFact,
    FailedFact,
    ArtifactCommit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct DurableTestArtifact {
    commit: CommitRef,
    output: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
enum DurableTestNodeFact {
    Completed(NodeCompletion),
    Failed(String),
}

struct DurableTestPorts {
    root: PathBuf,
    crash_point: Option<CrashPoint>,
}

impl DurableTestPorts {
    fn node_fact_path(&self, key: &InvocationKey) -> PathBuf {
        self.root.join(format!("node-{}.json", key.durable_key()))
    }

    fn artifact_path(&self, key: &InvocationKey) -> PathBuf {
        self.root
            .join(format!("artifact-{}.json", key.durable_key()))
    }

    fn execute_count_path(&self) -> PathBuf {
        self.root.join("execute-count")
    }

    fn freeze_count_path(&self) -> PathBuf {
        self.root.join("freeze-count")
    }
}

impl NodeExecutionPort for DurableTestPorts {
    fn capabilities(&self) -> NodeExecutionCapabilities {
        NodeExecutionCapabilities {
            agent: true,
            op_run: false,
            host_operations: false,
            exact_provider_request_budget: true,
        }
    }

    fn completion_fact<'a>(
        &'a self,
        key: &'a InvocationKey,
    ) -> Pin<Box<dyn Future<Output = Result<CompletionFact, GraphError>> + Send + 'a>> {
        Box::pin(async move {
            let path = self.node_fact_path(key);
            match fs::read(path) {
                Ok(bytes) => {
                    let fact: DurableTestNodeFact =
                        serde_json::from_slice(&bytes).map_err(GraphError::RunDecode)?;
                    match fact {
                        DurableTestNodeFact::Completed(completion) => {
                            Ok(CompletionFact::Completed(completion))
                        }
                        DurableTestNodeFact::Failed(reason) => Ok(CompletionFact::Failed(reason)),
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    if self.crash_point == Some(CrashPoint::NotStartedFactCheck) {
                        std::process::exit(70);
                    }
                    Ok(CompletionFact::NotStarted)
                }
                Err(error) => Err(error.into()),
            }
        })
    }

    fn execute<'a>(
        &'a self,
        request: NodeExecutionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<NodeExecutionOutcome, GraphError>> + Send + 'a>> {
        Box::pin(async move {
            if self.node_fact_path(&request.key).exists() {
                return Err(GraphError::CorruptRun(
                    "durable test Node port received a duplicate dispatch".into(),
                ));
            }
            write_durable_bytes(&self.execute_count_path(), b"1")?;
            if self.crash_point == Some(CrashPoint::FailedFact) {
                let reason = "durable test failure".to_owned();
                write_durable_json(
                    &self.node_fact_path(&request.key),
                    &DurableTestNodeFact::Failed(reason),
                )?;
                std::process::exit(73);
            }
            let completion = NodeCompletion {
                submission: format!("completed:{}", request.key.node_id),
                route: request.routes.first().cloned(),
                model_requests: 1,
                output: serde_json::json!({"node":request.key.node_id,"input":request.input}),
            };
            write_durable_json(
                &self.node_fact_path(&request.key),
                &DurableTestNodeFact::Completed(completion.clone()),
            )?;
            if self.crash_point == Some(CrashPoint::NodeFact) {
                std::process::exit(71);
            }
            Ok(NodeExecutionOutcome::Completed(completion))
        })
    }
}

impl ArtifactPort for DurableTestPorts {
    fn freeze<'a>(
        &'a self,
        key: &'a InvocationKey,
        completion: &'a NodeCompletion,
    ) -> Pin<Box<dyn Future<Output = Result<CommitRef, GraphError>> + Send + 'a>> {
        Box::pin(async move {
            let path = self.artifact_path(key);
            let artifact = match fs::read(&path) {
                Ok(bytes) => {
                    let existing: DurableTestArtifact =
                        serde_json::from_slice(&bytes).map_err(GraphError::RunDecode)?;
                    if existing.output != completion.output {
                        return Err(GraphError::CorruptRun(
                            "artifact retry changed the frozen output".into(),
                        ));
                    }
                    existing
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    let artifact = DurableTestArtifact {
                        commit: CommitRef {
                            id: key.durable_key(),
                            node_id: key.node_id.clone(),
                            invocation: key.invocation,
                        },
                        output: completion.output.clone(),
                    };
                    write_durable_json(&path, &artifact)?;
                    write_durable_bytes(&self.freeze_count_path(), b"1")?;
                    artifact
                }
                Err(error) => return Err(error.into()),
            };
            if self.crash_point == Some(CrashPoint::ArtifactCommit)
                && !completion.submission.starts_with("fanout activation")
            {
                std::process::exit(72);
            }
            Ok(artifact.commit)
        })
    }

    fn resolve<'a>(
        &'a self,
        commit: &'a CommitRef,
    ) -> Pin<Box<dyn Future<Output = Result<Value, GraphError>> + Send + 'a>> {
        Box::pin(async move {
            let path = self.root.join(format!("artifact-{}.json", commit.id));
            let bytes = fs::read(path)?;
            let artifact: DurableTestArtifact =
                serde_json::from_slice(&bytes).map_err(GraphError::RunDecode)?;
            if artifact.commit != *commit {
                return Err(GraphError::CorruptRun(
                    "test artifact commit identity mismatch".into(),
                ));
            }
            Ok(artifact.output)
        })
    }
}

fn write_durable_json(path: &std::path::Path, value: &impl Serialize) -> Result<(), GraphError> {
    let bytes = serde_json::to_vec(value).map_err(GraphError::RunDecode)?;
    write_durable_bytes(path, &bytes)
}

fn write_durable_bytes(path: &std::path::Path, bytes: &[u8]) -> Result<(), GraphError> {
    let parent = path
        .parent()
        .ok_or_else(|| GraphError::CorruptRun("test durable path has no parent".into()))?;
    fs::create_dir_all(parent)?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| GraphError::CorruptRun("test durable path is invalid".into()))?;
    let temporary = parent.join(format!(".{name}.{}.{}.tmp", std::process::id(), stamp));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&temporary, path)?;
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

fn graph(nodes: &[&str], edges: &[(&str, &str)], entry: &str) -> GraphSnapshot {
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
    GraphSnapshot {
        objective: "objective".into(),
        input: serde_json::json!({"defaults":{"a":1,"b":1}}),
        entry: entry.into(),
        agents,
        ops: BTreeMap::new(),
        nodes: nodes
            .iter()
            .map(|id| GraphNode {
                id: (*id).into(),
                agent: Some("worker".into()),
                op: None,
                input: None,
                plugins: vec![],
                max_rounds: None,
            })
            .collect(),
        edges: edges
            .iter()
            .map(|(a, b)| GraphEdge {
                from_node: (*a).into(),
                to_node: (*b).into(),
            })
            .collect(),
        module_rounds: BTreeMap::new(),
    }
}
fn parallel_graph() -> GraphSnapshot {
    let mut snapshot = graph(
        &["start", "left", "right", "collect", "after"],
        &[
            ("start", "left"),
            ("start", "right"),
            ("left", "collect"),
            ("right", "collect"),
            ("collect", "after"),
        ],
        "start",
    );
    snapshot.ops.insert(
        "fanout".into(),
        serde_json::json!({"fanout":{"join":"collect"}}),
    );
    snapshot
        .ops
        .insert("join".into(), serde_json::json!({"join":{}}));
    snapshot.nodes[0].agent = None;
    snapshot.nodes[0].op = Some("fanout".into());
    snapshot.nodes[3].agent = None;
    snapshot.nodes[3].op = Some("join".into());
    snapshot
}

#[derive(Default)]
struct ScenarioNodes {
    outcomes: Vec<Value>,
    calls: Mutex<Vec<(String, u64)>>,
    facts: Mutex<BTreeMap<String, CompletionFact>>,
}
impl NodeExecutionPort for ScenarioNodes {
    fn capabilities(&self) -> NodeExecutionCapabilities {
        NodeExecutionCapabilities {
            agent: true,
            op_run: false,
            host_operations: false,
            exact_provider_request_budget: true,
        }
    }

    fn completion_fact<'a>(
        &'a self,
        key: &'a InvocationKey,
    ) -> Pin<Box<dyn Future<Output = Result<CompletionFact, GraphError>> + Send + 'a>> {
        Box::pin(async move {
            Ok(self
                .facts
                .lock()
                .unwrap()
                .get(&key.durable_key())
                .cloned()
                .unwrap_or(CompletionFact::NotStarted))
        })
    }

    fn execute<'a>(
        &'a self,
        request: NodeExecutionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<NodeExecutionOutcome, GraphError>> + Send + 'a>> {
        Box::pin(async move {
            let index = self.calls.lock().unwrap().len();
            let outcome = self.outcomes.get(index).ok_or_else(|| {
                GraphError::CorruptRun("oracle has no outcome for node invocation".into())
            })?;
            if outcome["node"] != request.key.node_id
                || outcome["invocation"] != request.key.invocation
            {
                return Err(GraphError::CorruptRun(format!(
                    "oracle expected {} invocation {}, got {} invocation {}",
                    outcome["node"],
                    outcome["invocation"],
                    request.key.node_id,
                    request.key.invocation
                )));
            }
            self.calls
                .lock()
                .unwrap()
                .push((request.key.node_id.clone(), request.key.invocation));
            if outcome["exit_status"] == "budget_exhausted" {
                return Ok(NodeExecutionOutcome::BudgetExhausted { model_requests: 2 });
            }
            if outcome["exit_status"] != "Submitted" {
                return Ok(NodeExecutionOutcome::Failed {
                    reason: outcome["exit_status"]
                        .as_str()
                        .unwrap_or("fixture failure")
                        .into(),
                });
            }
            let completion = NodeCompletion {
                submission: format!("oracle:{}:{}", request.key.node_id, request.key.invocation),
                route: outcome["route"].as_str().map(str::to_owned),
                model_requests: 1,
                output: serde_json::json!({"node":request.key.node_id,"invocation":request.key.invocation}),
            };
            self.facts.lock().unwrap().insert(
                request.key.durable_key(),
                CompletionFact::Completed(completion.clone()),
            );
            Ok(NodeExecutionOutcome::Completed(completion))
        })
    }
}
