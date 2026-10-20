//! L0: where records come from. Every read of recorded history goes through
//! [`HistorySource`]; nothing above this layer touches a data root.

use std::fmt;
use std::path::Path;

use anchor_kernel::{HistoryReader, Record, Resource, Store, StoreError};

/// A read-only kernel history: resources by id, each with its records in the
/// order they were appended. A record's place is its 1-based position there.
pub trait HistorySource {
    /// The name rows from this source carry, telling its records apart from
    /// another root's.
    fn root(&self) -> &str;
    fn list(&self) -> Result<Vec<String>, SourceError>;
    fn load(&self, id: &str) -> Result<Option<Resource>, SourceError>;
    /// The records after the first `after`, so a reader can continue where
    /// it stopped.
    fn history_after(&self, id: &str, after: usize) -> Result<Vec<Record>, SourceError>;
}

#[derive(Debug)]
pub struct SourceError {
    pub root: String,
    pub error: StoreError,
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.root, self.error)
    }
}

impl std::error::Error for SourceError {}

/// A kernel data root on disk, read through [`HistoryReader`]: no lease, no
/// writes, and a writer racing it is seen up to its last complete record.
pub struct DataRoot {
    name: String,
    reader: HistoryReader,
}

impl DataRoot {
    pub fn open(path: &Path, name: impl Into<String>) -> Result<Self, SourceError> {
        let name = name.into();
        let reader = HistoryReader::open(path).map_err(|error| SourceError {
            root: name.clone(),
            error,
        })?;
        Ok(Self { name, reader })
    }

    fn fail(&self, error: StoreError) -> SourceError {
        SourceError {
            root: self.name.clone(),
            error,
        }
    }
}

impl HistorySource for DataRoot {
    fn root(&self) -> &str {
        &self.name
    }
    fn list(&self) -> Result<Vec<String>, SourceError> {
        self.reader.list("").map_err(|e| self.fail(e))
    }
    fn load(&self, id: &str) -> Result<Option<Resource>, SourceError> {
        self.reader.load(id).map_err(|e| self.fail(e))
    }
    fn history_after(&self, id: &str, after: usize) -> Result<Vec<Record>, SourceError> {
        let mut records = self.reader.history(id).map_err(|e| self.fail(e))?;
        Ok(records.split_off(after.min(records.len())))
    }
}

/// Any kernel store read as a source, such as a `MemoryStore` in tests. Only
/// its reading methods are used.
pub struct Stored<'a, S: Store> {
    name: String,
    store: &'a S,
}

impl<'a, S: Store> Stored<'a, S> {
    pub fn new(store: &'a S, name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            store,
        }
    }

    fn fail(&self, error: StoreError) -> SourceError {
        SourceError {
            root: self.name.clone(),
            error,
        }
    }
}

impl<S: Store> HistorySource for Stored<'_, S> {
    fn root(&self) -> &str {
        &self.name
    }
    fn list(&self) -> Result<Vec<String>, SourceError> {
        self.store.list("").map_err(|e| self.fail(e))
    }
    fn load(&self, id: &str) -> Result<Option<Resource>, SourceError> {
        self.store.load(id).map_err(|e| self.fail(e))
    }
    fn history_after(&self, id: &str, after: usize) -> Result<Vec<Record>, SourceError> {
        let mut records = self.store.history(id).map_err(|e| self.fail(e))?;
        Ok(records.split_off(after.min(records.len())))
    }
}
