//! Explicit serial probe. FileStore is the sole authority for these Runs;
//! legacy status values are read-only projections for existing consumers.
use super::{ApplicationError, RunApplication};
use anchor_kernel::{
    Action, Authority, FileStore, Kernel, Phase, Principal, PrincipalKind, Resource, Store,
    Submission,
};
use anchor_runtime::graph::{ArtifactPort, GraphError, GraphRunRecord, NodeExecutionPort};
use anchor_runtime::graph::{
    scheduler::{
        EXECUTOR_ID, HOST_ID, KernelScheduler, OPERATOR_ID, kernel_error, submit_with_retry,
        validate_probe,
    },
    validator::{GRAPH_VALIDATOR, GraphValidator, record_of, run_resource, run_resource_id},
};
use std::{
    collections::HashMap,
    fs::OpenOptions,
    path::Path,
    sync::{Arc, Mutex, atomic::Ordering},
};

struct ProbeAuthority;
impl Authority for ProbeAuthority {
    fn permits(&self, p: &Principal, action: Action, _: &str) -> bool {
        match (p.id.as_str(), p.kind) {
            (EXECUTOR_ID, PrincipalKind::Agent) => action == Action::Write,
            (HOST_ID, PrincipalKind::Code) => matches!(action, Action::Settle | Action::Request),
            (OPERATOR_ID, PrincipalKind::Human) => action == Action::Request,
            _ => false,
        }
    }
}

pub(crate) struct KernelRuns {
    pub(crate) kernel: Kernel<FileStore>,
    observations: super::run_observations::RunObservations,
    active: Mutex<HashMap<String, anchor_runtime::Cancellation>>,
    _lease: anchor_file_lease::FileLease,
}

struct ExecutionGuard<'a> {
    owner: &'a KernelRuns,
    id: String,
}
impl Drop for ExecutionGuard<'_> {
    fn drop(&mut self) {
        self.owner.active.lock().unwrap().remove(&self.id);
    }
}

impl KernelRuns {
    fn open(root: &Path) -> Result<Self, ApplicationError> {
        std::fs::create_dir_all(root).map_err(super::storage)?;
        let lease = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join(".writer.lock"))
            .map_err(super::storage)?;
        let lease = anchor_file_lease::FileLease::acquire(
            lease,
            rustix::fs::FlockOperation::NonBlockingLockExclusive,
        )
        .map_err(|e| {
            ApplicationError::Conflict(format!("Kernel probe store already has a writer: {e}"))
        })?;
        let mut kernel = Kernel::new(
            FileStore::new(root).map_err(|e| ApplicationError::Storage(e.to_string()))?,
            Box::new(ProbeAuthority),
        )
        .map_err(kernel_error)?;
        kernel
            .install(GRAPH_VALIDATOR, Arc::new(GraphValidator))
            .map_err(kernel_error)?;
        Ok(Self {
            kernel,
            active: Mutex::default(),
            observations: super::run_observations::RunObservations::new(
                &root
                    .parent()
                    .expect("state root")
                    .join("kernel-run-observations"),
            ),
            _lease: lease,
        })
    }
    pub(crate) fn load(&self, id: &str) -> Result<Option<Resource>, ApplicationError> {
        Ok(self
            .kernel
            .load(&run_resource_id(id))
            .map_err(kernel_error)?
            .filter(|r| r.validator == GRAPH_VALIDATOR))
    }
    pub(crate) fn create(
        &self,
        record: &GraphRunRecord,
        budget: u64,
    ) -> Result<(), ApplicationError> {
        validate_probe(record)?;
        self.kernel
            .create(run_resource(record, budget)?)
            .map_err(kernel_error)?;
        if let Err(error) = anchor_runtime::graph::scheduler::SubmissionObserver::accepted(
            &self.observations,
            &run_resource_id(&record.run_id),
        ) {
            eprintln!("Host initial update observation unavailable: {error}");
        }
        Ok(())
    }
    pub(crate) fn active(&self, id: &str) -> bool {
        self.active.lock().unwrap().contains_key(id)
    }
    pub(crate) async fn drive<A: ArtifactPort + ?Sized, N: NodeExecutionPort + ?Sized>(
        &self,
        id: &str,
        artifacts: &A,
        nodes: &N,
        cancellation: anchor_runtime::Cancellation,
    ) -> Result<Resource, GraphError> {
        {
            let mut active = self.active.lock().unwrap();
            if active.contains_key(id) {
                return Err(GraphError::RunBusy(id.to_owned()));
            }
            active.insert(id.to_owned(), cancellation.clone());
        }
        let _guard = ExecutionGuard {
            owner: self,
            id: id.to_owned(),
        };
        KernelScheduler {
            kernel: &self.kernel,
            observer: Some(&self.observations),
            artifacts,
            nodes,
            cancellation,
        }
        .run(&run_resource_id(id))
        .await
    }
    pub(crate) fn control(
        &self,
        id: &str,
        operation: &str,
        reason: Option<&str>,
    ) -> Result<(), ApplicationError> {
        if !matches!(operation, "stop" | "resume" | "abandon") {
            return Err(ApplicationError::Invalid(
                "P probe: pause is Unsupported; use stop/resume".into(),
            ));
        }
        if operation == "resume" && self.active(id) {
            return Err(ApplicationError::Conflict(
                "Kernel probe execution is still draining".into(),
            ));
        }
        let resource = self.load(id)?.ok_or(ApplicationError::Missing)?;
        let submission = Submission::new(&resource.id, resource.version, Action::Request)
            .content(serde_json::json!({"control":operation,"reason":reason.unwrap_or(operation)}));
        let verdict = submit_with_retry(
            &self.kernel,
            &Principal::new(OPERATOR_ID, PrincipalKind::Human),
            0,
            submission,
            Some(&self.observations),
        )?;
        if !verdict.is_accepted() {
            return Err(ApplicationError::Conflict(format!(
                "Kernel control refused: {:?}",
                verdict.reason
            )));
        }
        if operation != "resume"
            && let Some(cancellation) = self.active.lock().unwrap().get(id)
        {
            cancellation.store(true, Ordering::Release);
        }
        Ok(())
    }
    pub(crate) fn record(resource: &Resource) -> Result<GraphRunRecord, ApplicationError> {
        super::run_json::record(resource)
    }
    pub(crate) fn view(
        &self,
        resource: &Resource,
    ) -> Result<super::run_view::RunView, ApplicationError> {
        let record = record_of(resource)?;
        Ok(super::run_view::RunView {
            reason: None,
            phase: resource
                .phase()
                .ok_or_else(|| ApplicationError::Storage("Kernel Run has no phase".into()))?
                .clone(),
            worker: self.active(&record.run_id),
            // Read observation failures never affect lifecycle or Session eligibility.
            updated: self.observations.load(&record.run_id).ok().flatten(),
            graph: (&record).into(),
        })
    }
}

