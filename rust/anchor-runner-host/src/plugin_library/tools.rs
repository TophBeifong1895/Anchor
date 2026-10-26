//! Shared preparation steps; callers own grouping, order and error handling.
use super::Catalog;
use crate::plugin_view::{PreparedPlugin, Unusable};
use anchor_runtime::ReadOnlyInput;
use anchor_runtime::graph::PluginBinding;
use std::{collections::BTreeMap, path::PathBuf};

/// Everything a Host invocation derives from its verified Plugin definitions.
/// The bindings, MCP declarations, mounts and views all describe the same read.
pub(crate) struct PreparedPlugins {
    pub(crate) bindings: Vec<PluginBinding>,
    pub(crate) mcp: crate::tool_host::McpToolConfig,
    pub(crate) mounts: Vec<ReadOnlyInput>,
    pub(crate) views: BTreeMap<String, PreparedPlugin>,
    pub(crate) disclosure: crate::plugin_disclosure::PluginDisclosureCatalog,
}

/// Prepare current Library code under the selected authorization baseline.
/// The publication lease covers every filesystem read and is released before
/// callers perform OAuth authorization or start any MCP connection.
#[cfg(test)]
pub(crate) async fn prepare_plugins_async(
    bindings: Vec<PluginBinding>,
    library_root: Option<PathBuf>,
    fixture: crate::tool_host::PluginToolHost,
    cancellation: anchor_runtime::Cancellation,
) -> Result<PreparedPlugins, Unusable> {
    prepare_plugins_for_owner_async(bindings, library_root, fixture, cancellation, None).await
}

pub(crate) async fn prepare_plugins_for_owner_async(
    bindings: Vec<PluginBinding>,
    library_root: Option<PathBuf>,
    fixture: crate::tool_host::PluginToolHost,
    cancellation: anchor_runtime::Cancellation,
    credentials: Option<(PathBuf, String)>,
) -> Result<PreparedPlugins, Unusable> {
    fixture
        .for_bindings(&bindings)
        .map_err(|error| Unusable::recorded(error.to_string()))?;
    let needs_library = bindings
        .iter()
        .any(|binding| !crate::tool_host::is_fake_fixture(binding));
    let lease = if needs_library {
        let root = library_root.as_ref().ok_or_else(|| {
            Unusable::unconfigured("Plugin preparation requires the deployment Library root")
        })?;
        let access = crate::dependency_provision::DependencyAccess {
            network: false,
            cancellation: cancellation.clone(),
        };
        Some(
            crate::plugin_library::publication_lease(root, &access, "invocation preparation")
                .await
                .map_err(|error| Unusable::unconfigured(format!("{error:?}")))?,
        )
    } else {
        None
    };
    tokio::task::spawn_blocking(move || {
        let _lease = lease;
        let root = library_root.unwrap_or_default();
        let catalog = Catalog::new(&root);
        let mut definitions = BTreeMap::new();
        let mut current = Vec::with_capacity(bindings.len());
        let mut mounts = Vec::new();
        let mut views = BTreeMap::new();
        let mut disclosure = crate::plugin_disclosure::PluginDisclosureCatalog::default();
        for binding in &bindings {
            if cancellation.load(std::sync::atomic::Ordering::Acquire) {
                return Err(Unusable::unconfigured("Plugin preparation was cancelled"));
            }
            if crate::tool_host::is_fake_fixture(binding) {
                current.extend(
                    fixture
                        .resolve_fixture_plugins(std::slice::from_ref(&binding.id))
                        .map_err(Unusable::recorded)?,
                );
                continue;
            }
            let definition = catalog
                .checked_definition(
                    &binding.id,
                    super::ExpectedDefinition::Authorization(binding),
                )
                .map_err(|error| match error {
                    super::DefinitionError::Read(error) => Unusable::recorded(error.to_string()),
                    super::DefinitionError::Mismatch => Unusable::recorded(format!(
                        "Plugin `{}` authorization changed during preparation",
                        binding.id,
                    )),
                })?;
            let current_binding = definition.binding();
            views.insert(
                binding.id.clone(),
                PreparedPlugin::from_definition(&root, &definition)?,
            );
            mounts.push(ReadOnlyInput::new(
                &definition.directory,
                format!("/plugins/{}", definition.id),
            ));
            current.push(current_binding);
            disclosure.insert(definition.clone());
            definitions.insert(binding.id.clone(), definition);
        }
        let mcp = crate::tool_host::McpToolConfig::from_definitions_for_owner_execution(
            &root,
            &current,
            &definitions,
            credentials
                .as_ref()
                .map(|(data, owner)| (data.as_path(), owner.as_str())),
        )
        .map_err(Unusable::from)?;
        Ok(PreparedPlugins {
            bindings: current,
            mcp,
            mounts,
            views,
            disclosure,
        })
    })
    .await
    .map_err(|error| {
        Unusable::unconfigured(format!("Plugin invocation preparation failed: {error}"))
    })?
}

/// Connect one already prepared group, then associate live tools with the same
/// disclosure catalog. Grouping and soft/hard failure decisions stay at callers.
pub(crate) async fn connect(
    host: &crate::tool_host::PluginToolHost,
    mcp: &crate::tool_host::McpToolConfig,
    bindings: &[PluginBinding],
    network: bool,
    launch: Option<crate::tool_host::SandboxLaunch<'_>>,
    disclosure: &mut crate::plugin_disclosure::PluginDisclosureCatalog,
) -> Result<std::sync::Arc<crate::tool_host::CombinedPluginTools>, crate::tool_host::PreparationError>
{
    let port = host
        .assemble_for_execution(mcp, bindings, network, launch)
        .await?;
    disclosure.attach_tools(port.tool_owners());
    Ok(port)
}
