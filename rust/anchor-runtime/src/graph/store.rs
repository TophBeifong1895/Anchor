use super::*;

pub trait RunLease: Send {}

#[derive(Debug, Clone)]
pub struct FileRunLeaseProvider {
    root: PathBuf,
}
impl FileRunLeaseProvider {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
    pub fn acquire_read_lease(&self, run_id: &str) -> Result<Box<dyn RunLease>, GraphError> {
        self.acquire_file_lease(
            run_id,
            anchor_file_lease::FlockOperation::NonBlockingLockShared,
        )
    }
    fn acquire_file_lease(
        &self,
        run_id: &str,
        operation: anchor_file_lease::FlockOperation,
    ) -> Result<Box<dyn RunLease>, GraphError> {
        validate_component(run_id)?;
        fs::create_dir_all(&self.root)?;
        let path = self.root.join(format!(".{run_id}.lock"));
        // Keep the lock inode stable. The OS releases this advisory lock if
        // the process exits, including an unclean crash; never unlink it.
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        let file =
            anchor_file_lease::FileLease::acquire(file, operation).map_err(
                |error| match error {
                    error
                        if std::io::Error::from(error).kind() == std::io::ErrorKind::WouldBlock =>
                    {
                        GraphError::RunBusy(run_id.to_owned())
                    }
                    error => GraphError::Io(error.into()),
                },
            )?;
        file.sync_all()?;
        Ok(Box::new(FileRunLease { _file: file }))
    }
    /// Hold an exclusive process lease without owning Run persistence.
    pub fn acquire_lease(&self, run_id: &str) -> Result<Box<dyn RunLease>, GraphError> {
        self.acquire_file_lease(
            run_id,
            anchor_file_lease::FlockOperation::NonBlockingLockExclusive,
        )
    }
}
struct FileRunLease {
    _file: anchor_file_lease::FileLease,
}
impl RunLease for FileRunLease {}
