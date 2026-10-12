//! The plugins tools the assistant may call.
//!
//! Split out of the single dispatch in `pilot_tools`; the table there stays
//! the only description of what these tools accept.

use super::*;

impl PilotTools {
    pub(super) async fn plugin_candidate(
        &self,
        name: &str,
        arguments: &Value,
    ) -> Result<Value, ToolError> {
        self.require_active_turn()?;
        let context = self
            .context
            .as_ref()
            .ok_or_else(|| rejected("Pilot Turn context unavailable"))?;
        let directory = parameter(arguments, "directory")?;
        let candidate = workspace_directory(&self.task_workspace(), &directory)?;
        let id = arguments
            .get("id")
            .and_then(Value::as_str)
            .or_else(|| candidate.file_name().and_then(|name| name.to_str()))
            .filter(|id| identifier(id))
            .ok_or_else(|| rejected("id must be a Plugin identifier"))?;
        let invocation = match (arguments.get("server"), arguments.get("tool")) {
            (None, None) if arguments.get("arguments").is_none() => None,
            (Some(Value::String(server)), Some(Value::String(tool))) => Some((
                server.as_str(),
                tool.as_str(),
                arguments
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({})),
            )),
            _ => {
                return Err(rejected(
                    "pass server and tool together, with optional object arguments",
                ));
            }
        };
        let prepared = self
            .application
            .prepare_plugin_candidate(
                &self.library_root,
                &candidate,
                id,
                &self.preparation_root()?,
                &context.dependency_access(),
            )
            .await
            .map_err(|error| rejected(format!("Plugin preparation failed: {error:?}")))?;
        if name == "plugin_prepare" {
            let mut result = prepared.result(context.network);
            result["rust_development"] = context.rust_status();
            result["python_development"] = context.python_status();
            return Ok(result);
        }
        self.application
            .test_plugin_candidate(&prepared, context, invocation)
            .await
            .map_err(|error| rejected(format!("Plugin candidate test failed: {error:?}")))
    }
    pub(super) async fn plugin_list(&self, _arguments: &Value) -> Result<Value, ToolError> {
        let catalog = FilePluginCatalog::new(&self.library_root);
        let mut plugins = Vec::new();
        match std::fs::read_dir(self.library_root.join("plugins")) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry.map_err(|_| failed())?;
                    let id = entry.file_name().to_string_lossy().into_owned();
                    if id.starts_with('.') {
                        continue;
                    }
                    plugins.push(match catalog.definition(&id) {
                        Ok(definition) => public_plugin(&definition),
                        Err(_) => json!({"id":id,"available":false}),
                    });
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(failed()),
        }
        plugins.sort_by(|left, right| left["id"].as_str().cmp(&right["id"].as_str()));
        Ok(json!({"plugins":plugins}))
    }

    pub(super) async fn plugin_read(&self, arguments: &Value) -> Result<Value, ToolError> {
        let plugin = parameter(arguments, "plugin")?;
        let definition = FilePluginCatalog::new(&self.library_root)
            .definition(&plugin)
            .map_err(|_| failed())?;
        let mut result = public_plugin(&definition);
        let mut instructions = Vec::new();
        if definition.directory.join("instructions.md").is_file() {
            instructions.push(text_resource(&definition.directory, "instructions.md")?);
        } else {
            for skill in &definition.skills {
                instructions.push(text_resource(&definition.directory, skill)?);
            }
        }
        result["dependencies"] =
            serde_json::to_value(&definition.dependencies).map_err(|_| failed())?;
        let review = anchor_library::Library::new(&self.library_root)
            .review_directory(&plugin, &definition.directory)
            .map_err(|_| failed())?;
        result["resources"] = json!(review.resources);
        result["servers"] = json!(review.servers);
        if let Some(resource) = arguments.get("resource").and_then(Value::as_str) {
            if !review.resources.iter().any(|entry| entry == resource) {
                return Err(rejected("resource is not part of this Plugin"));
            }
            if resource != "plugin.json" && resource != ".mcp.json" {
                result["resource"] =
                    json!({"path":resource,"text":text_resource(&definition.directory, resource)?});
            } else {
                return Err(rejected(
                    "manifest resource reads are not exposed; use dependency and resource facts",
                ));
            }
        }
        result["instructions"] = Value::String(instructions.join("\n\n"));
        Ok(result)
    }

    pub(super) async fn plugin_review(&self, arguments: &Value) -> Result<Value, ToolError> {
        let directory = parameter(arguments, "directory")?;
        let candidate = workspace_directory(&self.task_workspace(), &directory)?;
        let id = match arguments.get("id").and_then(Value::as_str) {
            Some(id) if identifier(id) => id.to_owned(),
            Some(_) => return Err(rejected("id must be a Plugin identifier")),
            None => candidate
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| identifier(name))
                .ok_or_else(|| rejected("the directory name is not a usable Plugin id; pass `id`"))?
                .to_owned(),
        };
        let review = anchor_library::Library::new(&self.library_root)
            .review_directory(&id, &candidate)
            .map_err(|failure| match failure {
                // The assistant needs the catalog's reason to fix the
                // manifest; nothing secret is echoed back.
                anchor_library::InstallError::InvalidPluginReason(reason) => {
                    rejected(format!("Plugin candidate rejected: {reason}"))
                }
                anchor_library::InstallError::MissingManifest => {
                    rejected("Plugin candidate has no plugin.json manifest")
                }
                _ => rejected("Plugin candidate could not be reviewed"),
            })?;
        serde_json::to_value(&review).map_err(|_| rejected("Plugin review could not be encoded"))
    }
}
