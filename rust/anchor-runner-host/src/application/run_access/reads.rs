use super::RunView;
use crate::application::{ApplicationError, RunApplication, storage};
use anchor_runtime::graph::{FileRunStore, GraphError, GraphRunRecord, RunStore};
use std::{borrow::Cow, path::Path};

pub(crate) fn load(data_root: &Path, run_id: &str) -> Result<Option<RunView<'static>>, GraphError> {
    Ok(FileRunStore::new(data_root.join("runs"))
        .load(run_id)?
        .map(|record| RunView {
            record: Cow::Owned(record),
            active: false,
            requested: None,
            session_coordinated: false,
        }))
}

impl RunApplication {
    pub(in crate::application) async fn resume_interrupted_attempts(
        &self,
        run_id: &str,
    ) -> Result<bool, ApplicationError> {
        let Some(record) = self.store().load(run_id)? else {
            return Ok(false);
        };
        if !RunView::inactive(&record).has_pending_recovery() {
            return Ok(false);
        }
        for pending in record.recovery {
            let observation = format!(
                "The previous `{}` tool call (step {}) was interrupted before its result was recorded. Its external effect is unknown. Inspect the workspace, artifacts, and any available external state before deciding what to do; do not blindly repeat the call.",
                pending.attempt.tool, pending.attempt.step
            );
            self.recover(
                run_id,
                &pending.key.node_id,
                pending.key.invocation,
                pending.attempt.attempt_id,
                anchor_runtime::graph::RecoveryDecision::Completed { observation },
            )
            .await?;
        }
        Ok(true)
    }
    pub(crate) fn run_updated(&self, run_id: &str) -> Result<String, ApplicationError> {
        if run_id.is_empty()
            || run_id == "."
            || run_id == ".."
            || !run_id
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "-_.".contains(character))
        {
            return Err(ApplicationError::Invalid("invalid Run id".into()));
        }
        let modified =
            std::fs::metadata(self.data_root.join("runs").join(format!("{run_id}.json")))
                .and_then(|metadata| metadata.modified())
                .map_err(storage)?;
        Ok(chrono::DateTime::<chrono::Utc>::from(modified).to_rfc3339())
    }

    pub(in crate::application::run_access) fn raw_records(
        &self,
    ) -> Result<Vec<(String, GraphRunRecord)>, ApplicationError> {
        let root = self.data_root.join("runs");
        if !root.exists() {
            return Ok(Vec::new());
        }
        let mut records = Vec::new();
        for entry in std::fs::read_dir(root).map_err(storage)? {
            let entry = entry.map_err(storage)?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(id) = name.strip_suffix(".json")
                && let Some(record) = self.store().load(id)?
            {
                records.push((id.to_owned(), record));
            }
        }
        records.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(records)
    }

    pub(in crate::application::run_access) fn raw_runs_for_graph(
        &self,
        graph_path: &Path,
    ) -> Result<Vec<(String, GraphRunRecord)>, ApplicationError> {
        let mut runs = Vec::new();
        for (id, record) in self.raw_records()? {
            let Some(metadata) = self.metadata(&id)? else {
                continue;
            };
            if Self::graph_identity(&metadata.bundle_source)? == graph_path {
                runs.push((id, record));
            }
        }
        Ok(runs)
    }
    pub(crate) fn read_run(
        &self,
        run_id: &str,
    ) -> Result<Option<RunView<'static>>, ApplicationError> {
        load(&self.data_root, run_id).map_err(Into::into)
    }

    pub(crate) fn run_views(&self) -> Result<Vec<(String, RunView<'static>)>, ApplicationError> {
        Ok(self
            .raw_records()?
            .into_iter()
            .map(|(run_id, record)| {
                (
                    run_id,
                    RunView {
                        record: Cow::Owned(record),
                        active: false,
                        requested: None,
                        session_coordinated: false,
                    },
                )
            })
            .collect())
    }

    pub(crate) fn runs_for_graph(
        &self,
        path: &Path,
    ) -> Result<Vec<(String, RunView<'static>)>, ApplicationError> {
        Ok(self
            .raw_runs_for_graph(path)?
            .into_iter()
            .map(|(id, record)| {
                (
                    id,
                    RunView {
                        record: Cow::Owned(record),
                        active: false,
                        requested: None,
                        session_coordinated: false,
                    },
                )
            })
            .collect())
    }

    #[cfg(test)]
    pub(crate) fn records(&self) -> Result<Vec<(String, GraphRunRecord)>, ApplicationError> {
        self.raw_records()
    }
}

impl RunView<'_> {
    pub(crate) fn visible_active_nodes(&self) -> std::collections::BTreeSet<String> {
        let mut active_nodes = std::collections::BTreeSet::new();
        if !self.active || self.record.status != anchor_runtime::graph::RunStatus::Running {
            return active_nodes;
        }
        if let Some(cursor) = &self.record.cursor {
            let waiting = self
                .record
                .snapshot
                .nodes
                .iter()
                .find(|node| node.id == cursor.node_id)
                .and_then(|node| node.op.as_ref())
                .and_then(|name| self.record.snapshot.ops.get(name))
                .and_then(|operation| operation.get("host"))
                .and_then(|operation| operation.get("operation"))
                .and_then(serde_json::Value::as_str)
                == Some(crate::assistant::WAIT_INPUT);
            if !waiting {
                active_nodes.insert(cursor.node_id.clone());
            }
        }
        if let Some(parallel) = &self.record.parallel {
            for branch in &parallel.branches {
                if branch.status == anchor_runtime::graph::ParallelBranchStatus::Running
                    && let Some(cursor) = &branch.cursor
                {
                    active_nodes.insert(cursor.node_id.clone());
                }
            }
        }
        active_nodes
    }
}
