//! Published dependency environments and the deployment-scoped name table.
//!
//! An environment is immutable content addressed by a digest over the published
//! tree: entry names (raw bytes), directory entries, file modes and file bytes
//! all contribute, so the same digest always means the same tree and different
//! trees cannot collide. A published directory is never modified in place;
//! changing anything means publishing a new digest and switching a name.
//!
//! Names are a deployment-scoped table that resolves a readable combination name
//! to exactly one published digest. Switching or removing a name is a
//! compare-and-swap on that name's version: a decision made against a stale view
//! is refused instead of overwriting a newer one.
//!
//! Identity, publication and the name table live here. Building an environment,
//! approving a plan and binding a name into a Graph or Plugin belong to their own
//! layers; see `docs/dependency-environment-draft.md`.
//!
//! Read-only mode bits are a signal, not a security boundary: the same owner can
//! restore them. Deployment hardening (a separate owner or a read-only mount) is
//! what makes the property enforceable, which is why [`EnvironmentStore::verify`]
//! reports a published tree that is still writable.

use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
};

use rustix::fs::FlockOperation;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const ENVS: &str = "envs";
const LOCK: &str = ".env.lock";
const NAMES: &str = "names.json";
const STAGING_PREFIX: &str = ".env-staging-";

#[derive(Debug, thiserror::Error)]
pub enum EnvironmentError {
    #[error("environment name must be a non-empty identifier of [A-Za-z0-9._-]")]
    InvalidName,
    #[error("environment id must be a 64-character hex digest")]
    InvalidId,
    #[error("environment content must contain only real directories and regular files")]
    UnsafePath,
    #[error("environment content is empty")]
    EmptyContent,
    #[error(
        "environment content is not the approved content: expected {expected}, computed {actual}"
    )]
    UnexpectedContent { expected: String, actual: String },
    #[error("environment content must not contain or live inside the environment store")]
    OverlappingSource,
    #[error("unknown environment `{0}`")]
    Unknown(String),
    #[error("unknown environment name `{0}`")]
    UnknownName(String),
    #[error(
        "name `{name}` changed while the decision was being made: expected version {expected}, found {actual}"
    )]
    VersionConflict {
        name: String,
        expected: u64,
        actual: u64,
    },
    #[error("environment name table could not be decoded; refusing to rewrite it")]
    InvalidTable,
    #[error("published environment `{0}` does not match its digest")]
    Corrupt(String),
    #[error(
        "environment publication durability is uncertain; inspect the store before retrying: {0}"
    )]
    PublicationUncertain(String),
    #[error("environment storage failed: {0}")]
    Io(#[from] std::io::Error),
}

/// One published environment, addressed by the digest of its content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublishedEnvironment {
    pub id: String,
    pub bytes: u64,
    pub files: usize,
    pub directories: usize,
}

/// What a candidate directory would publish as, before it is published.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CandidateContent {
    pub id: String,
    pub bytes: u64,
    pub files: usize,
}

/// Result of publishing content, including whether the digest already existed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublishOutcome {
    pub id: String,
    pub bytes: u64,
    pub files: usize,
    pub directories: usize,
    pub reused: bool,
}

/// One name in the deployment table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NameEntry {
    pub name: String,
    pub id: String,
    pub version: u64,
}

