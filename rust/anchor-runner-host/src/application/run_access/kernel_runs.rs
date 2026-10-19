//! Explicit Kernel execution path. FileStore is the sole authority for these Runs;
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
    },
    validator::{GRAPH_VALIDATOR, GraphValidator, record_of, run_resource, run_resource_id},
};
use std::{
    collections::{BTreeSet, HashMap},
    fs::OpenOptions,
    path::Path,
    sync::{Arc, Mutex, atomic::Ordering},
};

pub(super) mod deletion;
mod pause;
#[path = "kernel_probe.rs"]
mod probe;
mod recovery;

struct KernelExecution {
    cancellation: anchor_runtime::Cancellation,
    pause: tokio::sync::mpsc::UnboundedSender<()>,
    wait_pause: Arc<tokio::sync::Notify>,
}

struct ProbeAuthority;
impl Authority for ProbeAuthority {
    fn permits(&self, p: &Principal, action: Action, _: &str) -> bool {
        match (p.id.as_str(), p.kind) {
            (EXECUTOR_ID, PrincipalKind::Agent) => action == Action::Write,
            (HOST_ID, PrincipalKind::Code) => {
                matches!(action, Action::Write | Action::Settle | Action::Request)
            }
            (OPERATOR_ID, PrincipalKind::Human) => action == Action::Request,
            _ => false,
        }
    }
}

