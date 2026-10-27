//! Agent/Op dispatch and sandbox outcome settlement.
use super::*;

/// Read the deterministic routing marker written by the anchor-route Op helper.
/// The graph kernel remains responsible for checking that the selected route
/// is an actual outgoing edge, and for requiring a choice when there are
/// multiple exits.
fn read_op_route(stdout: &str) -> (String, Option<String>) {
    let lines = stdout.lines().collect::<Vec<_>>();
    let Some((index, first)) = lines
        .iter()
        .enumerate()
        .find(|(_, line)| !line.trim().is_empty())
    else {
        return (String::new(), None);
    };
    let Some(target) = first.trim().strip_prefix("ANCHOR_ROUTE:") else {
        return (stdout.trim().to_owned(), None);
    };
    let submission = lines[index + 1..].join("\n").trim().to_owned();
    (submission, Some(target.trim().to_owned()))
}

impl HostNodes {
    async fn execute_node(
        &self,
        request: NodeExecutionRequest,
    ) -> Result<NodeExecutionOutcome, GraphError> {
        if request.kind != NodeKind::OpRun {
            return Err(GraphError::Unsupported(
                "HostNodes deterministic executor accepts Op.run only".into(),
            ));
        }
        let workspace = self
            .artifacts
            .prepare_workspace(&request.key, &request.input_commits)?;
        let local_inputs = self
            .io_resolver
            .local_input_mounts(&request.key)
            .map_err(GraphError::Unsupported)?;
        let mut readonly_inputs = self.artifacts.input_mounts(
            &request.input_commits,
            &request.key.run_id,
            &request.key.graph_digest,
        )?;
        readonly_inputs.extend(local_inputs.clone());
        if !request.plugins.is_empty() {
            return Err(GraphError::Unsupported(
                "Op.run cannot execute Plugins".into(),
            ));
        }
        let command = request
            .operation
            .as_ref()
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| GraphError::Unsupported("Op.run must be a command string".into()))?;
        // Kernel scheduler wraps the frozen Run input with upstream artifact metadata.
        // Business commands receive only the effective Graph input.
        let run_input = request.input.get("input").ok_or_else(|| {
            GraphError::Unsupported("Op.run request is missing effective Graph input".into())
        })?;
        let input = serde_json::to_string(run_input).map_err(|error| {
            GraphError::Unsupported(format!("Op.run input is not JSON: {error}"))
        })?;
        let argv =
            op::shell_command(command, &self.allowed_commands).map_err(GraphError::Unsupported)?;
        let route_helper = op::route_helper_mount().map_err(GraphError::Unsupported)?;
        let channel_send_helper =
            op::channel_send_helper_mount().map_err(GraphError::Unsupported)?;
        // Delegated bots are read from the Run's own Graph identity, never from
        // the Op's command line. A Graph that is not authorized gets no mount,
        // and mounting is what grants the ability. A Run with no readable
        // identity can delegate to nothing: fail closed by mounting no bot
        // rather than by refusing an Op that does not concern channels at all.
        let state_root = self.io_resolver.local_inputs.state_root();
        let delegated = match crate::run_metadata::load(state_root, &request.key.run_id) {
            Ok(Some(metadata)) => {
                crate::channel_supervisor::management::delegated_bots(state_root, &metadata.graph)
                    .map_err(GraphError::Unsupported)?
            }
            Ok(None) => Vec::new(),
            Err(error) => {
                return Err(GraphError::Unsupported(format!(
                    "delegated channel authorization unavailable: {error:?}"
                )));
            }
        };
        let mut channel_mounts = Vec::new();
        let mut channel_inputs = Vec::new();
        for bot in &delegated {
            // A gateway that stopped between the authorization read and the
            // mount is simply absent from the set: the node then reports "no
            // bot is delegated" instead of failing on an unresolvable grant.
            if bot.descriptor.canonicalize().is_err() || bot.socket.canonicalize().is_err() {
                continue;
            }
            let mounts = op::channel_mounts(&bot.graph, &bot.descriptor, &bot.socket);
            channel_inputs.extend(mounts.iter().cloned());
            channel_mounts.extend(mounts);
        }
        let environment = self
            .io_resolver
            .tool_environment()
            .map_err(GraphError::Unsupported)?;
        let sandbox = environment
            .grant_sandbox(&self.sandbox)
            .map_err(GraphError::Unsupported)?
            .with_readonly_grants(&local_inputs)
            .and_then(|sandbox| sandbox.with_readonly_grants(std::slice::from_ref(&route_helper)))
            .and_then(|sandbox| {
                sandbox.with_readonly_grants(std::slice::from_ref(&channel_send_helper))
            })
            .and_then(|sandbox| sandbox.with_readonly_grants(&channel_mounts))
            .map_err(|error| GraphError::Unsupported(error.to_string()))?;
        let network = self
            .io_resolver
            .mounted_plugins(&request)
            .map_err(GraphError::Unsupported)?
            .network;
        let mut sandbox_request = SandboxRequest {
            writable_inputs: Vec::new(),
            workspace: workspace.clone(),
            working_directory: None,
            command: argv,
            readonly_inputs,
            workspace_readonly: vec![],
            tool_dirs: vec![],
            devices: Vec::new(),
            environment: vec![
                anchor_runtime::SandboxEnvironment::new("ANCHOR_NODE", request.key.node_id.clone()),
                // The delegated-send helper derives its idempotency key from the
                // Run identity, never from the message text: a weekly report
                // that repeats last week's wording is a new delivery, while a
                // retried node must stay one.
                anchor_runtime::SandboxEnvironment::new(
                    "ANCHOR_RUN_ID",
                    request.key.run_id.clone(),
                ),
                anchor_runtime::SandboxEnvironment::new(
                    "ANCHOR_INVOCATION",
                    request.key.invocation.to_string(),
                ),
                anchor_runtime::SandboxEnvironment::new("ANCHOR_ROUTES", request.routes.join(",")),
                anchor_runtime::SandboxEnvironment::new("ANCHOR_INPUT", input),
            ],
            network: if network {
                NetworkPolicy::Enabled
            } else {
                NetworkPolicy::Disabled
            },
            timeout: Duration::from_secs_f64(request.wall_time_limit_seconds.unwrap_or(3600.0)),
            max_output_bytes: 64 * 1024,
            spill: None,
            cancellation: request.cancellation.clone(),
        };
        // The deployment's operator-registered tool paths are this invocation's
        // authority: an Op.run reaches them by absolute path, and nothing else
        // grants them.
        op::add_route_helper(&mut sandbox_request, route_helper);
        sandbox_request
            .readonly_inputs
            .push(channel_send_helper.clone());
        sandbox_request.readonly_inputs.extend(channel_inputs);
        environment.apply(&mut sandbox_request);
        self.start(&request.key)?;
        let result = match sandbox.run(sandbox_request).await {
            Ok(result) => result,
            Err(SandboxError::InvalidRequest(reason)) => {
                write_durable(
                    &self
                        .facts_root
                        .join(format!("{}.failed", fact_stem(&request.key))),
                    reason.as_bytes(),
                )?;
                return Ok(NodeExecutionOutcome::Failed { reason });
            }
            Err(error) => return Err(GraphError::Unsupported(error.to_string())),
        };
        match result.status {
            SandboxStatus::Completed if result.exit_code == Some(0) => {
                // Exit 0 without a declared output is not success: downstream
                // nodes would read a file that is not there.
                let missing = super::missing_outputs(&workspace, &request.writes);
                if !missing.is_empty() {
                    let reason = super::missing_outputs_reason(&missing);
                    write_durable(
                        &self
                            .facts_root
                            .join(format!("{}.failed", fact_stem(&request.key))),
                        reason.as_bytes(),
                    )?;
                    return Ok(NodeExecutionOutcome::Failed { reason });
                }
                let (submission, route) = read_op_route(&result.stdout);
                self.complete(&request.key, NodeCompletion {
                    submission,
                    route,
                    model_requests: 0,
                    output: json!({"stdout":result.stdout,"stderr":result.stderr,"exit_code":result.exit_code}),
                })
            }
            SandboxStatus::Cancelled => Ok(NodeExecutionOutcome::Cancelled),
            _ => {
                let mut reason = format!("{} (exit_code={:?})", result.reason, result.exit_code);
                for output in [&result.stdout, &result.stderr] {
                    if !output.trim().is_empty() {
                        reason.push('\n');
                        reason.push_str(output.trim());
                    }
                }
                write_durable(
                    &self
                        .facts_root
                        .join(format!("{}.failed", fact_stem(&request.key))),
                    reason.as_bytes(),
                )?;
                Ok(NodeExecutionOutcome::Failed { reason })
            }
        }
    }
}