/// Result of a name switch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NameSwitch {
    pub name: String,
    pub id: String,
    pub version: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct NameRecord {
    id: String,
    version: u64,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct NameFile {
    #[serde(default)]
    names: BTreeMap<String, NameRecord>,
}

/// What one tree walk observed.
#[derive(Debug, Default, Clone, Copy)]
struct Totals {
    bytes: u64,
    files: usize,
    directories: usize,
}

/// Listing totals plus whether anything in the tree is still writable.
struct Measure {
    totals: Totals,
    write_bits: bool,
}

/// Content-addressed environment storage plus its deployment name table.
#[derive(Debug, Clone)]
pub struct EnvironmentStore {
    root: PathBuf,
}

impl EnvironmentStore {
    pub fn new(library_root: impl Into<PathBuf>) -> Self {
        Self {
            root: library_root.into(),
        }
    }

    fn envs(&self) -> PathBuf {
        self.root.join(ENVS)
    }

    /// The library root this store reads, for diagnostics.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Open the environment directory and take its lease.
    ///
    /// Mutations create the store and take the exclusive lease; readers neither
    /// create anything nor need write access, so `list`/`verify` still work on a
    /// hardened deployment. A missing store is reported as `None` rather than
    /// created by a typo in `--root`.
    fn locked(&self, create: bool) -> Result<Option<(PathBuf, File)>, EnvironmentError> {
        let envs = self.envs();
        if create {
            fs::create_dir_all(&envs)?;
        }
        match fs::symlink_metadata(&envs) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => return Err(EnvironmentError::UnsafePath),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        }
        // A read path takes a shared lock on the existing lock file and never
        // creates it: `env list/show/names/verify` must work on a read-only mount
        // or as a different owner. Only a mutation creates the file and takes the
        // exclusive lock.
        let lock = if create {
            OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
                .open(envs.join(LOCK))?
        } else {
            match OpenOptions::new()
                .read(true)
                .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
                .open(envs.join(LOCK))
            {
                Ok(lock) => lock,
                // No lock file means no publication ever happened here.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error.into()),
            }
        };
        if !lock.metadata()?.is_file() {
            return Err(EnvironmentError::UnsafePath);
        }
        let operation = if create {
            FlockOperation::LockExclusive
        } else {
            FlockOperation::LockShared
        };
        rustix::fs::flock(&lock, operation).map_err(std::io::Error::from)?;
        Ok(Some((envs, lock)))
    }

    /// Publish a prepared tree and return its digest.
    ///
    /// The digest is computed over the staged copy, so the identity always
    /// describes the bytes that were actually published. Existing content with
    /// the same digest is reused — and re-secured, so a published tree that lost
    /// its read-only bits is repaired instead of blessed. Existing content under
    /// the same digest that no longer matches is reported as corrupt rather than
    /// overwritten.
    pub fn publish(
        &self,
        source_dir: impl AsRef<Path>,
    ) -> Result<PublishOutcome, EnvironmentError> {
        self.publish_expecting(source_dir, None)
    }

    /// Publish only if the content is exactly the identity the caller approved.
    ///
    /// The comparison happens after the copy and *before* anything becomes
    /// visible, so an approval can never publish a tree it did not approve, and
    /// the tree is still copied only once.
    pub fn publish_expecting(
        &self,
        source_dir: impl AsRef<Path>,
        expected_id: Option<&str>,
    ) -> Result<PublishOutcome, EnvironmentError> {
        let source = source_dir.as_ref();
        let Some((envs, _lease)) = self.locked(true)? else {
            return Err(EnvironmentError::Io(std::io::Error::other(
                "environment store is unavailable",
            )));
        };
        let source = fs::canonicalize(source)?;
        // Publishing the store into itself would walk the staging directory.
        if envs.starts_with(&source) || source.starts_with(&envs) {
            return Err(EnvironmentError::OverlappingSource);
        }
        let staging = tempfile::Builder::new()
            .prefix(STAGING_PREFIX)
            .tempdir_in(&envs)?;
        let staged = staging.path().join("content");
        fs::create_dir(&staged)?;
        let (id, totals) = copy_and_hash(&source, &staged)?;
        if let Some(expected) = expected_id
            && expected != id
        {
            // Nothing has been made visible yet.
            return Err(EnvironmentError::UnexpectedContent {
                expected: expected.to_owned(),
                actual: id,
            });
        }
        let destination = envs.join(&id);
        match fs::symlink_metadata(&destination) {
            Ok(metadata) => {
                if !metadata.is_dir() {
                    return Err(EnvironmentError::UnsafePath);
                }
                let (existing, _) = digest_tree(&destination)?;
                if existing != id {
                    return Err(EnvironmentError::Corrupt(id));
                }
                // A published tree that became writable again is repaired here,
                // because the reuse path is otherwise a way to bless it.
                make_read_only(&destination)?;
                return Ok(PublishOutcome {
                    id,
                    bytes: totals.bytes,
                    files: totals.files,
                    directories: totals.directories,
                    reused: true,
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        // Content becomes read-only before it becomes visible. The moved
        // directory keeps its own write bit across the rename because the kernel
        // updates that directory's `..` entry when its parent changes; the bit is
        // removed again as soon as the content is in place.
        make_read_only(&staged)?;
        let mut staged_permissions = fs::metadata(&staged)?.permissions();
        staged_permissions.set_mode(staged_permissions.mode() | 0o200);
        fs::set_permissions(&staged, staged_permissions)?;
        fs::rename(&staged, &destination)?;
        // Anything past this point has a visible directory, so a failure is
        // reported as uncertain rather than as an ordinary error.
        if let Err(error) = make_read_only(&destination) {
            return Err(EnvironmentError::PublicationUncertain(format!(
                "{id} is published but not read-only: {error}"
            )));
        }
        if let Err(error) = sync_tree(&destination) {
            return Err(EnvironmentError::PublicationUncertain(format!(
                "{id} may not be durable: {error}"
            )));
        }
        File::open(&envs)?.sync_all()?;
        Ok(PublishOutcome {
            id,
            bytes: totals.bytes,
            files: totals.files,
            directories: totals.directories,
            reused: false,
        })
    }

    /// The identity `publish` would assign to this directory, without publishing
    /// it.
    ///
    /// Staging is the one `publish` uses, so a human approval can be bound to
    /// exactly the content that would become visible; no store root is needed
    /// because nothing is written outside the system temporary directory.
    pub fn digest_candidate(
        source_dir: impl AsRef<Path>,
    ) -> Result<CandidateContent, EnvironmentError> {
        let source = fs::canonicalize(source_dir.as_ref())?;
        let staging = tempfile::Builder::new().prefix(STAGING_PREFIX).tempdir()?;
        let staged = staging.path().join("content");
        fs::create_dir(&staged)?;
        let (id, totals) = copy_and_hash(&source, &staged)?;
        Ok(CandidateContent {
            id,
            bytes: totals.bytes,
            files: totals.files,
        })
    }

    /// Every published environment, without re-hashing its content.
    pub fn list(&self) -> Result<Vec<PublishedEnvironment>, EnvironmentError> {
        let Some((envs, _lease)) = self.locked(false)? else {
            return Ok(Vec::new());
        };
        let mut published = Vec::new();
        for directory in published_directories(&envs)? {
            let totals = measure(&directory)?.totals;
            published.push(PublishedEnvironment {
                id: file_name(&directory),
                bytes: totals.bytes,
                files: totals.files,
                directories: totals.directories,
            });
        }
        Ok(published)
    }

    /// Statistics for one published environment, after re-hashing it.
    pub fn show(&self, id: &str) -> Result<PublishedEnvironment, EnvironmentError> {
        validate_id(id)?;
        let Some((envs, _lease)) = self.locked(false)? else {
            return Err(EnvironmentError::Unknown(id.to_owned()));
        };
        let directory = envs.join(id);
        if !directory.is_dir() {
            return Err(EnvironmentError::Unknown(id.to_owned()));
        }
        let (digest, totals) = digest_tree(&directory)?;
        if digest != id {
            return Err(EnvironmentError::Corrupt(id.to_owned()));
        }
        Ok(PublishedEnvironment {
            id: id.to_owned(),
            bytes: totals.bytes,
            files: totals.files,
            directories: totals.directories,
        })
    }

    /// Every name in the deployment table, sorted.
    pub fn names(&self) -> Result<Vec<NameEntry>, EnvironmentError> {
        let Some((envs, _lease)) = self.locked(false)? else {
            return Ok(Vec::new());
        };
        Ok(read_names(&envs)?
            .names
            .into_iter()
            .map(|(name, record)| NameEntry {
                name,
                id: record.id,
                version: record.version,
            })
            .collect())
    }

    /// Point a name at a published environment, checked against its version.
    ///
    /// `expect_version` is the version the caller saw; `None` asserts that the
    /// name does not exist yet. The table is re-read under the exclusive lease,
    /// so a switch built on a stale view is refused instead of overwriting.
    pub fn set_name(
        &self,
        name: &str,
        id: &str,
        expect_version: Option<u64>,
    ) -> Result<NameSwitch, EnvironmentError> {
        validate_name(name)?;
        validate_id(id)?;
        let Some((envs, _lease)) = self.locked(true)? else {
            return Err(EnvironmentError::Unknown(id.to_owned()));
        };
        require_published(&envs, id)?;
        let mut table = read_names(&envs)?;
        let current = table.names.get(name).map(|record| record.version);
        let actual = current.unwrap_or(0);
        if expect_version.unwrap_or(0) != actual {
            return Err(EnvironmentError::VersionConflict {
                name: name.to_owned(),
                expected: expect_version.unwrap_or(0),
                actual,
            });
        }
        let version = actual
            .checked_add(1)
            .ok_or(EnvironmentError::InvalidTable)?;
        table.names.insert(
            name.to_owned(),
            NameRecord {
                id: id.to_owned(),
                version,
            },
        );
        write_names(&envs, &table)?;
        Ok(NameSwitch {
            name: name.to_owned(),
            id: id.to_owned(),
            version,
        })
    }

    /// Remove a name, checked against its version.
    ///
    /// This only detaches the name; deleting the version's content is a separate
    /// action with its own reference check, and is not implemented here. Like
    /// [`EnvironmentStore::set_name`], `expect_version` is an assertion: `None`
    /// asserts the name does not exist, so a caller that never observed a version
    /// cannot silently clobber one.
    pub fn remove_name(
        &self,
        name: &str,
        expect_version: Option<u64>,
    ) -> Result<NameEntry, EnvironmentError> {
        validate_name(name)?;
        let Some((envs, _lease)) = self.locked(true)? else {
            return Err(EnvironmentError::UnknownName(name.to_owned()));
        };
        let mut table = read_names(&envs)?;
        let record = table
            .names
            .get(name)
            .cloned()
            .ok_or_else(|| EnvironmentError::UnknownName(name.to_owned()))?;
        if expect_version.unwrap_or(0) != record.version {
            return Err(EnvironmentError::VersionConflict {
                name: name.to_owned(),
                expected: expect_version.unwrap_or(0),
                actual: record.version,
            });
        }
        table.names.remove(name);
        write_names(&envs, &table)?;
        Ok(NameEntry {
            name: name.to_owned(),
            id: record.id,
            version: record.version,
        })
    }

    /// Re-hash every published environment and check every name target.
    ///
    /// Returns one message per problem; an empty list means the store is
    /// consistent with its own identities and that published content is still
    /// read-only.
    pub fn verify(&self) -> Result<Vec<String>, EnvironmentError> {
        let Some((envs, _lease)) = self.locked(false)? else {
            return Ok(Vec::new());
        };
        let mut problems = Vec::new();
        // Anything that is not a published digest, the lock or the name table is
        // a leftover, not content: a crash between staging and publication must
        // be visible rather than silently ignored.
        for entry in fs::read_dir(&envs)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == LOCK || name == NAMES {
                continue;
            }
            if validate_id(&name).is_err() || !entry.file_type()?.is_dir() {
                problems.push(format!("{name}: unexpected entry in the environment store"));
            }
        }
        for directory in published_directories(&envs)? {
            let id = file_name(&directory);
            match digest_tree(&directory) {
                Ok((digest, _)) if digest == id => {}
                Ok(_) => problems.push(format!("{id}: content does not match its digest")),
                Err(error) => problems.push(format!("{id}: {error}")),
            }
            if let Ok(measure) = measure(&directory)
                && measure.write_bits
            {
                problems.push(format!("{id}: published content is still writable"));
            }
        }
        let table = read_names(&envs)?;
        for (name, record) in &table.names {
            if validate_id(&record.id).is_err() {
                problems.push(format!("{name}: name target is not an environment digest"));
            } else if !envs.join(&record.id).is_dir() {
                problems.push(format!(
                    "{name}: points at missing environment {}",
                    record.id
                ));
            }
        }
        Ok(problems)
    }
}

fn validate_name(name: &str) -> Result<(), EnvironmentError> {
    if !anchor_runtime::graph::is_environment_name(name) {
        return Err(EnvironmentError::InvalidName);
    }
    Ok(())
}

fn validate_id(id: &str) -> Result<(), EnvironmentError> {
    if id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(EnvironmentError::InvalidId)
    }
}

fn require_published(envs: &Path, id: &str) -> Result<(), EnvironmentError> {
    if !envs.join(id).is_dir() {
        return Err(EnvironmentError::Unknown(id.to_owned()));
    }
    Ok(())
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Published environments are exactly the 64-hex directories under `envs/`.
fn published_directories(envs: &Path) -> Result<Vec<PathBuf>, EnvironmentError> {
    let mut published = Vec::new();
    for entry in fs::read_dir(envs)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if validate_id(&name).is_err() || !entry.file_type()?.is_dir() {
            continue;
        }
        published.push(entry.path());
    }
    published.sort();
    Ok(published)
}

/// A path relative to the published tree root, kept as raw bytes: two entry
/// names that differ only in invalid UTF-8 must not collapse into one digest.
#[derive(Debug)]
struct RelativePath(Vec<u8>);

impl RelativePath {
    fn root() -> Self {
        Self(Vec::new())
    }

    fn join(&self, name: &OsStr) -> Self {
        let mut bytes = self.0.clone();
        if !bytes.is_empty() {
            bytes.push(b'/');
        }
        bytes.extend_from_slice(name.as_bytes());
        Self(bytes)
    }
}

/// The mode a published entry will actually carry.
///
/// Publication always clears write bits, so they are not part of an
/// environment's identity; executable and read bits are.
fn published_mode(mode: u32) -> u32 {
    mode & 0o777 & !0o222
}

fn hash_entry(hash: &mut Sha256, kind: u8, relative: &RelativePath, mode: u32) {
    hash.update([kind]);
    hash.update((relative.0.len() as u64).to_be_bytes());
    hash.update(&relative.0);
    hash.update(mode.to_be_bytes());
}

/// Raw name ordering, so the digest does not depend on locale or encoding.
fn sorted_entries(directory: &Path) -> Result<Vec<(OsString, PathBuf)>, EnvironmentError> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        entries.push((entry.file_name(), entry.path()));
    }
    entries.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));
    Ok(entries)
}