pub(crate) struct KernelRuns {
    pub(crate) kernel: Kernel<FileStore>,
    observations: super::run_observations::RunObservations,
    active: Mutex<HashMap<String, KernelExecution>>,
    // Deletion reserves inactive Runs across filesystem cleanup. Registration,
    // creation and controls take active before this mutex.
    deleting: Mutex<BTreeSet<String>>,
    pause_root: std::path::PathBuf,
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
    pub(crate) fn abandon(
        &self,
        run_id: &str,
        reason: String,
    ) -> Result<super::super::abandon::AbandonResult, ApplicationError> {
        self.control(run_id, "abandon", Some(&reason))?;
        Ok(super::super::abandon::AbandonResult {
            action: super::super::AbandonAction::Abandoned,
            status: anchor_runtime::graph::RunStatus::Aborted,
            reason: Some(reason),
            requested_at: None,
        })
    }
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
        let probe = probe::HostProbe::new(
            &root.parent().expect("state root").join("kernel-probe"),
            kernel.store(),
        );
        kernel = kernel.with_probe(Box::new(probe));
        kernel
            .install(GRAPH_VALIDATOR, Arc::new(GraphValidator))
            .map_err(kernel_error)?;
        Ok(Self {
            kernel,
            active: Mutex::default(),
            deleting: Mutex::default(),
            pause_root: root
                .parent()
                .expect("state root")
                .join("kernel-pause-intents"),
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
        crate::reject_snapshot(&record.snapshot).map_err(ApplicationError::Invalid)?;
        let _active = self.active.lock().unwrap();
        self.ensure_not_deleting(&record.run_id)?;
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
        let (pause, requests) = tokio::sync::mpsc::unbounded_channel();
        let wait_pause = Arc::new(tokio::sync::Notify::new());
        {
            let mut active = self.active.lock().unwrap();
            if active.contains_key(id) {
                return Err(GraphError::RunBusy(id.to_owned()));
            }
            self.ensure_not_deleting(id)
                .map_err(|_| GraphError::RunBusy(id.to_owned()))?;
            // A worker may have been queued before deletion acquired its
            // reservation. It must observe absence before executing any slot.
            if self
                .load(id)
                .map_err(|error| {
                    GraphError::CorruptRun(format!("cannot load Kernel Run: {error:?}"))
                })?
                .is_none()
            {
                return Err(GraphError::CorruptRun("missing Kernel Run".into()));
            }
            if self.pause_pending(id)? {
                pause.send(()).expect("new pause receiver");
                wait_pause.notify_one();
            }
            active.insert(
                id.to_owned(),
                KernelExecution {
                    cancellation: cancellation.clone(),
                    pause,
                    wait_pause,
                },
            );
        }
        let _guard = ExecutionGuard {
            owner: self,
            id: id.to_owned(),
        };
        let boundary = pause::PauseBoundary::new(self, requests);
        KernelScheduler {
            kernel: &self.kernel,
            observer: Some(&self.observations),
            boundary: Some(&boundary),
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
        if operation == "pause" {
            return self.request_pause(id);
        }
        if !matches!(operation, "stop" | "resume" | "abandon") {
            return Err(ApplicationError::Invalid(
                "unknown Kernel Run control".into(),
            ));
        }
        let active = self.active.lock().unwrap();
        self.ensure_not_deleting(id)?;
        if operation == "resume" && active.contains_key(id) {
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
            && let Some(execution) = active.get(id)
        {
            execution.cancellation.store(true, Ordering::Release);
        }
        self.clear_pause(id)?;
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
        Self::project(
            resource,
            self.active(&record.run_id),
            self.observations.load(&record.run_id).ok().flatten(),
        )
    }
    pub(super) fn project(
        resource: &Resource,
        worker: bool,
        updated: Option<String>,
    ) -> Result<super::run_view::RunView, ApplicationError> {
        Ok(super::run_view::RunView {
            reason: None,
            phase: resource
                .phase()
                .ok_or_else(|| ApplicationError::Storage("Kernel Run has no phase".into()))?
                .clone(),
            worker,
            updated,
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
    pub(in crate::application::run_access) fn load_run_impl(
        &self,
        id: &str,
    ) -> Result<Option<GraphRunRecord>, ApplicationError> {
        if let Some(runs) = self.kernel_owner(id)? {
            return runs.load(id)?.as_ref().map(KernelRuns::record).transpose();
        }
        Ok(self.store().load(id)?)
    }
    #[cfg(not(test))]
    pub(in crate::application::run_access) fn load_run(
        &self,
        id: &str,
    ) -> Result<Option<GraphRunRecord>, ApplicationError> {
        self.load_run_impl(id)
    }
    #[cfg(test)]
    pub(crate) fn load_run(&self, id: &str) -> Result<Option<GraphRunRecord>, ApplicationError> {
        self.load_run_impl(id)
    }
    /// Runtime adapter only; business consumers use read_run and RunView.
    pub(crate) fn execution_record(
        &self,
        id: &str,
    ) -> Result<Option<GraphRunRecord>, ApplicationError> {
        self.load_run_impl(id)
    }
    pub(in crate::application::run_access) fn kernel_records(
        &self,
    ) -> Result<Vec<(String, GraphRunRecord)>, ApplicationError> {
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
    /// An idle Session wait can suspend on a pause event without cancelling
    /// work or polling a second Run state. The dispatch boundary owns stop.
    pub(crate) fn kernel_wait_pause(
        &self,
        id: &str,
    ) -> Result<Option<Arc<tokio::sync::Notify>>, ApplicationError> {
        let Some(runs) = self.kernel_owner(id)? else {
            return Ok(None);
        };
        let active = runs.active.lock().unwrap();
        Ok(active.get(id).map(|execution| execution.wait_pause.clone()))
    }

    pub(crate) fn spawn_kernel(
        &self,
        record: GraphRunRecord,
        execution: crate::execution::PreparedExecution,
        control: crate::HostControl,
    ) -> Result<(), ApplicationError> {
        let runs = self.kernel_runs(true)?.expect("Kernel probe enabled");
        let (artifacts, nodes) = execution.kernel_ports(self.clone());
        let application = self.clone();
        self.executor()?.spawn(async move {
            let nodes = super::kernel_calls::KernelCallNodes::new(&application, &artifacts, &nodes);
            if let Err(error) = runs
                .drive(&record.run_id, &artifacts, &nodes, control.cancellation)
                .await
            {
                eprintln!("Kernel probe {} suspended: {error}", record.run_id);
            }
            if let Err(error) = application
                .settle_channel_run(&record.run_id, anchor_runtime::graph::RunStatus::Stopped)
            {
                eprintln!(
                    "Kernel channel {} settlement failed: {error:?}",
                    record.run_id
                );
            }
            // A child can settle while its parent worker is releasing the
            // active slot. Recheck the durable wait once after that release.
            if let Err(error) = application.kernel_recheck_child(&record.run_id).await {
                eprintln!(
                    "Kernel parent {} child observation failed: {error:?}",
                    record.run_id
                );
            }
            if let Err(error) = application.kernel_child_finished(&record.run_id).await {
                eprintln!(
                    "Kernel child {} completion callback failed: {error:?}",
                    record.run_id
                );
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
        let active = self.active.lock().await;
        self.check_execution_scope(&metadata, &active)?;
        if !runs.recovery_facts(id)?.0.is_empty() {
            return Err(ApplicationError::Conflict(
                "record a recovery observation or abandon before resuming an unknown effect".into(),
            ));
        }
        let control = super::new_control();
        self.reject_superseded_conversation_execution(id)?;
        let execution = self.prepare_resume(&record, &metadata, control.clone())?;
        execution
            .bind_local_inputs(&record, &metadata.graph)
            .map_err(ApplicationError::Invalid)?;
        if resource.phase() != Some(&Phase::Executing) {
            runs.control(id, "resume", None)?;
        }
        self.spawn_kernel(record, execution, control)
    }
}
