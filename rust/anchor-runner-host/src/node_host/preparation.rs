//! Authorized tool preparation and connection for one invocation.
use super::*;

impl HostIoResolver {
    pub(super) fn tool_environment(
        &self,
    ) -> Result<crate::tool_environment::ToolEnvironment, String> {
        let environment = self.mcp.environment.clone();
        match &self.runtime_configuration {
            Some(configuration) => environment.with_runtime_snapshot(&configuration.snapshot()?),
            None => Ok(environment),
        }
    }

    pub(crate) fn validate_plugin_configuration(
        &self,
        bindings: &[PluginBinding],
    ) -> Result<(), String> {
        self.catalog_config(bindings)?
            .validate_bindings(bindings, true)
    }
}

impl HostIoResolver {
    pub(super) fn prepare_tools<'a>(
        &'a self,
        request: &'a NodeExecutionRequest,
    ) -> ToolResolution<'a> {
        Box::pin(async move {
            // The error type is named once, here: every `?` inside then converts a
            // host-side string into `ResolutionError::Host`, while the environment
            // sites keep their own cause.
            let resolved: Result<ResolvedNodeTools, ResolutionError> = async move {
                let environment = self.tool_environment()?;
                // The first fallible step names the error type, so every later `?`
                // converts a host-side reason into `ResolutionError::Host` while the
                // environment sites keep their own cause.
                let (mut mounted, prepared) = self.select_invocation_plugins(request).await?;
                mounted.bindings = prepared.bindings;
                let PreparedNodeLaunch {
                    workspace,
                    readonly_inputs,
                    sandbox,
                    conversation,
                } = self.prepare_tool_launch(request, &environment, prepared.mounts)?;
                let mut mcp = prepared.mcp;
                mcp.environment = environment.clone();
                mcp.startup_cancellation = Some(request.cancellation.clone());
                mcp.authorize_oauth(
                    self.oauth_owner_for_run(&request.key.run_id)?,
                    mounted.network,
                )
                .await?;
                let plugin_views = prepared.views;
                let mut disclosure = prepared.disclosure;
                self.remember_plugin_observation(
                    &request.key,
                    plugin_views
                        .iter()
                        .filter_map(|(id, plugin)| {
                            plugin
                                .declaration_digest()
                                .map(|digest| (id.clone(), digest.to_owned()))
                        })
                        .collect(),
                    mounted
                        .bindings
                        .iter()
                        .map(|binding| (binding.id.clone(), binding.digest.clone()))
                        .collect(),
                    &mounted,
                );
                let plugins = self
                    .tools
                    .assemble_with_sandbox(
                        &mcp,
                        &mounted.bindings,
                        mounted.network,
                        crate::tool_host::SandboxLaunch {
                            sandbox: &sandbox,
                            workspace: &workspace,
                            readonly_inputs: &readonly_inputs,
                            plugin_views: &plugin_views,
                        },
                    )
                    .await?;
                disclosure.attach_tools(plugins.tool_owners());
                // Output beyond the stream previews is retained host-side and mounted
                // read-only at /spill, so a model can read what was cut off.
                let spill = crate::node_tools::SpillDirectory::new(
                    self.local_inputs
                        .state_root()
                        .join("spill")
                        .join(&request.key.run_id)
                        .join(
                            workspace
                                .file_name()
                                .map(|name| name.to_string_lossy().into_owned())
                                .unwrap_or_else(|| request.key.node_id.clone()),
                        ),
                    "/spill",
                );
                let node_tools = crate::node_tools::NodeTools::new(
                    crate::channel_tools::wrap(
                        plugins,
                        &mounted.bindings,
                        request.key.clone(),
                        self.local_inputs.state_root().to_path_buf(),
                        conversation.as_ref().map(|value| value.reply_node.clone()),
                        workspace.clone(),
                        readonly_inputs.clone(),
                        request.cancellation.clone(),
                    ),
                    sandbox,
                    workspace,
                    readonly_inputs,
                    request.cancellation.clone(),
                )
                .with_spill(spill)
                .with_environment(environment)
                .with_network(mounted.network)
                .with_timeout_ceiling(crate::node_tools::command_timeout_ceiling(
                    request.wall_time_limit_seconds,
                ));
                let tools =
                    std::sync::Arc::new(node_tools) as std::sync::Arc<dyn anchor_runtime::ToolPort>;
                // Tools a node's completion and continuity depend on stay listed; the rest
                // (plugins, channel tools) becomes reachable on demand.
                let mut always_visible = vec![
                    crate::node_tools::RUN_TOOL_NAME.to_owned(),
                    crate::node_tools::READ_TOOL_NAME.to_owned(),
                    crate::node_tools::EDIT_TOOL_NAME.to_owned(),
                ];
                let tools = match conversation {
                    Some(conversation) => {
                        always_visible
                            .push(crate::node_host::conversation::HISTORY_TOOL.to_owned());
                        self.conversation_tools(tools, &request.key, conversation)
                    }
                    None => tools,
                };
                let (tools, plugin_instructions) = crate::tool_disclosure::maybe_wrap(
                    tools,
                    crate::tool_disclosure::always_visible(always_visible),
                    disclosure,
                );
                Ok::<_, ResolutionError>(ResolvedNodeTools {
                    tools,
                    plugins: mounted.bindings,
                    plugin_instructions,
                })
            }
            .await;
            resolved
        })
    }
}