/// Open a candidate file without following a final symlink.
fn open_nofollow(path: &Path) -> Result<File, EnvironmentError> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(EnvironmentError::UnsafePath);
    }
    Ok(file)
}

/// Copy a prepared tree into `destination` and return its digest.
fn copy_and_hash(source: &Path, destination: &Path) -> Result<(String, Totals), EnvironmentError> {
    let metadata = fs::symlink_metadata(source)?;
    if !metadata.is_dir() {
        return Err(EnvironmentError::UnsafePath);
    }
    let mut hash = Sha256::new();
    let mut totals = Totals::default();
    copy_directory(
        source,
        destination,
        &RelativePath::root(),
        &mut hash,
        &mut totals,
    )?;
    if totals.files == 0 {
        return Err(EnvironmentError::EmptyContent);
    }
    Ok((format!("{:x}", hash.finalize()), totals))
}

fn copy_directory(
    source: &Path,
    destination: &Path,
    relative: &RelativePath,
    hash: &mut Sha256,
    totals: &mut Totals,
) -> Result<(), EnvironmentError> {
    for (name, path) in sorted_entries(source)? {
        let metadata = fs::symlink_metadata(&path)?;
        let child = relative.join(&name);
        let target = destination.join(&name);
        if metadata.is_dir() {
            fs::create_dir(&target)?;
            totals.directories += 1;
            hash_entry(hash, b'd', &child, published_mode(metadata.mode()));
            copy_directory(&path, &target, &child, hash, totals)?;
            fs::set_permissions(&target, metadata.permissions())?;
            continue;
        }
        if !metadata.is_file() {
            return Err(EnvironmentError::UnsafePath);
        }
        let mut opened = open_nofollow(&path)?;
        let mut contents = Vec::new();
        opened.read_to_end(&mut contents)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)?;
        file.write_all(&contents)?;
        file.sync_all()?;
        fs::set_permissions(&target, metadata.permissions())?;
        hash_entry(hash, b'f', &child, published_mode(metadata.mode()));
        hash.update(Sha256::digest(&contents));
        totals.bytes += contents.len() as u64;
        totals.files += 1;
    }
    Ok(())
}

