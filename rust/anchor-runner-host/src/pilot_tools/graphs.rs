//! The graphs tools the assistant may call.
//!
//! Split out of the single dispatch in `pilot_tools`; the table there stays
//! the only description of what these tools accept.

use super::*;

impl PilotTools {
    pub(super) async fn graph_create(&self, arguments: &Value) -> Result<Value, ToolError> {
        let graph = parameter(arguments, "name")?;
        self.refuse_self_graph(&graph)?;
        self.require_active_turn()?;
        self.application
            .create_graph(&graph, arguments.get("definition").cloned())
            .await
            .map_err(|failure| ToolError::Failed(format!("Graph creation rejected: {failure:?}")))
    }

    pub(super) async fn graph_update(&self, arguments: &Value) -> Result<Value, ToolError> {
        let graph = parameter(arguments, "graph")?;
        // A self-target is refused before anything else: the Graph that
        // defines the assistant is not something the assistant may
        // rewrite, regardless of Turn state.
        self.refuse_self_graph(&graph)?;
        self.require_active_turn()?;
        let definition = arguments
            .get("definition")
            .filter(|value| value.is_object())
            .cloned()
            .ok_or_else(|| ToolError::Failed("definition must be an object".into()))?;
        self.application
            .update_graph(&graph, definition)
            .await
            .map_err(|failure| ToolError::Failed(format!("Graph update rejected: {failure:?}")))
    }

    pub(super) async fn graph_run(&self, arguments: &Value) -> Result<Value, ToolError> {
        self.require_active_turn()?;
        let graph = parameter(arguments, "graph")?;
        let objective = match arguments.get("objective") {
            None | Some(Value::Null) => None,
            Some(Value::String(value)) => Some(value.clone()),
            _ => return Err(ToolError::Failed("objective must be a string".into())),
        };
        let path = self
            .application
            .graph_path_checked(&graph)
            .map_err(|failure| ToolError::Failed(format!("Graph unavailable: {failure:?}")))?;
        let lease = self
            .application
            .graph_admission_lease(&path)
            .map_err(|failure| ToolError::Failed(format!("Run admission rejected: {failure:?}")))?;
        let (path, bundle) = self
            .application
            .load_graph(&graph)
            .map_err(|failure| ToolError::Failed(format!("Graph unavailable: {failure:?}")))?;
        let run = self
            .application
            .admit(
                graph.clone(),
                &path,
                bundle,
                arguments.get("input").cloned().unwrap_or_else(|| json!({})),
                AdmissionOptions {
                    objective,
                    trigger: RunTrigger::default(),
                    pilot: Some(PilotRunSource {
                        owner: self.owner.clone(),
                        session: self.session.clone(),
                        turn: self.turn.clone(),
                    }),
                    oauth_owner: Some(crate::api::oauth::binding_owner(&self.owner)),
                },
                lease,
            )
            .await
            .map_err(|failure| ToolError::Failed(format!("Run admission rejected: {failure:?}")))?;
        let mut result = json!({"run":run,"graph":graph,"session":self.session,"accepted":true});
        if self
            .sessions
            .associate_run(&self.owner, &self.session, &self.turn, &run)
            .is_err()
        {
            result["association_error"] = json!(
                "Run accepted; Session association pending reconciliation. Do not retry graph_run; inspect this Run ID."
            );
        }
        Ok(result)
    }

    pub(super) async fn graph_list(&self, _arguments: &Value) -> Result<Value, ToolError> {
        let mut names = vec![self.graph_name.clone()];
        if let Ok(entries) = std::fs::read_dir(&self.catalog_root) {
            for entry in entries {
                let entry = entry.map_err(|_| failed())?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if identifier(&name)
                    && !names.contains(&name)
                    && entry.file_type().is_ok_and(|kind| kind.is_dir())
                    && entry.path().join("graph.json").is_file()
                {
                    names.push(name);
                }
            }
        }
        names.sort();
        let mut graphs = Vec::new();
        for graph in names {
            let active = self.application.active_runs(Some(&graph)).await;
            graphs.push(json!({"graph":graph,"active_runs":active}));
        }
        Ok(json!({"graphs":graphs}))
    }

    pub(super) async fn graph_read(&self, arguments: &Value) -> Result<Value, ToolError> {
        let graph = parameter(arguments, "graph")?;
        if !identifier(&graph) {
            return Err(failed());
        }
        let definition = text_resource(&self.application.graph_bundle_path(&graph), "graph.json")?;
        Ok(
            json!({"graph":graph,"definition":serde_json::from_str::<Value>(&definition).map_err(|_| failed())?}),
        )
    }

    pub(super) async fn graph_validate(&self, arguments: &Value) -> Result<Value, ToolError> {
        let definition = arguments
            .get("definition")
            .filter(|value| value.is_object())
            .ok_or_else(failed)?;
        let validation = (|| {
            let snapshot =
                GraphSnapshot::from_authoring(definition.clone()).map_err(|_| failed())?;
            crate::reject_snapshot(&snapshot).map_err(|_| failed())?;
            let ids = snapshot
                .nodes
                .iter()
                .flat_map(|node| node.plugins.iter().cloned())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            FilePluginCatalog::new(&self.library_root)
                .resolve(&ids)
                .map_err(|_| failed())?;
            Ok::<_, ToolError>(snapshot)
        })();
        match validation {
            Ok(snapshot) => Ok(
                json!({"valid":true,"entry":snapshot.entry,"nodes":snapshot.nodes.iter().map(|node| &node.id).collect::<Vec<_>>()}),
            ),
            Err(_) => Ok(json!({"valid":false,"error":"invalid Graph or unavailable Plugin"})),
        }
    }
}
