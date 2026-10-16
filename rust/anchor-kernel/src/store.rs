//! Each resource owner stores its own state and its own append-only history.
//! There is no global log: the protocol is shared, storage is not.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::model::{Record, Resource};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("resource `{0}` already exists")]
    Exists(String),
    #[error("resource `{0}` does not exist")]
    Missing(String),
    #[error("invalid resource id `{0}`")]
    InvalidId(String),
    #[error("storage failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("stored record is unreadable: {0}")]
    Corrupt(String),
}

pub trait Store: Send + Sync {
    /// The current resource, including any accepted change whose state write
    /// was interrupted after its record was appended. A task's spent budget
    /// is derived from every record's cost, independently of resource versions.
    fn load(&self, id: &str) -> Result<Option<Resource>, StoreError>;
    /// Create a resource. Fails if it exists: creation is never a silent
    /// overwrite.
    fn create(&self, resource: &Resource) -> Result<(), StoreError>;
    /// Append `record` to the history of `id`, then, if it was accepted, save
    /// `record.after` as the current state. The record is durable first. Only
    /// newline-complete records are history; an incomplete tail is discarded
    /// before appending. The caller holds the kernel's writer lock.
    fn commit(&self, id: &str, record: &Record) -> Result<(), StoreError>;
    fn history(&self, id: &str) -> Result<Vec<Record>, StoreError>;
    /// Ids starting with `prefix`, in order.
    fn list(&self, prefix: &str) -> Result<Vec<String>, StoreError>;
}

// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct MemoryStore {
    inner: Mutex<BTreeMap<String, (Resource, Vec<Record>)>>,
}

impl Store for MemoryStore {
    fn load(&self, id: &str) -> Result<Option<Resource>, StoreError> {
        Ok(self
            .inner
            .lock()
            .expect("store lock")
            .get(id)
            .map(|(resource, history)| {
                let mut resource = resource.clone();
                derive_spent(&mut resource, history);
                resource
            }))
    }

    fn create(&self, resource: &Resource) -> Result<(), StoreError> {
        let mut inner = self.inner.lock().expect("store lock");
        if inner.contains_key(&resource.id) {
            return Err(StoreError::Exists(resource.id.clone()));
        }
        inner.insert(resource.id.clone(), (resource.clone(), Vec::new()));
        Ok(())
    }

    fn commit(&self, id: &str, record: &Record) -> Result<(), StoreError> {
        let mut inner = self.inner.lock().expect("store lock");
        let (resource, history) = inner
            .get_mut(id)
            .ok_or_else(|| StoreError::Missing(id.into()))?;
        history.push(record.clone());
        if let Some(after) = &record.after {
            *resource = after.clone();
        }
        Ok(())
    }