/// Re-hash a published tree with exactly the rules publication used.
fn digest_tree(root: &Path) -> Result<(String, Totals), EnvironmentError> {
    let mut hash = Sha256::new();
    let mut totals = Totals::default();
    hash_directory(root, &RelativePath::root(), &mut hash, &mut totals)?;
    Ok((format!("{:x}", hash.finalize()), totals))
}

fn hash_directory(
    directory: &Path,
    relative: &RelativePath,
    hash: &mut Sha256,
    totals: &mut Totals,
) -> Result<(), EnvironmentError> {
    for (name, path) in sorted_entries(directory)? {
        let metadata = fs::symlink_metadata(&path)?;
        let child = relative.join(&name);
        if metadata.is_dir() {
            totals.directories += 1;
            hash_entry(hash, b'd', &child, published_mode(metadata.mode()));
            hash_directory(&path, &child, hash, totals)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(EnvironmentError::UnsafePath);
        }
        let mut opened = open_nofollow(&path)?;
        let mut contents = Vec::new();
        opened.read_to_end(&mut contents)?;
        hash_entry(hash, b'f', &child, published_mode(metadata.mode()));
        hash.update(Sha256::digest(&contents));
        totals.bytes += contents.len() as u64;
        totals.files += 1;
    }
    Ok(())
}

