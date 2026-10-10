//! Who uses an environment name right now, and whether the assistant itself does.

use super::*;

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
        scanned: usize,
        total: usize,
        missing: Vec<String>,
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

/// The scan bound for a derived impact report.
const ENVIRONMENT_REFERENCE_LIMIT: usize = 200;

impl RunApplication {
    /// Who references one environment name right now.
    ///
    /// This reads the *editable* Graph definitions, not the compiled snapshots:
    /// an impact report may only be a derived view, and the operator's edits are
    /// what a name switch affects next. No reverse index is stored anywhere.
    /// Who uses this environment name right now.
    ///
    /// The site enumeration is the kernel's (`declared_environment_sites`), so
    /// this report cannot disagree with what admission would freeze: it loads
    /// each Graph as a bundle and reads the same list freezing resolves.
    pub(crate) fn environment_references(
        &self,
        name: &str,
    ) -> Result<EnvironmentReferences, ApplicationError> {
        // The deployment's configured bundle may live outside the editable
        // catalog; both are places a run can come from, so both are scanned.
        let mut roots = vec![self.catalog_root.clone()];
        if let Some(bundle) = std::env::var_os("ANCHOR_RUNNER_BUNDLE_ROOT").map(PathBuf::from)
            && bundle.is_dir()
            && !roots.contains(&bundle)
        {
            roots.push(bundle);
        }
        let mut graphs = Vec::new();
        let mut missing = Vec::new();
        for root in &roots {
            let entries = match std::fs::read_dir(root) {
                Ok(entries) => entries,
                Err(error) => {
                    // An unreadable root is not "nobody uses this".
                    return Ok(EnvironmentReferences {
                        name: name.to_owned(),
                        nodes: Vec::new(),
                        servers: Vec::new(),
                        coverage: EnvironmentCoverage::Unknown {
                            reason: format!("{} could not be read: {error}", root.display()),
                        },
                    });
                }
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let graph = entry.file_name().to_string_lossy().into_owned();
                if !path.join("graph.json").is_file() {
                    continue;
                }
                if entry.file_type().is_ok_and(|kind| kind.is_symlink()) {
                    // A symlinked Graph is a Graph too, but resolving it is the
                    // loader's business; count it rather than dropping it.
                    missing.push(graph);
                    continue;
                }
                graphs.push(graph);
            }
        }
        graphs.sort();
        graphs.dedup();
        let total = graphs.len();
        let scanned = total.min(ENVIRONMENT_REFERENCE_LIMIT);
        graphs.truncate(ENVIRONMENT_REFERENCE_LIMIT);

        let mut nodes = Vec::new();
        let mut servers = Vec::new();
        for graph in &graphs {
            let (_, bundle) = match self.load_graph(graph) {
                Ok(bundle) => bundle,
                Err(_) => {
                    // A Graph this host cannot load is one it cannot report on.
                    missing.push(graph.clone());
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
                scanned,
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
    use super::super::test_support::*;
    use anchor_graph_host::PluginCatalog;

    #[test]
    fn the_impact_scan_finds_node_and_plugin_server_references() {
        let root = tempfile::tempdir().unwrap();
        let application = application(root.path());
        // The bundle loader needs the run store the application owns.
        std::fs::create_dir_all(root.path().join("state/runs")).unwrap();
        // A real, loadable bundle: the Plugin lives in the bundle's own
        // `plugins/` directory and the manifest summarises its真实 digest, so the
        // scan reads exactly what admission would freeze.
        let graph = root.path().join("used");
        std::fs::create_dir_all(graph.join("plugins/demo")).unwrap();
        std::fs::write(
            graph.join("plugins/demo/plugin.json"),
            r#"{"name":"Demo","mcpServers":{"probe":{"command":"bin/run","anchor":{"environment":"research"}}}}"#,
        )
        .unwrap();
        std::fs::write(graph.join("plugins/demo/bin/run"), "#!/bin/sh\n").ok();
        // The manifest must summarise exactly what the catalog resolves.
        let binding = anchor_graph_host::FilePluginCatalog::new(&graph)
            .resolve(&["demo".to_owned()])
            .unwrap()
            .remove(0);
        std::fs::write(
            graph.join("graph.json"),
            serde_json::json!({
                "objective": "impact",
                "entry": "worker",
                "agents": {"worker": {"model": "test"}},
                "ops": {},
                "nodes": [
                    {"id": "worker", "agent": "worker", "plugins": ["demo"]},
                    {"id": "other", "agent": "worker", "environment": "research"}
                ],
                "edges": []
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            graph.join("manifest.json"),
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
        assert!(matches!(
            references.coverage,
            crate::application::environments::EnvironmentCoverage::Complete
        ));
        let unused = application.environment_references("unused").unwrap();
        assert!(unused.nodes.is_empty() && unused.servers.is_empty());
    }
}