impl NodeExecutionPort for HostNodes {
    fn capabilities(&self) -> NodeExecutionCapabilities {
        NodeExecutionCapabilities {
            agent: self.goose_nodes.is_some(),
            op_run: true,
            host_operations: true,
            exact_provider_request_budget: false,
        }
    }
    fn resolve_plugins(&self, ids: &[String]) -> Result<Vec<PluginBinding>, GraphError> {
        self.io_resolver
            .resolve_plugins(ids)
            .map_err(GraphError::Unsupported)
    }
    fn invocation_plugin_authority(&self, record: &GraphRunRecord) -> Result<bool, GraphError> {
        crate::assistant_plugins::invocation_authority(
            self.io_resolver.local_inputs.state_root(),
            record,
        )
        .map_err(GraphError::Unsupported)
    }
    fn completion_fact<'a>(
        &'a self,
        key: &'a InvocationKey,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<CompletionFact, GraphError>> + Send + 'a>>
    {
        Box::pin(async move {
            if let Some(fact) = self.assistant_completion_fact(key)? {
                return Ok(fact);
            }
            let host = load_host_completion_fact(&self.facts_root, key)?;
            if let CompletionFact::Completed(completion) = &host {
                self.settle_assistant_reply(key, completion)?;
            }
            if crate::run_data::legacy_invocation_exists(
                self.io_resolver.local_inputs.state_root(),
                key,
            )
            .map_err(GraphError::Unsupported)?
            {
                return Ok(CompletionFact::Uncertain(
                    "legacy Agent state is not resumable by the standard Goose Host".into(),
                ));
            }
            if let Some(nodes) = &self.goose_nodes {
                let goose = nodes.completion_fact(key)?;
                return Ok(match (&host, &goose) {
                    (CompletionFact::NotStarted, _) => goose,
                    (_, CompletionFact::NotStarted) => host,
                    _ if host == goose => host,
                    _ => CompletionFact::Uncertain("host and Goose node facts conflict".into()),
                });
            }
            Ok(host)
        })
    }
    fn record_recovery_decision(
        &self,
        key: &InvocationKey,
        attempt_id: i64,
        decision: RecoveryDecision,
    ) -> Result<(), GraphError> {
        self.goose_nodes
            .as_ref()
            .ok_or_else(|| {
                GraphError::CorruptRun("native recovery backend is not configured".into())
            })?
            .record_recovery_decision(key, attempt_id, decision)
    }
    fn observed_plugin_views(&self, key: &InvocationKey) -> BTreeMap<String, String> {
        self.io_resolver.observed_plugin_views(key)
    }
    fn observed_plugin_digests(&self, key: &InvocationKey) -> BTreeMap<String, String> {
        self.io_resolver.observed_plugin_digests(key)
    }
    fn execute<'a>(
        &'a self,
        request: NodeExecutionRequest,
    ) -> Pin<
        Box<dyn std::future::Future<Output = Result<NodeExecutionOutcome, GraphError>> + Send + 'a>,
    > {
        if let Some(nodes) = &self.goose_nodes {
            match nodes.pending_recovery(&request.key) {
                Ok(attempts) if !attempts.is_empty() => {
                    return Box::pin(async move {
                        Ok(NodeExecutionOutcome::WaitingRecovery { attempts })
                    });
                }
                Err(error) => return Box::pin(async move { Err(error) }),
                _ => {}
            }
        }
        if request.kind == NodeKind::OpHost {
            return Box::pin(self.execute_host_operation(request));
        }
        match crate::assistant::source(self.io_resolver.local_inputs.state_root(), &request.key) {
            Ok(Some(_)) => return Box::pin(self.execute_assistant_request(request)),
            Ok(None) => {}
            Err(error) => return Box::pin(async move { Err(GraphError::Unsupported(error)) }),
        }
        self.execute_regular(request)
    }
}

