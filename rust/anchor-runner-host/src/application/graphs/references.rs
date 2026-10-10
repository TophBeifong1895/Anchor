//! Who uses an environment name right now, and whether the assistant itself does.
//!
//! Both answers are *Graph* questions: the scan compiles the Graphs a run can come
//! from and reads the kernel's declared sites, and the self-guard follows a
//! Graph's `Op.call` edges. They lived in the environment application module
//! because they are about environment names, but they depend on the Graph
//! catalog and the bundle loader and nothing else.

use super::super::{ApplicationError, RunApplication};
use serde::{Deserialize, Serialize};

impl RunApplication {
    /// Whether this assistant's own Graph — or any Graph it calls — uses one
    /// environment name, on a node or on a bound Plugin server.
    ///
    /// The self-guard has to follow `Op.call`: the assistant may author a called
    /// Graph, so checking only its own top-level bundle would let it choose the
    /// dependency content of a Graph it then runs. An unreadable Graph refuses
    /// (fail closed) rather than reporting "no".
    pub(crate) fn assistant_uses_environment_name(
        &self,
        graph_name: &str,
        name: &str,
    ) -> Result<bool, ApplicationError> {
        const MAX_GRAPHS: usize = 50;
        let mut pending = vec![graph_name.to_owned()];
        let mut visited = std::collections::BTreeSet::new();
        while let Some(graph) = pending.pop() {
            if !visited.insert(graph.clone()) {
                continue;
            }
            if visited.len() > MAX_GRAPHS {
                return Err(ApplicationError::Invalid(
                    "the assistant Graph calls too many Graphs to verify its own dependencies"
                        .to_owned(),
                ));
            }
            let (_, bundle) = self.load_graph(&graph).map_err(|error| {
                ApplicationError::Invalid(format!(
                    "Graph `{graph}` could not be read to verify the assistant's own dependencies: {error:?}"
                ))
            })?;
            let bindings = plugin_bindings(&bundle);
            if anchor_runtime::graph::declared_environment_sites(&bundle.snapshot, &bindings)
                .iter()
                .any(|declared| declared.name == name)
            {
                return Ok(true);
            }
            for node in &bundle.snapshot.nodes {
                // Follow Op.call edges.
                if let Some(op) = node.op.as_ref().and_then(|op| bundle.snapshot.ops.get(op))
                    && let Some(callee) = op
                        .get("call")
                        .and_then(|call| call.get("graph"))
                        .and_then(serde_json::Value::as_str)
                {
                    pending.push(callee.to_owned());
                }
            }
        }
        Ok(false)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EnvironmentReferences {
    pub(crate) name: String,
    /// Graphs whose node declares this environment.
    pub(crate) nodes: Vec<EnvironmentNodeReference>,
    /// Plugin servers whose manifest declares it, per node that binds them.
    pub(crate) servers: Vec<EnvironmentServerReference>,
    /// How much of the catalog this report actually saw. A report may only be
    /// read as "nobody uses this" when coverage is `Complete`.
    pub(crate) coverage: EnvironmentCoverage,
}

/// How complete one derived impact report is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum EnvironmentCoverage {
    Complete,
    /// The scan stopped early or could not read some definitions.
    Partial {
        /// How many Graphs this report considered before its bound.
        considered: usize,
        /// How many Graphs a run could come from.
        total: usize,
        missing: Vec<EnvironmentMissing>,
    },
    /// The report could not be produced at all.
    Unknown {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EnvironmentNodeReference {
    pub(crate) graph: String,
    pub(crate) node: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EnvironmentServerReference {
    pub(crate) graph: String,
    pub(crate) node: String,
    pub(crate) plugin: String,
    pub(crate) server: String,
}

/// One Graph this report could not read, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EnvironmentMissing {
    pub(crate) graph: String,
    pub(crate) reason: String,
}

/// The scan bound for a derived impact report.
const ENVIRONMENT_REFERENCE_LIMIT: usize = 200;

impl RunApplication {
    /// Who uses this environment name right now.
    ///
    /// The site enumeration is the kernel's (`declared_environment_sites`), so
    /// this report cannot disagree with what admission would freeze: it loads
    /// each Graph as a bundle and reads the same list freezing resolves. The
    /// candidate set is the Graphs a run can come from — the editable catalog
    /// plus the one bundle this deployment is configured to run — and anything
    /// this host cannot read is reported with its reason rather than dropped.
    pub(crate) fn environment_references(
        &self,
        name: &str,
    ) -> Result<EnvironmentReferences, ApplicationError> {
        let unknown = |reason: String| EnvironmentReferences {
            name: name.to_owned(),
            nodes: Vec::new(),
            servers: Vec::new(),
            coverage: EnvironmentCoverage::Unknown { reason },
        };
        let mut candidates: Vec<String> = Vec::new();
        if self.catalog_root.is_dir() {
            let entries = match std::fs::read_dir(&self.catalog_root) {
                Ok(entries) => entries,
                Err(error) => {
                    // An unreadable catalog is not "nobody uses this".
                    return Ok(unknown(format!(
                        "{} could not be read: {error}",
                        self.catalog_root.display()
                    )));
                }
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() && path.join("graph.json").is_file() {
                    candidates.push(entry.file_name().to_string_lossy().into_owned());
                }
            }
        } else if self.configured_graph.is_none() {
            return Ok(unknown(format!(
                "{} is not a readable Graph catalog",
                self.catalog_root.display()
            )));
        }
        if let Some((configured, _)) = &self.configured_graph {
            candidates.push(configured.clone());
        }
        candidates.sort();
        // The configured bundle may also be a child of the catalog; scanning it
        // twice would double-count its declarations.
        let mut seen_paths = std::collections::BTreeSet::new();
        candidates.retain(|graph| match self.graph_path_checked(graph) {
            Ok(path) => seen_paths.insert(path.canonicalize().unwrap_or(path)),
            Err(_) => true,
        });
        let total = candidates.len();
        let considered = total.min(ENVIRONMENT_REFERENCE_LIMIT);
        candidates.truncate(ENVIRONMENT_REFERENCE_LIMIT);

        let mut nodes = Vec::new();
        let mut servers = Vec::new();
        let mut missing = Vec::new();
        for graph in &candidates {
            let (_, bundle) = match self.load_graph(graph) {
                Ok(bundle) => bundle,
                Err(error) => {
                    // A Graph this host cannot load is one it cannot report on,
                    // and the report says which one and why.
                    missing.push(EnvironmentMissing {
                        graph: graph.clone(),
                        reason: error.to_string(),
                    });
                    continue;
                }
            };
            let bindings = plugin_bindings(&bundle);
            for declared in
                anchor_runtime::graph::declared_environment_sites(&bundle.snapshot, &bindings)
            {
                if declared.name != name {
                    continue;
                }
                match declared.site {
                    anchor_runtime::graph::EnvironmentSite::NodeTask { node } => {
                        nodes.push(EnvironmentNodeReference {
                            graph: graph.clone(),
                            node,
                        });
                    }
                    anchor_runtime::graph::EnvironmentSite::PluginServer {
                        node,
                        plugin,
                        server,
                    } => {
                        servers.push(EnvironmentServerReference {
                            graph: graph.clone(),
                            node,
                            plugin,
                            server,
                        });
                    }
                }
            }
        }
        nodes.sort_by(|left, right| (&left.graph, &left.node).cmp(&(&right.graph, &right.node)));
        nodes.dedup();
        servers.sort_by(|left, right| {
            (&left.graph, &left.node, &left.plugin, &left.server).cmp(&(
                &right.graph,
                &right.node,
                &right.plugin,
                &right.server,
            ))
        });
        servers.dedup();
        let coverage = if missing.is_empty() && total <= ENVIRONMENT_REFERENCE_LIMIT {
            EnvironmentCoverage::Complete
        } else {
            EnvironmentCoverage::Partial {
                considered,
                total,
                missing,
            }
        };
        Ok(EnvironmentReferences {
            name: name.to_owned(),
            nodes,
            servers,
            coverage,
        })
    }
}

/// The bundle's Plugin bindings, keyed by id.
fn plugin_bindings(
    bundle: &anchor_graph_host::LoadedGraphBundle,
) -> std::collections::BTreeMap<String, anchor_runtime::graph::PluginBinding> {
    bundle
        .plugins
        .iter()
        .map(|binding| (binding.id.clone(), binding.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::EnvironmentCoverage;
    use crate::application::environments::test_support::*;
    use anchor_graph_host::PluginCatalog;

    /// Write one loadable bundle that declares the named environment on a node.
    fn bundle(root: &std::path::Path, graph: &str, environment: &str) -> std::path::PathBuf {
        let directory = root.join(graph);
        std::fs::create_dir_all(directory.join("plugins/demo")).unwrap();
        std::fs::write(
            directory.join("plugins/demo/plugin.json"),
            r#"{"name":"Demo","mcpServers":{"probe":{"command":"bin/run","anchor":{"environment":"research"}}}}"#,
        )
        .unwrap();
        let binding = anchor_graph_host::FilePluginCatalog::new(&directory)
            .resolve(&["demo".to_owned()])
            .unwrap()
            .remove(0);
        std::fs::write(
            directory.join("graph.json"),
            serde_json::json!({
                "objective": "impact",
                "entry": "worker",
                "agents": {"worker": {"model": "test"}},
                "ops": {},
                "nodes": [
                    {"id": "worker", "agent": "worker", "plugins": ["demo"]},
                    {"id": "other", "agent": "worker", "environment": environment}
                ],
                "edges": []
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            directory.join("manifest.json"),
            serde_json::json!({
                "format": 1,
                "graph": "graph.json",
                "plugins": [{
                    "id": "demo",
                    "digest": binding.digest,
                    "resources": binding.resources,
                    "mcp_servers": binding.mcp_servers,
                }]
            })
            .to_string(),
        )
        .unwrap();
        directory
    }

    #[test]
    fn the_impact_scan_finds_node_and_plugin_server_references() {
        let root = tempfile::tempdir().unwrap();
        let application = application(root.path());
        bundle(root.path(), "used", "research");

        let references = application.environment_references("research").unwrap();
        assert!(
            references
                .nodes
                .iter()
                .any(|reference| reference.graph == "used" && reference.node == "other"),
            "{references:?}"
        );
        assert!(
            references.servers.iter().any(|reference| {
                reference.graph == "used"
                    && reference.node == "worker"
                    && reference.plugin == "demo"
                    && reference.server == "probe"
            }),
            "{references:?}"
        );
        assert!(matches!(references.coverage, EnvironmentCoverage::Complete));
        let unused = application.environment_references("unused").unwrap();
        assert!(unused.nodes.is_empty() && unused.servers.is_empty());
    }

    #[test]
    fn a_configured_bundle_outside_the_catalog_is_still_reported() {
        // The deployment's bundle lives elsewhere and is ONE bundle, not a
        // directory of bundles: the Graph that will actually run must appear in
        // the report, or the approving human is told "nobody uses this".
        let root = tempfile::tempdir().unwrap();
        let catalog = root.path().join("catalog");
        std::fs::create_dir_all(&catalog).unwrap();
        let deployment = root.path().join("deploy");
        bundle(root.path(), "deploy", "research");
        let application =
            application(&catalog).with_configured_graph("deployed".to_owned(), deployment.clone());

        let references = application.environment_references("research").unwrap();
        assert!(
            references
                .nodes
                .iter()
                .any(|reference| reference.graph == "deployed" && reference.node == "other"),
            "{references:?}"
        );
        assert!(
            matches!(references.coverage, EnvironmentCoverage::Complete),
            "{references:?}"
        );
        assert_eq!(references.servers.len(), 1, "{references:?}");
    }

    #[test]
    fn a_graph_that_cannot_be_loaded_is_reported_with_its_reason() {
        let root = tempfile::tempdir().unwrap();
        let application = application(root.path());
        // A Graph directory without a manifest cannot be loaded, so the report
        // may not claim to be complete, and it must name the Graph.
        std::fs::create_dir_all(root.path().join("broken")).unwrap();
        std::fs::write(
            root.path().join("broken/graph.json"),
            r#"{"objective":"broken","entry":"only","agents":{"worker":{"model":"test"}},"ops":{},"nodes":[{"id":"only","agent":"worker"}],"edges":[]}"#,
        )
        .unwrap();

        let references = application.environment_references("research").unwrap();
        match references.coverage {
            EnvironmentCoverage::Partial { missing, .. } => {
                assert_eq!(missing.len(), 1, "{missing:?}");
                assert_eq!(missing[0].graph, "broken");
                assert!(!missing[0].reason.is_empty(), "{missing:?}");
            }
            other => panic!("expected partial coverage, got {other:?}"),
        }
    }
}
