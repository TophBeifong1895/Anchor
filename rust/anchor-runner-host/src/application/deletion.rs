use super::{ApplicationError, RunApplication, graphs, metadata, storage};
use crate::run_data;
use anchor_runtime::graph::{GraphSnapshot, RunLease, RunStatus, RunStore};
use serde_json::Value;
use std::{collections::BTreeSet, path::Path, time::Duration};

impl RunApplication {
    /// Acquire the shared Graph lease, retrying briefly so an in-flight child
    /// admission is not surfaced as a spurious delete conflict.
    pub(crate) async fn acquire_graph_lease_waiting(
        &self,
        graph_path: &Path,
    ) -> Result<Box<dyn RunLease>, ApplicationError> {
        for attempt in 0..100 {
            match self.graph_lease(graph_path) {
                Ok(lease) => return Ok(lease),
                Err(ApplicationError::Conflict(_)) if attempt < 99 => {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Err(error) => return Err(error),
            }
        }
        Err(ApplicationError::Conflict(
            "Graph is busy with an admission; retry the operation".into(),
        ))
    }

    fn snapshot_calls_graph(
        &self,
        snapshot: &GraphSnapshot,
        target_path: &Path,
    ) -> Result<bool, ApplicationError> {
        for node in &snapshot.nodes {
            let Some(op_name) = &node.op else { continue };
            let Some(graph) = snapshot.ops[op_name]
                .get("call")
                .and_then(|call| call.get("graph"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            if Self::graph_identity(&self.graph_bundle_path(graph))? == target_path {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Every Graph that calls `target_path`, or a refusal when the catalog could
    /// not be read completely.
    ///
    /// Deletion protection may not read an unreadable Graph as "nobody calls
    /// this": the candidate set is the one impact scan, which names every Graph
    /// it could not load, and an incomplete scan refuses the deletion instead of
    /// allowing it.
    fn graph_callers(&self, target_path: &Path) -> Result<Vec<String>, ApplicationError> {
        let mut callers = BTreeSet::new();
        let (bundles, considered, total, missing) = match self.impact_scan() {
            graphs::ImpactScan::Read {
                bundles,
                considered,
                total,
                missing,
            } => (bundles, considered, total, missing),
            graphs::ImpactScan::Unreadable(reason) => {
                return Err(ApplicationError::Conflict(format!(
                    "cannot delete this Graph: the Graph catalog could not be read to find out who calls it ({reason})"
                )));
            }
        };
        for (name, bundle) in &bundles {
            if self.snapshot_calls_graph(&bundle.snapshot, target_path)? {
                callers.insert(format!("Graph `{name}`"));
            }
        }
        for (run_id, record) in self.records()? {
            if !record.status.is_terminal()
                && self.snapshot_calls_graph(&record.snapshot, target_path)?
            {
                let graph = self
                    .metadata(&run_id)?
                    .map(|metadata| metadata.graph)
                    .unwrap_or_else(|| "unknown Graph".into());
                callers.insert(format!("unfinished Run `{run_id}` of `{graph}`"));
            }
        }
        // A named caller already refuses the deletion. With no caller found, an
        // unreadable Graph is the difference between "nobody calls this" and "we
        // could not tell", and only the first may delete.
        if callers.is_empty() {
            match graphs::coverage_of(considered, total, missing) {
                graphs::ImpactCoverage::Complete => {}
                graphs::ImpactCoverage::Partial {
                    considered,
                    total,
                    missing,
                } => {
                    return Err(ApplicationError::Conflict(format!(
                        "cannot delete this Graph: considered {considered} of {total} Graphs and could not read {}, which may call it",
                        missing
                            .iter()
                            .map(|gap| format!("`{}` ({})", gap.graph, gap.reason))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )));
                }
                graphs::ImpactCoverage::Unknown { reason } => {
                    return Err(ApplicationError::Conflict(format!(
                        "cannot delete this Graph: who calls it could not be determined ({reason})"
                    )));
                }
            }
        }
        Ok(callers.into_iter().collect())
    }

    /// Delete an unreferenced Graph and its own terminal Run data.
    /// Current Graph definitions and unfinished Run snapshots keep their call
    /// targets alive; completed history does not prevent deletion.
    pub(crate) async fn delete_graph(
        &self,
        bundle_path: &Path,
        workspace_root: &Path,
    ) -> Result<usize, ApplicationError> {
        let graph_path = Self::graph_identity(bundle_path)?;
        let graph_lease = self.acquire_graph_lease_waiting(&graph_path).await?;
        self.delete_graph_with_lease(bundle_path, workspace_root, graph_lease)
            .await
    }

    pub(crate) async fn delete_graph_with_lease(
        &self,
        bundle_path: &Path,
        workspace_root: &Path,
        _graph_lease: Box<dyn RunLease>,
    ) -> Result<usize, ApplicationError> {
        let graph_path = Self::graph_identity(bundle_path)?;
        let callers = self.graph_callers(&graph_path)?;
        if !callers.is_empty() {
            return Err(ApplicationError::Conflict(format!(
                "cannot delete this Graph while it is called by {}",
                callers.join(", ")
            )));
        }
        let active = self.active.lock().await;
        let targets = self.runs_for_graph(&graph_path)?;
        for (id, _) in &targets {
            if self.kernel_owner(id)?.is_some() {
                return Err(ApplicationError::Conflict(
                    "P probe: deleting a Graph with Kernel Runs is Unsupported".into(),
                ));
            }
        }
        // A durable hand-off may have metadata before its Run is admitted. It
        // still owns history and cannot be erased by an empty cascade.
        for id in metadata::ids(&self.data_root)? {
            if let Some(retained) = self.metadata(&id)?
                && Self::graph_identity(&retained.bundle_source)? == graph_path
                && !targets.iter().any(|(run, _)| run == &id)
            {
                return Err(ApplicationError::Conflict(format!(
                    "Graph has retained hand-off `{id}` without its Run; settle it before deletion"
                )));
            }
        }
        for (run_id, record) in &targets {
            self.reject_pending_session_delivery(run_id)?;
            if active.contains_key(run_id) {
                return Err(ApplicationError::Conflict(
                    "Graph still has active execution".into(),
                ));
            }
            if !record.status.is_terminal() {
                let conversation = self
                    .metadata(run_id)?
                    .is_some_and(|metadata| metadata.conversation.is_some());
                if !conversation
                    || (record.status != RunStatus::Stopped
                        && !self.has_conversation_successor(run_id)?)
                {
                    return Err(ApplicationError::Conflict(format!(
                        "Graph has unfinished Run `{run_id}`; resume it, or stop and delete its record before deleting the Graph"
                    )));
                }
            }
            if self
                .metadata(run_id)?
                .is_some_and(|metadata| metadata.conversation.is_some())
            {
                self.ensure_conversation_delete_settled(record, &active)?;
            }
        }
        for (run_id, _) in &targets {
            self.retire_assistant_for_deletion(run_id)?;
        }
        let mut conversations = BTreeSet::new();
        for (run_id, record) in &targets {
            if let Some(metadata) = self.metadata(run_id)? {
                crate::conversation_resources::retain(&self.data_root, record, &metadata)?;
                for node in &record.snapshot.nodes {
                    if let Some(hint) = crate::node_host::conversation_hint_for(&metadata, &node.id)
                    {
                        conversations.insert(hint.key);
                    }
                }
            }
        }
        conversations.extend(crate::conversation_resources::for_graph(
            &self.data_root,
            &graph_path,
        )?);
        for hint in conversations {
            crate::goose_acp::remove_conversation(&workspace_root.join(".goose-process"), &hint)
                .map_err(ApplicationError::Storage)?;
            run_data::remove_legacy_conversation(&self.data_root, &hint)
                .map_err(ApplicationError::Storage)?;
        }
        for (id, record) in &targets {
            // Cascade deletion removes whole lineages, but a Host that dies
            // halfway through must still leave every removed link explained.
            if let Some(metadata) = self.metadata(id)?
                && let Some(source) = metadata.conversation.as_ref()
            {
                crate::run_deletions::save(
                    &self.data_root,
                    &crate::run_deletions::RunDeletion::new(
                        id,
                        &Self::graph_identity(&metadata.bundle_source)?.to_string_lossy(),
                        &source.session,
                        &source.reply_node,
                        source.previous_run.as_deref(),
                    ),
                )
                .map_err(ApplicationError::Storage)?;
            }
            run_data::delete_run_data(record, &self.data_root, workspace_root)
                .map_err(ApplicationError::Storage)?;
            run_data::remove_run_files(&self.data_root, id).map_err(ApplicationError::Storage)?;
        }

        // Remove the bundle itself while still holding the Graph lease, so no
        // admission can slip in between the Run cleanup and the directory drop.
        crate::assistant_plugins::cleanup_publications(&self.data_root, &graph_path)
            .map_err(ApplicationError::Storage)?;
        if bundle_path.is_dir() {
            std::fs::remove_dir_all(bundle_path).map_err(storage)?;
        }
        Ok(targets.len())
    }

    pub(crate) async fn delete_run(
        &self,
        run_id: &str,
        workspace_root: &Path,
    ) -> Result<(), ApplicationError> {
        if self.kernel_owner(run_id)?.is_some() {
            return Err(ApplicationError::Conflict(
                "P probe: Kernel Run deletion is Unsupported".into(),
            ));
        }
        // Match admission's lock order. A saved hand-off may precede its Run,
        // so the Graph lease must cover reference inspection and cleanup.
        let _graph_lease = match self.metadata(run_id)? {
            Some(metadata) if metadata.conversation.is_some() => Some(
                self.acquire_graph_lease_waiting(&metadata.bundle_source)
                    .await?,
            ),
            _ => None,
        };
        let active = self.active.lock().await;
        if active.contains_key(run_id) {
            return Err(ApplicationError::Conflict(
                "that Run is still running".into(),
            ));
        }
        let store = self.store();
        let record = store.load(run_id)?.ok_or(ApplicationError::Missing)?;
        let metadata = self.metadata(run_id)?;
        self.reject_pending_session_delivery(run_id)?;
        if let Some(metadata) = metadata.as_ref() {
            self.ensure_channel_delivery_settled(metadata)?;
        }
        match metadata
            .as_ref()
            .filter(|metadata| metadata.conversation.is_some())
        {
            // A conversation Run may only go once every later Run has stopped
            // reading it; see `ensure_conversation_delete_allowed`.
            Some(metadata) => {
                self.ensure_conversation_delete_allowed(&record, metadata, &active)?;
            }
            None => self.reject_conversation_successor(run_id)?,
        }
        if !record.graph_calls.is_empty() {
            return Err(ApplicationError::Conflict(
                "Run is referenced by or owns a Graph call".into(),
            ));
        }
        for (other_id, other) in self.records()? {
            if other_id != run_id
                && other
                    .graph_calls
                    .values()
                    .any(|call| call.child_run_id.as_deref() == Some(run_id))
            {
                return Err(ApplicationError::Conflict(
                    "Run is referenced by another Graph call".into(),
                ));
            }
        }
        let _lease = store.acquire_lease(run_id)?;
        let artifacts = crate::HostArtifacts::new(
            self.data_root.join("artifacts"),
            workspace_root.to_path_buf(),
        );
        for result in record.results.values().flatten() {
            if self
                .data_root
                .join("artifacts")
                .join(&result.commit.id)
                .exists()
            {
                artifacts
                    .list_files(&result.commit)
                    .map_err(|error| ApplicationError::Invalid(error.to_string()))?;
            }
            if result.key.run_id != run_id
                || result.key.graph_digest != record.graph_digest
                || result.commit.node_id != result.key.node_id
                || result.commit.invocation != result.key.invocation
            {
                return Err(ApplicationError::Invalid(
                    "Run artifact identity mismatch".into(),
                ));
            }
        }
        // Durable before anything is removed: a Host that dies in the middle of
        // this cleanup leaves an explained lineage gap, not corruption.
        if let Some(metadata) = metadata.as_ref()
            && let Some(source) = metadata.conversation.as_ref()
        {
            crate::conversation_resources::retain(&self.data_root, &record, metadata)?;
            crate::run_deletions::save(
                &self.data_root,
                &crate::run_deletions::RunDeletion::new(
                    run_id,
                    &Self::graph_identity(&metadata.bundle_source)?.to_string_lossy(),
                    &source.session,
                    &source.reply_node,
                    source.previous_run.as_deref(),
                ),
            )
            .map_err(ApplicationError::Storage)?;
            crate::conversation_resources::release_unreferenced(
                self,
                &self.data_root,
                &Self::graph_identity(&metadata.bundle_source)?,
                run_id,
                workspace_root,
            )?;
        }
        run_data::delete_run_data(&record, &self.data_root, workspace_root)
            .map_err(ApplicationError::Storage)?;
        run_data::remove_run_files(&self.data_root, run_id).map_err(ApplicationError::Storage)
    }
}
