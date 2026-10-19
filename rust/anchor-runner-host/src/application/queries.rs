use super::{ApplicationError, RunApplication, RunMetadata, metadata, storage};
use anchor_runtime::graph::{GraphRunRecord, RunStore};
use std::path::Path;

impl RunApplication {
    pub(crate) async fn active_runs(&self, graph: Option<&str>) -> Vec<String> {
        let graph_path =
            graph.and_then(|name| Self::graph_identity(&self.graph_bundle_path(name)).ok());
        let mut ids: Vec<String> = self
            .active
            .lock()
            .await
            .iter()
            .filter(|(_, active)| {
                graph_path
                    .as_ref()
                    .is_none_or(|path| &active.graph_path == path)
            })
            .map(|(id, _)| id.clone())
            .collect();
        if let Ok(Some(runs)) = self.kernel_runs(false) {
            for (id, _) in self.kernel_records().unwrap_or_default() {
                let matches_graph = graph_path.as_ref().is_none_or(|path| {
                    self.metadata(&id).ok().flatten().is_some_and(|m| {
                        Self::graph_identity(&m.bundle_source).ok().as_ref() == Some(path)
                    })
                });
                if runs.active(&id) && matches_graph {
                    ids.push(id);
                }
            }
        }
        ids.sort();
        ids
    }

    pub(crate) async fn run_is_active(&self, run_id: &str) -> bool {
        if let Ok(Some(runs)) = self.kernel_owner(run_id) {
            return runs.active(run_id);
        }
        self.active.lock().await.contains_key(run_id)
    }

    pub(crate) fn metadata(&self, run_id: &str) -> Result<Option<RunMetadata>, ApplicationError> {
        metadata::load(&self.data_root, run_id)
    }

    /// Legacy file modification time or the native Run's Host update observation.
    pub(crate) fn run_updated(&self, run_id: &str) -> Result<Option<String>, ApplicationError> {
        if run_id.is_empty()
            || run_id == "."
            || run_id == ".."
            || !run_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        {
            return Err(ApplicationError::Invalid("invalid Run id".into()));
        }
        if let Some(runs) = self.kernel_owner(run_id)? {
            return Ok(runs
                .view(&runs.load(run_id)?.ok_or(ApplicationError::Missing)?)?
                .updated);
        }
        let path = self.data_root.join("runs").join(format!("{run_id}.json"));
        let modified = match std::fs::metadata(path).and_then(|metadata| metadata.modified()) {
            Ok(modified) => modified,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(storage(error)),
        };
        Ok(Some(
            chrono::DateTime::<chrono::Utc>::from(modified).to_rfc3339(),
        ))
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

    pub(crate) fn records(&self) -> Result<Vec<(String, GraphRunRecord)>, ApplicationError> {
        let root = self.data_root.join("runs");
        let mut records = self.kernel_records()?;
        if !root.exists() {
            return Ok(records);
        }
        for entry in std::fs::read_dir(root).map_err(storage)? {
            let entry = entry.map_err(storage)?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(id) = name.strip_suffix(".json")
                && let Some(record) = self.store().load(id)?
            {
                records.push((id.to_owned(), record));
            }
        }
        records.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(records)
    }

    /// Runs whose immutable Graph identity resolves to this exact bundle path.
    /// Runs without metadata cannot be attributed and are left untouched.
    pub(crate) fn runs_for_graph(
        &self,
        graph_path: &Path,
    ) -> Result<Vec<(String, GraphRunRecord)>, ApplicationError> {
        let mut runs = Vec::new();
        for (id, record) in self.records()? {
            let Some(metadata) = self.metadata(&id)? else {
                continue;
            };
            if Self::graph_identity(&metadata.bundle_source)? == graph_path {
                runs.push((id, record));
            }
        }
        Ok(runs)
    }
}
