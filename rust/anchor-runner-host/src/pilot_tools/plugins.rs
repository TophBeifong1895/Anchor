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
    pub(super) async fn plugin_list(&self, arguments: &Value) -> Result<Value, ToolError> {
        let _guard = anchor_library::Library::new(&self.library_root)
            .catalog_read_guard()
            .map_err(|_| rejected("Plugin Library is changing; retry this read"))?;
        let catalog = FilePluginCatalog::new(&self.library_root);
        let mut disclosure = crate::plugin_disclosure::PluginDisclosureCatalog::default();
        if _guard.is_some() {
            match std::fs::read_dir(self.library_root.join("plugins")) {
                Ok(entries) => {
                    for entry in entries {
                        let entry = entry.map_err(|_| failed())?;
                        let id = entry.file_name().to_string_lossy().into_owned();
                        if id.starts_with('.') {
                            continue;
                        }
                        match catalog.definition(&id) {
                            Ok(definition) => disclosure.insert(definition),
                            Err(_) => disclosure.unavailable(&id),
                        }
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(failed()),
            }
        }
        let mut result = disclosure.list(arguments)?;
        result["scope"] = json!("installed_library");
        result["note"] = json!(
            "Installed Library management only; listing does not mount or authorize execution. Read a selected installed Plugin using plugin_read. Execution capabilities are listed separately by anchor_plugins."
        );
        Ok(result)
    }

    pub(super) async fn plugin_read(&self, arguments: &Value) -> Result<Value, ToolError> {
        let _guard = anchor_library::Library::new(&self.library_root)
            .catalog_read_guard()
            .map_err(|_| rejected("Plugin Library is changing; retry this read"))?;
        if _guard.is_none() {
            return Err(rejected("Plugin Library unavailable"));
        }
        let plugin = parameter(arguments, "plugin")?;
        let definition = FilePluginCatalog::new(&self.library_root)
            .definition(&plugin)
            .map_err(|_| failed())?;
        let mut disclosure = crate::plugin_disclosure::PluginDisclosureCatalog::default();
        disclosure.insert(definition);
        let mut selection = arguments.clone();
        if let Some(resource) = arguments.get("resource").and_then(Value::as_str) {
            if arguments.get("section").is_some() || arguments.get("path").is_some() {
                return Err(rejected("use either resource or section/path"));
            }
            selection["section"] = json!("resource");
            selection["path"] = json!(resource);
        }
        let mut result = disclosure.read(&selection)?;
        result["scope"] = json!("installed_library");
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
