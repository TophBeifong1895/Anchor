//! Task networking is a Host permission, exposed through the `network` Plugin.
//! Requirements are derived from mounted Plugins; they never write a Session
//! switch or override a human's explicit network denial.

use std::{collections::BTreeMap, path::Path};

use anchor_graph_host::{FilePluginCatalog, PluginCatalog, PluginDefinition};
use anchor_runtime::graph::PluginBinding;

pub(crate) const NETWORK_PLUGIN: &str = "network";

pub(crate) fn requires_network(plugin: &PluginDefinition) -> bool {
    plugin.network
        || plugin
            .mcp_servers
            .iter()
            .any(|server| server.config.get("url").is_some())
}

pub(crate) struct NetworkResolution {
    pub(crate) enabled: bool,
    pub(crate) allowed: bool,
    pub(crate) enabled_by: Vec<String>,
    pub(crate) errors: BTreeMap<String, String>,
}

pub(crate) fn resolve(root: &Path, mounted: &[String], permission: bool) -> NetworkResolution {
    let catalog = FilePluginCatalog::new(root);
    let mut enabled_by = mounted
        .iter()
        .filter(|id| id.as_str() != NETWORK_PLUGIN)
        .filter(|id| {
            catalog
                .definition(id)
                .is_ok_and(|plugin| requires_network(&plugin))
        })
        .cloned()
        .collect::<Vec<_>>();
    enabled_by.sort();
    enabled_by.dedup();
    let enabled = mounted.iter().any(|id| id == NETWORK_PLUGIN) || !enabled_by.is_empty();
    let problem = if !enabled {
        None
    } else if !permission {
        Some("当前任务已禁止联网".to_owned())
    } else {
        match catalog.definition(NETWORK_PLUGIN) {
            Ok(plugin) if plugin.network => {
                crate::plugin_view::prepare_for_plugin(root, NETWORK_PLUGIN)
                    .err()
                    .map(|error| error.to_string())
            }
            _ => Some("联网能力未安装或声明无效：需要 network Plugin（network: true）".to_owned()),
        }
    };
    let allowed = enabled && problem.is_none();
    let mut errors = BTreeMap::new();
    if let Some(problem) = problem {
        errors.insert(NETWORK_PLUGIN.to_owned(), problem.clone());
        for id in &enabled_by {
            errors.insert(id.clone(), format!("该插件需要联网：{problem}"));
        }
    }
    NetworkResolution {
        enabled,
        allowed,
        enabled_by,
        errors,
    }
}

/// Apply the same capability resolution the Session UI reports. Network-needed
/// tools stay absent when denied; offline tools still run in a network namespace
/// without host connectivity.
pub(crate) fn apply(
    root: &Path,
    bindings: &mut Vec<PluginBinding>,
    permission: bool,
    reasons: &mut Vec<String>,
) -> Result<bool, String> {
    let ids = bindings
        .iter()
        .map(|binding| binding.id.clone())
        .collect::<Vec<_>>();
    let resolution = resolve(root, &ids, permission);
    bindings.retain(|binding| {
        if let Some(problem) = resolution.errors.get(&binding.id) {
            reasons.push(format!("Plugin `{}`: {problem}", binding.id));
            false
        } else {
            true
        }
    });
    if resolution.allowed && !bindings.iter().any(|binding| binding.id == NETWORK_PLUGIN) {
        let binding = FilePluginCatalog::new(root)
            .resolve(&[NETWORK_PLUGIN.to_owned()])
            .map_err(|error| error.to_string())?
            .pop()
            .ok_or_else(|| "network Plugin is unavailable".to_owned())?;
        bindings.push(binding);
    }
    Ok(resolution.allowed)
}