/// Listing totals, plus whether anything in the tree is still writable.
fn measure(root: &Path) -> Result<Measure, EnvironmentError> {
    let mut totals = Totals::default();
    let mut write_bits = root.metadata()?.mode() & 0o222 != 0;
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for (_, path) in sorted_entries(&directory)? {
            // No-follow, so a listing can never walk outside the published tree.
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.is_dir() {
                totals.directories += 1;
                write_bits |= metadata.mode() & 0o222 != 0;
                stack.push(path);
            } else if metadata.is_file() {
                totals.bytes += metadata.len();
                totals.files += 1;
                write_bits |= metadata.mode() & 0o222 != 0;
            } else {
                return Err(EnvironmentError::UnsafePath);
            }
        }
    }
    Ok(Measure { totals, write_bits })
}

/// Remove every write bit from the tree, so the content is not modified in place.
///
/// This is a signal, not a security boundary: the same owner can restore the
/// bits. Deployment hardening (a separate owner or a read-only mount) is what
/// makes it enforceable.
fn make_read_only(root: &Path) -> Result<(), EnvironmentError> {
    let mut stack = vec![root.to_path_buf()];
    let mut directories = Vec::new();
    while let Some(directory) = stack.pop() {
        directories.push(directory.clone());
        for (_, path) in sorted_entries(&directory)? {
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.is_dir() {
                stack.push(path);
            } else if metadata.is_file() {
                let mut permissions = metadata.permissions();
                permissions.set_mode(metadata.mode() & !0o222);
                fs::set_permissions(&path, permissions)?;
            }
        }
    }
    // Parents last, so traversal still works while the tree is being walked.
    for directory in directories.into_iter().rev() {
        let metadata = fs::symlink_metadata(&directory)?;
        let mut permissions = metadata.permissions();
        permissions.set_mode(metadata.mode() & !0o222);
        fs::set_permissions(&directory, permissions)?;
    }
    Ok(())
}

