//! Host observation only. Never used for lifecycle, authorization or recovery.
use super::{ApplicationError, storage};
use anchor_runtime::graph::{GraphError, scheduler::SubmissionObserver};
use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};

pub(super) struct RunObservations {
    root: PathBuf,
    writer: Mutex<()>,
}
#[derive(Serialize, Deserialize)]
struct Observation {
    updated: String,
}
impl RunObservations {
    pub(super) fn new(root: &Path) -> Self {
        Self {
            root: root.to_owned(),
            writer: Mutex::new(()),
        }
    }
    pub(super) fn load(&self, id: &str) -> Result<Option<String>, ApplicationError> {
        let bytes = match std::fs::read(self.root.join(format!("{id}.json"))) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(storage(error)),
        };
        serde_json::from_slice::<Observation>(&bytes)
            .map(|v| Some(v.updated))
            .map_err(|e| ApplicationError::Storage(e.to_string()))
    }
}
impl SubmissionObserver for RunObservations {
    fn accepted(&self, resource_id: &str) -> Result<(), GraphError> {
        let id = resource_id
            .strip_prefix("run/")
            .filter(|id| super::RunApplication::valid_run_component(id))
            .ok_or_else(|| GraphError::CorruptRun("invalid observation Run id".into()))?;
        let _writer = self.writer.lock().unwrap();
        std::fs::create_dir_all(&self.root)?;
        let value = Observation {
            updated: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
        };
        let mut temporary = tempfile::NamedTempFile::new_in(&self.root)?;
        temporary.write_all(&serde_json::to_vec(&value).map_err(GraphError::RunDecode)?)?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(self.root.join(format!("{id}.json")))
            .map_err(|e| e.error)?;
        std::fs::File::open(&self.root)?.sync_all()?;
        Ok(())
    }
}