impl RunApplication {
    pub(crate) fn kernel_runs(
        &self,
        create: bool,
    ) -> Result<Option<Arc<KernelRuns>>, ApplicationError> {
        let mut cached = self.kernel_probe.lock().unwrap();
        if cached.is_none() && (create || self.data_root.join("kernel-runs").exists()) {
            *cached = Some(Arc::new(KernelRuns::open(
                &self.data_root.join("kernel-runs"),
            )?));
        }
        Ok(cached.clone())
    }
    pub(crate) fn kernel_owner(
        &self,
        id: &str,
    ) -> Result<Option<Arc<KernelRuns>>, ApplicationError> {
        if !Self::valid_run_component(id) {
            return Err(ApplicationError::Invalid("invalid Run id".into()));
        }
        let Some(runs) = self.kernel_runs(false)? else {
            return Ok(None);
        };
        if runs.load(id)?.is_none() {
            return Ok(None);
        }
        if self.store().load(id)?.is_some() {
            return Err(ApplicationError::Conflict("Run has two authorities".into()));
        }
        Ok(Some(runs))
    }
    pub(crate) fn load_run(&self, id: &str) -> Result<Option<GraphRunRecord>, ApplicationError> {
        if let Some(runs) = self.kernel_owner(id)? {
            return runs.load(id)?.as_ref().map(KernelRuns::record).transpose();
        }
        Ok(self.store().load(id)?)
    }
    pub(crate) fn kernel_records(&self) -> Result<Vec<(String, GraphRunRecord)>, ApplicationError> {
        let Some(runs) = self.kernel_runs(false)? else {
            return Ok(Vec::new());
        };
        let mut records = Vec::new();
        for id in runs
            .kernel
            .store()
            .list("")
            .map_err(|e| ApplicationError::Storage(e.to_string()))?
        {
            if let Some(resource) = runs
                .kernel
                .load(&id)
                .map_err(kernel_error)?
                .filter(|r| r.validator == GRAPH_VALIDATOR)
            {
                let id = record_of(&resource)?.run_id;
                if self.store().load(&id)?.is_some() {
                    return Err(ApplicationError::Conflict("Run has two authorities".into()));
                }
                records.push((id, KernelRuns::record(&resource)?));
            }
        }
        Ok(records)
    }
}
use anchor_runtime::graph::RunStore;

#[cfg(test)]
mod tests;

impl RunApplication {
    pub(crate) fn spawn_kernel(
        &self,
        record: GraphRunRecord,
        execution: crate::execution::PreparedExecution,
        control: crate::HostControl,
    ) -> Result<(), ApplicationError> {
        let runs = self.kernel_runs(true)?.expect("Kernel probe enabled");
        let (artifacts, nodes) = execution.kernel_ports(self.clone());
        self.executor()?.spawn(async move {
            if let Err(error) = runs
                .drive(&record.run_id, &artifacts, &nodes, control.cancellation)
                .await
            {
                eprintln!("Kernel probe {} suspended: {error}", record.run_id);
            }
        });
        Ok(())
    }
    pub(crate) async fn kernel_control(
        &self,
        runs: Arc<KernelRuns>,
        id: &str,
        operation: &str,
    ) -> Result<(), ApplicationError> {
        if operation != "resume" {
            return runs.control(id, operation, None);
        }
        if runs.active(id) {
            return Err(ApplicationError::Conflict(
                "Kernel probe execution is still draining".into(),
            ));
        }
        let resource = runs.load(id)?.ok_or(ApplicationError::Missing)?;
        let record = KernelRuns::record(&resource)?;
        let metadata = self.metadata(id)?.ok_or(ApplicationError::Missing)?;
        let control = super::new_control();
        let execution = crate::execution::PreparedExecution::prepare_with_catalog(
            &record,
            control.clone(),
            record.plugin_bindings.clone(),
            self.catalog_root.clone(),
            self.clone(),
        )
        .map_err(ApplicationError::Invalid)?;
        execution
            .bind_local_inputs(&record, &metadata.graph)
            .map_err(ApplicationError::Invalid)?;
        if resource.phase() != Some(&Phase::Executing) {
            runs.control(id, "resume", None)?;
        }
        self.spawn_kernel(record, execution, control)
    }
}
