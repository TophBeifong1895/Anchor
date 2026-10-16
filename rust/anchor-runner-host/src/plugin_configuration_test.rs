//! An explicit management probe reuses the invocation's MCP preparation and
//! sandbox launch. It initializes and lists tools, and never calls a tool.
use anchor_graph_host::FilePluginCatalog;
use anchor_sandbox_bwrap::{BubblewrapPolicy, BubblewrapSandbox};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

pub(crate) async fn test(
    data: PathBuf,
    library: PathBuf,
    owner: String,
    plugin: String,
    authorization_digest: String,
    revision: u64,
) -> Result<Value, String> {
    if !crate::pilot_tools::identifier(&plugin) {
        return Err("invalid Plugin configuration ID".into());
    }
    let root = library.clone();
    let id = plugin.clone();
    let snapshot_data = data.clone();
    let snapshot_owner = owner.clone();
    let expected_authorization = authorization_digest.clone();
    // Reject stale requests before preparing dependencies or launching anything.
    // Both current authority and the owner-bound revision are checked again by
    // invocation preparation, which captures the material actually tested.
    let definition = tokio::task::spawn_blocking(move || {
        let definition = FilePluginCatalog::new(root)
            .definition(&id)
            .map_err(|_| "Plugin connection definition unavailable")?;
        if definition.authorization_digest != expected_authorization {
            return Err(
                "Plugin authorization changed; reload configuration before testing".to_owned(),
            );
        }
        let snapshot = crate::credential_store::environment_snapshot(
            &snapshot_data,
            &snapshot_owner,
            &definition,
        )?;
        if snapshot.revision != revision {
            return Err("Plugin configuration changed; reload before testing".to_owned());
        }
        Ok(definition)
    })
    .await
    .map_err(|_| "Plugin connection preparation unavailable")??;
    let cancellation = Arc::new(AtomicBool::new(false));
    let prepared = crate::plugin_view::prepare_plugins_for_owner_async(
        vec![definition.binding()],
        Some(library.clone()),
        crate::tool_host::PluginToolHost::new(Vec::<String>::new()),
        cancellation.clone(),
        Some((data.clone(), owner.clone())),
    )
    .await
    .map_err(|_| "Plugin connection preparation failed; check configuration and dependencies")?;
    let mut mcp = prepared.mcp;
    if mcp
        .configuration_revisions
        .get(&plugin)
        .copied()
        .unwrap_or(0)
        != revision
    {
        return Err("Plugin configuration changed; reload before testing".into());
    }
    mcp.startup_timeout = Some(Duration::from_secs(20));
    mcp.startup_cancellation = Some(cancellation.clone());
    // The management action explicitly authorizes this connection test. It
    // grants the same Plugin code/dependency mounts, with a disposable workspace.
    let sandbox_library = library.clone();
    let (workspace, sandbox, environment) = tokio::task::spawn_blocking(move || {
        let workspace = tempfile::tempdir().map_err(|_| "connection test workspace unavailable")?;
        let commands = std::env::var("ANCHOR_RUNNER_ALLOWED_COMMANDS").unwrap_or_default();
        let policy = BubblewrapPolicy::new(
            std::env::var_os("ANCHOR_BWRAP")
                .map(PathBuf::from)
                .unwrap_or_else(|| "bwrap".into()),
            commands.split(',').filter(|value| !value.is_empty()),
        )
        .authorize_workspace_root(workspace.path())
        .authorize_readonly_destination_root("/plugins")
        .allow_network();
        let environment = crate::tool_environment::ToolEnvironment::load(&sandbox_library)
            .map_err(|_| "connection test tool environment unavailable")?;
        let sandbox = BubblewrapSandbox::new(environment.authorize(policy))
            .map_err(|_| "connection test sandbox unavailable")?;
        Ok::<_, String>((workspace, sandbox, environment))
    })
    .await
    .map_err(|_| "connection test sandbox preparation failed")??;
    mcp.environment = environment;
    // OAuth owner is already the admitted binding identity, as in execution.
    mcp.authorize_oauth(Some(owner.clone()), true)
        .await
        .map_err(|_| "Plugin OAuth authorization unavailable")?;
    let results = mcp
        .test_connections(
            &prepared.bindings,
            crate::tool_host::SandboxLaunch {
                sandbox: &sandbox,
                workspace: workspace.path(),
                readonly_inputs: &prepared.mounts,
                plugin_views: &prepared.views,
            },
        )
        .await;
    let tested_binding = mcp.configuration_bindings.get(&plugin).cloned();
    let current = tokio::task::spawn_blocking(move || {
        let definition = FilePluginCatalog::new(&library).definition(&plugin).ok()?;
        if definition.authorization_digest != authorization_digest {
            return None;
        }
        let binding = definition.binding();
        let definitions = std::collections::BTreeMap::from([(plugin.clone(), definition)]);
        let current = crate::tool_host::McpToolConfig::from_definitions_for_owner(
            &library,
            &[binding],
            &definitions,
            Some((&data, &owner)),
        )
        .ok()?;
        Some(
            current
                .configuration_revisions
                .get(&plugin)
                .copied()
                .unwrap_or(0)
                == revision
                && current.configuration_bindings.get(&plugin) == tested_binding.as_ref(),
        )
    })
    .await
    .ok()
    .flatten()
    .unwrap_or(false);
    Ok(json!({"plugin":definition.id,"revision":revision,"results":results,"current":current}))
}