/// fsync every file and directory of a published tree, bottom-up.
fn sync_tree(path: &Path) -> Result<(), EnvironmentError> {
    for (_, entry) in sorted_entries(path)? {
        let metadata = fs::symlink_metadata(&entry)?;
        if metadata.is_dir() {
            sync_tree(&entry)?;
        } else if metadata.is_file() {
            File::open(&entry)?.sync_all()?;
        } else {
            return Err(EnvironmentError::UnsafePath);
        }
    }
    File::open(path)?.sync_all()?;
    Ok(())
}

fn names_path(envs: &Path) -> PathBuf {
    envs.join(NAMES)
}

fn read_names(envs: &Path) -> Result<NameFile, EnvironmentError> {
    match fs::read(names_path(envs)) {
        Ok(bytes) => {
            // `deny_unknown_fields` plus id validation: a table this version
            // cannot fully represent is refused instead of being silently
            // truncated by the next write.
            let table: NameFile =
                serde_json::from_slice(&bytes).map_err(|_| EnvironmentError::InvalidTable)?;
            for record in table.names.values() {
                validate_id(&record.id).map_err(|_| EnvironmentError::InvalidTable)?;
            }
            Ok(table)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(NameFile::default()),
        Err(error) => Err(error.into()),
    }
}

/// Replace the table atomically: a reader never sees a half-written table.
fn write_names(envs: &Path, table: &NameFile) -> Result<(), EnvironmentError> {
    let mut staged = tempfile::Builder::new()
        .prefix(".env-names-")
        .tempfile_in(envs)?;
    staged.write_all(
        &serde_json::to_vec_pretty(table).map_err(|_| EnvironmentError::InvalidTable)?,
    )?;
    staged.as_file().sync_all()?;
    staged
        .persist(names_path(envs))
        .map_err(|error| EnvironmentError::Io(error.error))?;
    File::open(envs)?.sync_all()?;
    Ok(())
}
