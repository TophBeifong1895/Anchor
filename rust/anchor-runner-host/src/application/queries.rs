use super::{ApplicationError, RunApplication, RunMetadata, metadata, storage};

impl RunApplication {
    pub(crate) async fn active_runs(&self, graph: Option<&str>) -> Vec<String> {
        let graph_path =
            graph.and_then(|name| Self::graph_identity(&self.graph_bundle_path(name)).ok());
        self.active
            .lock()
            .await
            .iter()
            .filter(|(_, active)| {
                graph_path
                    .as_ref()
                    .is_none_or(|path| &active.graph_path == path)
            })
            .map(|(id, _)| id.clone())
            .collect()
    }

    pub(crate) async fn run_is_active(&self, run_id: &str) -> bool {
        self.active.lock().await.contains_key(run_id)
    }

    pub(crate) fn metadata(&self, run_id: &str) -> Result<Option<RunMetadata>, ApplicationError> {
        metadata::load(&self.data_root, run_id)
    }

    pub(crate) fn child_metadata(
        &self,
        parent_run: &str,
    ) -> Result<Vec<RunMetadata>, ApplicationError> {
        let root = self.data_root.join("run-metadata");
        if !root.exists() {
            return Ok(Vec::new());
        }
        let mut children = Vec::new();
        for entry in std::fs::read_dir(root).map_err(storage)? {
            let entry = entry.map_err(storage)?;
            let Some(run_id) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.strip_suffix(".json"))
                .map(str::to_owned)
            else {
                continue;
            };
            if let Some(metadata) = self.metadata(&run_id)?
                && metadata
                    .graph_call
                    .as_ref()
                    .is_some_and(|source| source.parent_run == parent_run)
            {
                children.push(metadata);
            }
        }
        children.sort_by(|a, b| a.run_id.cmp(&b.run_id));
        Ok(children)
    }
}