    fn history(&self, id: &str) -> Result<Vec<Record>, StoreError> {
        Ok(self
            .inner
            .lock()
            .expect("store lock")
            .get(id)
            .map(|(_, h)| h.clone())
            .unwrap_or_default())
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>, StoreError> {
        Ok(self
            .inner
            .lock()
            .expect("store lock")
            .range(prefix.to_string()..)
            .take_while(|(k, _)| k.starts_with(prefix))
            .map(|(k, _)| k.clone())
            .collect())
    }
}

/// Costs are facts in the history even when a submission changes no version.
fn derive_spent(resource: &mut Resource, history: &[Record]) {
    if let Some(task) = &mut resource.task {
        task.budget.spent = history.iter().fold(0_u64, |spent, record| {
            spent.saturating_add(record.stamp.cost())
        });
    }
}

// ---------------------------------------------------------------------------

/// One directory per resource: `resource.json` (current state, replaced
/// atomically) and `history.jsonl` (append-only, fsynced before the state).
pub struct FileStore {
    root: PathBuf,
}

impl FileStore {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    fn dir(&self, id: &str) -> Result<PathBuf, StoreError> {
        Ok(self.root.join(encode(id)?))
    }
}

/// Keep `[A-Za-z0-9-_]`, escape everything else, so ids map to one safe
/// directory name each and `/` or `..` can never escape the root.
fn encode(id: &str) -> Result<String, StoreError> {
    if id.is_empty() || id.len() > 512 {
        return Err(StoreError::InvalidId(id.into()));
    }
    let mut out = String::with_capacity(id.len());
    for byte in id.bytes() {
        if byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_' {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    Ok(out)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    let tmp = path.with_extension("tmp");
    {
        let mut file = File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

fn read_resource(path: &Path) -> Result<Resource, StoreError> {
    let bytes = fs::read(path)?;
    serde_json::from_slice(&bytes).map_err(|e| StoreError::Corrupt(e.to_string()))
}

/// A crash can leave bytes that never formed a record. Locate the last
/// newline from the end without rewriting any existing complete records.
fn trim_incomplete_tail(file: &mut File) -> Result<(), std::io::Error> {
    let original_len = file.metadata()?.len();
    let mut end = original_len;
    let mut chunk = [0_u8; 8192];
    while end > 0 {
        let start = end.saturating_sub(chunk.len() as u64);
        let len = (end - start) as usize;
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(&mut chunk[..len])?;
        if let Some(newline) = chunk[..len].iter().rposition(|byte| *byte == b'\n') {
            let complete_len = start + newline as u64 + 1;
            if complete_len != original_len {
                file.set_len(complete_len)?;
            }
            return Ok(());
        }
        end = start;
    }
    if original_len > 0 {
        file.set_len(0)?;
    }
    Ok(())
}

impl Store for FileStore {
    fn load(&self, id: &str) -> Result<Option<Resource>, StoreError> {
        let dir = self.dir(id)?;
        let path = dir.join("resource.json");
        if !path.exists() {
            return Ok(None);
        }
        let mut resource = read_resource(&path)?;
        let history = self.history(id)?;
        // Crash window: the record was appended but the state not yet saved.
        // The record carries the resulting resource, so recovery is a read.
        if let Some(after) = history.iter().rev().find_map(|r| r.after.as_ref())
            && after.version > resource.version
        {
            resource = after.clone();
        }
        derive_spent(&mut resource, &history);
        Ok(Some(resource))
    }

    fn create(&self, resource: &Resource) -> Result<(), StoreError> {
        let dir = self.dir(&resource.id)?;
        match fs::create_dir(&dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(StoreError::Exists(resource.id.clone()));
            }
            Err(e) => return Err(e.into()),
        }
        let bytes = serde_json::to_vec(resource).map_err(|e| StoreError::Corrupt(e.to_string()))?;
        write_atomic(&dir.join("resource.json"), &bytes)
    }

    fn commit(&self, id: &str, record: &Record) -> Result<(), StoreError> {
        let dir = self.dir(id)?;
        if !dir.join("resource.json").exists() {
            return Err(StoreError::Missing(id.into()));
        }
        let mut line =
            serde_json::to_vec(record).map_err(|e| StoreError::Corrupt(e.to_string()))?;
        line.push(b'\n');
        let mut history = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(dir.join("history.jsonl"))?;
        trim_incomplete_tail(&mut history)?;
        history.write_all(&line)?;
        history.sync_all()?;
        if let Some(after) = &record.after {
            let bytes =
                serde_json::to_vec(after).map_err(|e| StoreError::Corrupt(e.to_string()))?;
            write_atomic(&dir.join("resource.json"), &bytes)?;
        }
        Ok(())
    }

    fn history(&self, id: &str) -> Result<Vec<Record>, StoreError> {
        let path = self.dir(id)?.join("history.jsonl");
        if !path.exists() {
            return Ok(Vec::new());
        }
        let mut records = Vec::new();
        let mut reader = BufReader::new(File::open(&path)?);
        let mut line = Vec::new();
        while reader.read_until(b'\n', &mut line)? > 0 {
            // Even valid JSON without its final newline was never a complete
            // record. Only this unterminated tail may be ignored.
            if line.last() != Some(&b'\n') {
                break;
            }
            let record = serde_json::from_slice(&line).map_err(|error| {
                StoreError::Corrupt(format!(
                    "{}: line {}: {error}",
                    path.display(),
                    records.len() + 1
                ))
            })?;
            records.push(record);
            line.clear();
        }
        Ok(records)
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>, StoreError> {
        let mut ids = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let name = entry?.file_name();
            if let Some(id) = name.to_str().and_then(decode)
                && id.starts_with(prefix)
            {
                ids.push(id);
            }
        }
        ids.sort();
        Ok(ids)
    }
}

fn decode(name: &str) -> Option<String> {
    let bytes = name.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}
