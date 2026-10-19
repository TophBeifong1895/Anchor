use crate::application::{
    ActiveRun, AdmissionOptions, ApplicationError, RunApplication, RunMetadata, metadata,
    new_control, storage,
};
use crate::{create_durable_directory, execution::PreparedExecution};
use anchor_graph_host::LoadedGraphBundle;
use anchor_runtime::graph::{GraphRunRecord, RunLease, RunStore};
use serde_json::Value;
use std::path::Path;

impl RunApplication {
    pub(crate) async fn admit(
        &self,
        graph: String,
        source: &Path,
        bundle: LoadedGraphBundle,
        input: Value,
        options: AdmissionOptions,
        graph_lease: Box<dyn RunLease>,
    ) -> Result<String, ApplicationError> {
        self.admit_selected(graph, source, bundle, input, options, graph_lease, false)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn admit_selected(
        &self,
        graph: String,
        source: &Path,
        mut bundle: LoadedGraphBundle,
        input: Value,
        options: AdmissionOptions,
        graph_lease: Box<dyn RunLease>,
        kernel_probe: bool,
    ) -> Result<String, ApplicationError> {
        options
            .trigger
            .validate()
            .map_err(ApplicationError::Invalid)?;
        if let Some(objective) = options.objective.filter(|value| !value.is_empty()) {
            bundle.snapshot.objective = objective;
        }
        let graph_path = Self::graph_identity(source)?;
        let plugin_lease = if bundle.plugins.is_empty() {
            None
        } else {
            Some(self.plugin_reference_lease().await?)
        };
        let mut active = self.active.lock().await;
        if active.values().any(|run| run.graph_path == graph_path) {
            return Err(ApplicationError::Conflict(
                "this graph is already running".into(),
            ));
        }
        // A service restart does not silently launch another Run over an orphan.
        for (id, record) in self.run_views()? {
            if !record.is_settled() {
                match self.metadata(&id)? {
                    Some(meta)
                        if Self::graph_identity(&meta.bundle_source)? == graph_path =>
                    {
                        return Err(ApplicationError::Conflict(
                            "this graph has an unfinished Run; resume it, or stop and delete its record".into(),
                        ));
                    }
                    None => return Err(ApplicationError::Conflict(
                        "an unfinished Run has no immutable Graph identity; admission is quarantined".into(),
                    )),
                    Some(_) => {}
                }
            }
        }
        let run_id = format!("rust-{:x}", metadata::now_nanos());
        let plugin_bindings = self.bundle_plugin_bindings(&bundle)?;
        let mut record = GraphRunRecord::create_with_id(bundle.snapshot, input, run_id.clone())?;
        record.plugin_bindings = plugin_bindings;
        record.plugin_bindings_initialized = true;
        if kernel_probe {
            anchor_runtime::graph::scheduler::validate_probe(&record)?;
        }
        // The admission observation and the durable reference are published
        // under one Library lease. Node dispatch resolves the current Plugin
        // again so an update can take effect at the next invocation.
        let control = new_control();
        let execution = PreparedExecution::prepare_with_catalog(
            &record,
            control.clone(),
            record.plugin_bindings.clone(),
            self.catalog_root.clone(),
            self.clone(),
        )
        .map_err(ApplicationError::Invalid)?;
        execution
            .bind_local_inputs(&record, &graph)
            .map_err(ApplicationError::Invalid)?;
        self.check_store()?;
        let mut metadata = RunMetadata::new(
            run_id.clone(),
            graph.clone(),
            record.graph_digest.clone(),
            source,
        )?;
        options.trigger.apply(&mut metadata);
        metadata.pilot = options.pilot;
        metadata.oauth_owner = options.oauth_owner;
        if kernel_probe {
            let runs = self.kernel_runs(true)?.expect("created Kernel probe store");
            metadata::save(&self.data_root, &metadata)?;
            runs.create(&record, u64::MAX)?;
            drop(active);
            drop(plugin_lease);
            drop(graph_lease);
            return self
                .spawn_kernel(record, execution, control)
                .map(|()| run_id);
        }
        create_durable_directory(&self.data_root.join("runs")).map_err(storage)?;
        let lease = self.store().acquire_lease(&run_id)?;
        // Metadata is saved first: a crash can leave unused metadata, never an accepted
        // Run without its immutable Graph identity. The response follows both saves.
        metadata::save(&self.data_root, &metadata)?;
        record.touch();
        self.store().save(&record)?;
        active.insert(run_id.clone(), ActiveRun::new(graph_path, control));
        drop(lease);
        drop(plugin_lease);
        // The durable snapshot and immutable identity now serialize future
        // admissions. The Graph lease only protects reading/writing the bundle.
        drop(graph_lease);
        self.spawn(record, execution)?;
        Ok(run_id)
    }
}
