//! The plugins tools the assistant may call.
//!
//! Split out of the single dispatch in `pilot_tools`; the table there stays
//! the only description of what these tools accept.

use super::*;

impl PilotTools {
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
        result["instructions"] = Value::String(instructions.join("\n\n"));
        Ok(result)
    }

    pub(super) async fn plugin_review(&self, arguments: &Value) -> Result<Value, ToolError> {
        let directory = parameter(arguments, "directory")?;
        let candidate = workspace_directory(&self.workspace_root, &directory)?;
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
