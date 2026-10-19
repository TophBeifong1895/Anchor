//! A pause is a durable Host command, consumed at a dispatch boundary. It never
//! cancels the current execution and is not a second source of Run phase.
use super::*;
use anchor_runtime::graph::scheduler::DispatchBoundary;
use std::io::Write;
use tokio::sync::mpsc::UnboundedReceiver;

pub(super) struct PauseBoundary<'a> {
    runs: &'a KernelRuns,
    requests: Mutex<UnboundedReceiver<()>>,
}

impl<'a> PauseBoundary<'a> {
    pub(super) fn new(runs: &'a KernelRuns, requests: UnboundedReceiver<()>) -> Self {
        Self {
            runs,
            requests: Mutex::new(requests),
        }
    }
}

impl DispatchBoundary for PauseBoundary<'_> {
    fn before_dispatch(&self, resource_id: &str) -> Result<(), GraphError> {
        // Serialize intent consumption with control acceptance. A later pause
        // must not lose its durable marker to this boundary's cleanup.
        let _active = self.runs.active.lock().unwrap();
        {
            let mut requests = self.requests.lock().unwrap();
            if requests.try_recv().is_err() {
                return Ok(());
            }
            while requests.try_recv().is_ok() {}
        }
        let resource = self
            .runs
            .kernel
            .load(resource_id)
            .map_err(kernel_error)?
            .ok_or_else(|| GraphError::CorruptRun("missing Kernel Run at pause boundary".into()))?;
        let id = resource_id
            .strip_prefix("run/")
            .ok_or_else(|| GraphError::CorruptRun("invalid Kernel Run at pause boundary".into()))?;
        if resource.phase() == Some(&Phase::Executing) {
            let verdict = submit_with_retry(
                &self.runs.kernel,
                &Principal::new(OPERATOR_ID, PrincipalKind::Human),
                0,
                Submission::new(resource_id, resource.version, Action::Request)
                    .content(serde_json::json!({"control":"stop","reason":"paused"})),
                Some(&self.runs.observations),
            )?;
            if !verdict.is_accepted()
                && !matches!(
                    verdict.reason,
                    anchor_kernel::Reason::TaskWaiting | anchor_kernel::Reason::TaskSettled
                )
            {
                return Err(GraphError::RunConflict);
            }
        }
        self.runs.clear_pause(id)?;
        Ok(())
    }
}

impl KernelRuns {
    fn pause_path(&self, id: &str) -> std::path::PathBuf {
        self.pause_root.join(format!("{id}.pause"))
    }

    pub(super) fn pause_pending(&self, id: &str) -> Result<bool, GraphError> {
        match std::fs::read(self.pause_path(id)) {
            Ok(bytes) if bytes == b"pause\n" => Ok(true),
            Ok(_) => Err(GraphError::CorruptRun("invalid Host pause intent".into())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    pub(super) fn clear_pause(&self, id: &str) -> Result<(), GraphError> {
        match std::fs::remove_file(self.pause_path(id)) {
            Ok(()) => std::fs::File::open(&self.pause_root)?
                .sync_all()
                .map_err(Into::into),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    pub(super) fn request_pause(&self, id: &str) -> Result<(), ApplicationError> {
        let active = self.active.lock().unwrap();
        let execution = active
            .get(id)
            .ok_or_else(|| ApplicationError::Conflict("that Run is not active".into()))?;
        let resource = self.load(id)?.ok_or(ApplicationError::Missing)?;
        if resource.phase() != Some(&Phase::Executing) {
            return Err(ApplicationError::Conflict(
                "only an executing Run can be paused".into(),
            ));
        }
        std::fs::create_dir_all(&self.pause_root).map_err(super::super::storage)?;
        let mut temporary =
            tempfile::NamedTempFile::new_in(&self.pause_root).map_err(super::super::storage)?;
        temporary
            .write_all(b"pause\n")
            .map_err(super::super::storage)?;
        temporary
            .as_file()
            .sync_all()
            .map_err(super::super::storage)?;
        temporary
            .persist(self.pause_path(id))
            .map_err(|error| super::super::storage(error.error))?;
        std::fs::File::open(&self.pause_root)
            .and_then(|directory| directory.sync_all())
            .map_err(super::super::storage)?;
        execution
            .pause
            .send(())
            .map_err(|_| ApplicationError::Conflict("Kernel execution has ended".into()))?;
        execution.wait_pause.notify_one();
        Ok(())
    }
}
