//! Read-only Graph bundle admission and reference validation.

use crate::{invalid, resources::reject_symlink_components};
use anchor_runtime::graph::{GraphError, GraphSnapshot};
use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;

/// Strict metadata for a distributable, secret-free Graph directory.
///
/// Format 1 contains one `graph.json`, this `manifest.json`, and explicitly
/// declared Plugin resource directories at `plugins/<id>/`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphBundleManifest {
    pub format: u32,
    pub graph: String,
    pub plugins: Vec<BundlePluginReference>,
}

/// One Plugin a Graph bundle *references*.
///
/// A bundle never carries Plugin bytes or digests: the Plugin exists exactly once,
/// in the Library's `plugins/<id>`, and the runtime resolves it there. A bundle
/// that still holds `digest`/`resources`/`mcp_servers` (or a `plugins/`
/// directory) is refused, because it would be a second copy of the Plugin.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundlePluginReference {
    pub id: String,
}

/// Admitted expanded Graph snapshot and secret-free Plugin identities.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedGraphBundle {
    /// The editable graph definition as it appears in `graph.json`. Keep this
    /// beside the compiled Runtime snapshot so hosts can return and persist
    /// author-only fields without feeding them into Runtime execution.
    pub authoring_definition: Value,
    pub snapshot: GraphSnapshot,
    /// The Plugin ids this bundle references. Their code and their views live in
    /// the Library, which is the only place the runtime resolves them from.
    pub plugins: Vec<String>,
}

/// Read-only loader for a single standalone Graph bundle directory.
pub struct FileGraphBundleLoader {
    root: PathBuf,
    /// The deployment's Library root, when the caller knows it.
    ///
    /// A deployment may use the Library root itself as a Graph bundle directory
    /// (a single-root development deployment does); in that case `plugins/` is the
    /// Library's own Plugin home rather than Plugin bytes carried by the bundle.
    /// With three separate roots — the standard deployment — the bundle is a
    /// different directory and any `plugins/` in it is refused.
    library_root: Option<PathBuf>,
}

impl FileGraphBundleLoader {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            library_root: None,
        }
    }

    /// Tell the loader where the Library's Plugin home is.
    pub fn with_library_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.library_root = Some(root.into());
        self
    }

    /// Load and admit `manifest.json`, `graph.json`, and explicitly declared
    /// `plugins/<id>/` assets. This function never dispatches Graph work.
    pub fn load(&self) -> Result<LoadedGraphBundle, GraphError> {
        let root = self.root.canonicalize()?;
        if !root.is_dir() {
            return Err(invalid("Graph bundle root must be a directory".into()));
        }
        let manifest_path = root.join("manifest.json");
        reject_symlink_components(&root, &manifest_path)?;
        let raw = std::fs::read(&manifest_path)?;
        let manifest_value: Value =
            serde_json::from_slice(&raw).map_err(GraphError::SnapshotDecode)?;
        // A bundle from the previous model carries the Plugins themselves. That is
        // a second copy of a Plugin that must exist exactly once, so it is refused
        // with the one thing the operator can do about it.
        if let Some(fields) = legacy_plugin_fields(&manifest_value) {
            return Err(invalid(format!(
                "this Graph bundle still carries Plugin {fields}; a bundle references Plugins by id only, so re-export the Graph from the current host"
            )));
        }
        let manifest: GraphBundleManifest =
            serde_json::from_value(manifest_value).map_err(GraphError::SnapshotDecode)?;
        if manifest.format != 1 || manifest.graph != "graph.json" {
            return Err(invalid(
                "unsupported Graph bundle format or graph path".into(),
            ));
        }
        let graph_path = root.join("graph.json");
        reject_symlink_components(&root, &graph_path)?;
        let authoring_definition: Value = serde_json::from_slice(&std::fs::read(&graph_path)?)
            .map_err(GraphError::SnapshotDecode)?;
        let snapshot = GraphSnapshot::from_authoring(authoring_definition.clone())?;

        let expected = snapshot
            .nodes
            .iter()
            .flat_map(|node| node.plugins.iter().map(|reference| reference.id.clone()))
            .collect::<std::collections::BTreeSet<_>>();
        let ids = manifest
            .plugins
            .iter()
            .map(|plugin| plugin.id.clone())
            .collect::<Vec<_>>();
        let declared = ids
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        if expected != declared || declared.len() != ids.len() {
            return Err(invalid(
                "manifest Plugin set must exactly match Graph references".into(),
            ));
        }

        // A bundle holds a Graph and nothing else: no Plugin bytes, no extra
        // top-level payload that could be carried into deployment unnoticed. The
        // one exception is a deployment that uses its Library root as the bundle
        // directory: there `plugins/` *is* the Library's Plugin home.
        let library_here = self
            .library_root
            .as_ref()
            .and_then(|library| library.canonicalize().ok())
            .is_some_and(|library| library == root);
        if root.join("plugins").exists() && !library_here {
            return Err(invalid(
                "this Graph bundle carries Plugin bytes under `plugins/`; a Plugin exists once, in the Library, so re-export the Graph from the current host"
                    .to_owned(),
            ));
        }
        let mut actual = std::fs::read_dir(&root)?
            .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
            .collect::<Result<Vec<_>, _>>()?;
        actual.sort_unstable();
        let mut allowed = vec!["graph.json".to_owned(), "manifest.json".to_owned()];
        // A deployment that uses its Library root as the bundle directory has the
        // Plugin home next to the Graph; it is not part of the bundle's own
        // content, and a bundle without it is perfectly valid.
        if library_here && root.join("plugins").is_dir() {
            allowed.push("plugins".to_owned());
        }
        allowed.sort_unstable();
        if actual != allowed {
            return Err(invalid(
                "Graph bundle contains undeclared top-level resources".into(),
            ));
        }
        Ok(LoadedGraphBundle {
            authoring_definition,
            snapshot,
            plugins: ids,
        })
    }
}

/// The Plugin fields a pre-reference bundle's manifest carries, if any.
///
/// Reported so the refusal can name what is wrong instead of a serde message
/// about an unexpected field.
fn legacy_plugin_fields(manifest: &Value) -> Option<String> {
    let plugins = manifest.get("plugins")?.as_array()?;
    let mut found = Vec::new();
    for plugin in plugins {
        let object = plugin.as_object()?;
        for key in object.keys() {
            if key != "id" && !found.contains(key) {
                found.push(key.clone());
            }
        }
    }
    (!found.is_empty()).then(|| found.join(", "))
}

#[cfg(test)]
#[path = "bundle_tests.rs"]
mod tests;