/// Invocation-local data: paths and grants prepared before any OAuth/MCP startup.
struct PreparedNodeLaunch {
    workspace: PathBuf,
    readonly_inputs: Vec<anchor_runtime::ReadOnlyInput>,
    sandbox: std::sync::Arc<BubblewrapSandbox>,
    conversation: Option<conversation::Conversation>,
}

impl HostIoResolver {
    async fn select_invocation_plugins(
        &self,
        request: &NodeExecutionRequest,
    ) -> Result<(MountedPlugins, crate::plugin_view::PreparedPlugins), ResolutionError> {
        if let Some(source) =
            crate::assistant::source(self.local_inputs.state_root(), &request.key)?
        {
            let binding = crate::assistant::binding(self.local_inputs.state_root(), &request.key)?
                .ok_or_else(|| "assistant invocation has no Turn binding".to_owned())?;
            self.freeze_assistant_plugins(
                &request.key,
                &binding.turn,
                false,
                request.cancellation.clone(),
            )
            .await?;
            if source.work_node != request.key.node_id {
                return Err("only the assistant work node may prepare Plugin tools"
                    .to_owned()
                    .into());
            }
        }
        let admitted = self
            .load_run(&request.key.run_id)
            .map_err(|error| {
                ResolutionError::Host(format!("load admitted Run Plugin bindings: {error}"))
            })?
            .ok_or_else(|| {
                ResolutionError::Host(format!(
                    "Run `{}` has no durable Plugin binding record",
                    request.key.run_id
                ))
            })?;
        if admitted.graph_digest != request.key.graph_digest
            || !admitted.plugin_bindings_initialized
        {
            return Err(ResolutionError::Host(format!(
                "Run `{}` has no matching initialized Plugin binding record",
                request.key.run_id
            )));
        }
        // The set this invocation actually mounts: the Graph's
        // defaults filtered by the Session's switches and frozen for
        // this Turn. Everything below uses this set, so there is one
        // filter point and not several.
        let mount_lease = match &self.library_root {
            Some(root)
                if !request.plugins.is_empty() || self.session_turn(&request.key)?.is_some() =>
            {
                let access = crate::dependency_provision::DependencyAccess {
                    network: false,
                    cancellation: request.cancellation.clone(),
                };
                Some(
                    crate::application::plugins::plugin_publication_lease(
                        root,
                        &access,
                        "invocation mount selection",
                    )
                    .await
                    .map_err(|error| format!("Plugin mount selection is unavailable: {error:?}"))?,
                )
            }
            _ => None,
        };
        let mounted = self.mounted_plugins_unlocked(request)?;
        drop(mount_lease);
        self.validate_invocation_plugins(&admitted, request, &mounted.bindings)?;
        let prepared = crate::plugin_view::prepare_plugins_for_owner_async(
            mounted.bindings.clone(),
            self.library_root.clone(),
            self.tools.as_ref().clone(),
            request.cancellation.clone(),
            self.oauth_owner_for_run(&request.key.run_id)?
                .map(|owner| (self.local_inputs.state_root().to_path_buf(), owner)),
        )
        .await
        .map_err(|error| ResolutionError::Graph(error.into_graph_error()))?;
        self.validate_invocation_plugins(&admitted, request, &prepared.bindings)?;
        Ok((mounted, prepared))
    }

    fn prepare_tool_launch(
        &self,
        request: &NodeExecutionRequest,
        environment: &crate::tool_environment::ToolEnvironment,
        plugin_mounts: Vec<anchor_runtime::ReadOnlyInput>,
    ) -> Result<PreparedNodeLaunch, ResolutionError> {
        let workspace = self.workspace(request)?;
        let mut local_inputs = self.local_input_mounts(&request.key)?;
        let conversation = self.conversation(&request.key)?;
        if let Some(conversation) = &conversation
            && let Some(mount) = self.previous_mount(&request.key, conversation)?
        {
            local_inputs.push(mount);
        }
        // Everything this invocation mounts is granted on the invocation
        // itself: the Run policy is the deployment floor, not a list of
        // per-Run trees (a parent Run cannot know its children's Plugins or
        // environments).
        let mut grants = local_inputs.clone();
        grants.extend(plugin_mounts.iter().cloned());
        let sandbox = std::sync::Arc::new(
            environment
                .grant_sandbox(&self.sandbox)?
                .with_readonly_grants(&grants)
                .map_err(|error| error.to_string())?,
        );
        let mut readonly_inputs = self
            .artifacts
            .input_mounts(
                &request.input_commits,
                &request.key.run_id,
                &request.key.graph_digest,
            )
            .map_err(|error| error.to_string())?;
        readonly_inputs.extend(plugin_mounts);
        readonly_inputs.extend(local_inputs);
        Ok(PreparedNodeLaunch {
            workspace,
            readonly_inputs,
            sandbox,
            conversation,
        })
    }
}
