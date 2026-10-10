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
            for node in &bundle.snapshot.nodes {
                if node.environment.as_deref() == Some(name) {
                    return Ok(true);
                }
                for plugin in &node.plugins {
                    let Some(binding) = bundle.plugins.iter().find(|binding| &binding.id == plugin)
                    else {
                        continue;
                    };
                    if binding
                        .environments
                        .values()
                        .any(|declared| declared == name)
                    {
                        return Ok(true);
                    }
                }
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
    pub(crate) fn environment_references(
        &self,
        library_root: &std::path::Path,
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
                let name_in_root = entry.file_name().to_string_lossy().into_owned();
                if !path.join("graph.json").is_file() {
                    continue;
                }
                // A symlinked Graph is a Graph too, but resolving it is the
                // loader's business; count it rather than dropping it.
                if entry.file_type().is_ok_and(|kind| kind.is_symlink()) {
                    missing.push(name_in_root);
                    continue;
                }
                graphs.push((root.clone(), name_in_root));
            }
        }
        graphs.sort();
        let total = graphs.len();
        let scanned = total.min(ENVIRONMENT_REFERENCE_LIMIT);
        graphs.truncate(ENVIRONMENT_REFERENCE_LIMIT);

        let mut nodes = Vec::new();
        let mut servers = Vec::new();
        for (root, graph) in &graphs {
            let path = root.join(graph).join("graph.json");
            let Some(definition) = std::fs::read(&path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            else {
                missing.push(graph.clone());
                continue;
            };
            // Which Plugins this Graph uses, and what they declare, comes from the
            // Plugin manifests themselves.
            let mut declared: Vec<(String, String, String)> = Vec::new();
            let plugin_ids = self.graph_plugin_ids(root, graph);
            for plugin in plugin_ids {
                let manifest = library_root
                    .join("plugins")
                    .join(&plugin)
                    .join("plugin.json");
                let Ok(bytes) = std::fs::read(&manifest) else {
                    continue;
                };
                let Ok(manifest) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
                    continue;
                };
                let Some(entries) = manifest
                    .get("mcpServers")
                    .and_then(|value| value.as_object())
                else {
                    continue;
                };
                for (server, config) in entries {
                    let environment = config
                        .get("anchor")
                        .and_then(|anchor| anchor.get("environment"))
                        .and_then(serde_json::Value::as_str);
                    if environment == Some(name) {
                        declared.push((plugin.clone(), server.clone(), name.to_owned()));
                    }
                }
            }
            // Nodes live at the top level and inside inline module graphs.
            let mut bodies = vec![&definition];
            if let Some(modules) = definition.get("graphs").and_then(|value| value.as_object()) {
                bodies.extend(modules.values());
            }
            for body in bodies {
                let Some(node_list) = body.get("nodes").and_then(|value| value.as_array()) else {
                    continue;
                };
                // Admission only lets an agent node or an `Op.run` node declare an
                // environment; a `call`/`fanout`/`join`/`host` declaration is
                // refused, so the report must not count it either.
                let ops = body.get("ops").and_then(|value| value.as_object());
                for node in node_list {
                    let node_id = node
                        .get("id")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    let declares_ok = if node.get("agent").is_some() {
                        true
                    } else {
                        node.get("op")
                            .and_then(serde_json::Value::as_str)
                            .and_then(|op| ops.and_then(|ops| ops.get(op)))
                            .is_some_and(|spec| spec.get("run").is_some())
                    };
                    if declares_ok
                        && node.get("environment").and_then(serde_json::Value::as_str) == Some(name)
                    {
                        nodes.push(EnvironmentNodeReference {
                            graph: graph.clone(),
                            node: node_id.clone(),
                        });
                    }
                    let bound = node
                        .get("plugins")
                        .and_then(|value| value.as_array())
                        .map(|plugins| {
                            plugins
                                .iter()
                                .filter_map(serde_json::Value::as_str)
                                .map(str::to_owned)
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    for (plugin, server, _) in &declared {
                        if bound.iter().any(|bound| bound == plugin) {
                            servers.push(EnvironmentServerReference {
                                graph: graph.clone(),
                                node: node_id.clone(),
                                plugin: plugin.clone(),
                                server: server.clone(),
                            });
                        }
                    }
                }
            }
        }
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

    /// The Plugin ids one Graph's manifest declares.
    fn graph_plugin_ids(&self, root: &std::path::Path, graph: &str) -> Vec<String> {
        let manifest = root.join(graph).join("manifest.json");
        let Ok(bytes) = std::fs::read(&manifest) else {
            return Vec::new();
        };
        let Ok(manifest) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            return Vec::new();
        };
        manifest
            .get("plugins")
            .and_then(|value| value.as_array())
            .map(|plugins| {
                plugins
                    .iter()
                    .filter_map(|plugin| {
                        plugin
                            .get("id")
                            .and_then(serde_json::Value::as_str)
                            .or_else(|| plugin.as_str())
                            .map(str::to_owned)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;

    #[test]
    fn the_impact_scan_finds_node_and_plugin_server_references() {
        let root = tempfile::tempdir().unwrap();
        let application = application(root.path());
        // The bundle loader needs the run store the application owns.
        std::fs::create_dir_all(root.path().join("state/runs")).unwrap();
        std::fs::create_dir_all(root.path().join("library/plugins/demo")).unwrap();
        std::fs::write(
            root.path().join("library/plugins/demo/plugin.json"),
            r#"{"name":"Demo","mcpServers":{"probe":{"command":"bin/run","anchor":{"environment":"research"}}}}"#,
        )
        .unwrap();
        let graph = root.path().join("used");
        std::fs::create_dir_all(&graph).unwrap();
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
                    "digest": "a".repeat(64),
                    "resources": [],
                    "mcp_servers": []
                }]
            })
            .to_string(),
        )
        .unwrap();

        let library = root.path().join("library");
        let references = application
            .environment_references(&library, "research")
            .unwrap();
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
        let unused = application
            .environment_references(&library, "unused")
            .unwrap();
        assert!(unused.nodes.is_empty() && unused.servers.is_empty());
    }
}