impl HostNodes {
    pub(super) fn execute_regular<'a>(
        &'a self,
        request: NodeExecutionRequest,
    ) -> Pin<
        Box<dyn std::future::Future<Output = Result<NodeExecutionOutcome, GraphError>> + Send + 'a>,
    > {
        if request.kind == NodeKind::Agent {
            match crate::run_data::legacy_invocation_exists(
                self.io_resolver.local_inputs.state_root(),
                &request.key,
            ) {
                Ok(false) => {}
                Ok(true) => {
                    return Box::pin(async {
                        Err(GraphError::Unsupported(
                            "legacy Agent state cannot be executed as a Goose invocation".into(),
                        ))
                    });
                }
                Err(error) => return Box::pin(async move { Err(GraphError::Unsupported(error)) }),
            }
            if let Some(nodes) = &self.goose_nodes {
                return Box::pin(nodes.execute(request));
            }
            return Box::pin(async {
                Err(GraphError::Unsupported(
                    "Agent execution requires a configured Goose runtime".into(),
                ))
            });
        }
        Box::pin(async move {
            let cancellation = request.cancellation.clone();
            let timeout =
                Duration::try_from_secs_f64(request.wall_time_limit_seconds.unwrap_or(3600.0))
                    .map_err(|_| GraphError::Unsupported("invalid node wall time".into()))?;
            let cancel = async {
                while !cancellation.load(Ordering::Relaxed) {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            };
            tokio::select! {
                biased;
                _ = cancel => Ok(NodeExecutionOutcome::Cancelled),
                result = tokio::time::timeout(timeout, self.execute_node(request)) => result
                    .map_err(|_| GraphError::Unsupported("node wall time exceeded; unfinished effects are uncertain".into()))?,
            }
        })
    }
}
