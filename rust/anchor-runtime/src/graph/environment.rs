//! Freezing a Graph's declared dependency environments into Run facts.
//!
//! The traversal lives here, once, so parent admission, child-Run admission and
//! record validation cannot drift: every declaration is turned into exactly the
//! site-keyed binding that `GraphRunRecord::validate` requires.

use super::*;

/// Resolve one declared environment name to a published identity.
///
/// The kernel never reads a store: the host supplies this, which keeps files and
/// deployment layout out of Runtime.
pub type NameResolver<'a> = dyn Fn(&str) -> Result<String, String> + 'a;

/// Freeze every environment this Graph declares, keyed by its use site.
pub fn freeze_environments(
    snapshot: &GraphSnapshot,
    plugin_bindings: &BTreeMap<String, PluginBinding>,
    resolve: &NameResolver<'_>,
) -> Result<BTreeMap<String, EnvironmentBinding>, String> {
    let mut bindings = BTreeMap::new();
    for node in &snapshot.nodes {
        if let Some(name) = &node.environment {
            let id = resolve(name)?;
            let binding = EnvironmentBinding::node_task(node.id.clone(), name.clone(), id);
            if let Some(site) = binding.site.clone() {
                bindings.insert(site.key(), binding);
            }
        }
        // A Plugin bound to several nodes freezes one site per node: the same
        // server may run in different environments in different nodes.
        for plugin in &node.plugins {
            let Some(bound) = plugin_bindings.get(plugin) else {
                continue;
            };
            for (server, name) in &bound.environments {
                let id = resolve(name)?;
                let site = EnvironmentSite::PluginServer {
                    node: node.id.clone(),
                    plugin: plugin.clone(),
                    server: server.clone(),
                };
                let key = site.key();
                bindings.insert(
                    key,
                    EnvironmentBinding {
                        site: Some(site),
                        name: name.clone(),
                        id,
                    },
                );
            }
        }
    }
    Ok(bindings)
}

/// Whether a name may be used for a dependency environment.
///
/// The Library owns the deployment namespace, the authoring layer validates the
/// Graph field and the host tools validate a request, so the rule lives here, in
/// the kernel, instead of being re-implemented at each of them.
pub fn is_environment_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-_.".contains(character))
}
