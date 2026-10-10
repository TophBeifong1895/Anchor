//! What the deployment authorizes for environment preparation.
//!
//! Every value here comes from the deployment's own environment, never from a
//! model argument, a Graph field or an HTTP request. Resolution and fetching
//! reach the network, so the sources are an allowlist the operator wrote down;
//! the `uv` that performs them is pinned by digest, which also keeps the
//! bootstrap from depending on itself.

use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

/// Why preparation could not proceed.
///
/// One variant per *reason*, because the boundaries that surface it map it
/// differently: an unconfigured or invalid ask is the caller's problem, a
/// failed step and a moved plan are the operator's.
#[derive(Debug)]
pub(crate) enum ProvisionError {
    /// This deployment does not authorize environment preparation at all.
    Unconfigured(String),
    /// The ask or the plan itself is not acceptable.
    Invalid(String),
    /// A step (resolve, fetch, build, verify) failed.
    Failed(String),
}

impl std::error::Error for ProvisionError {}

impl std::fmt::Display for ProvisionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unconfigured(reason) | Self::Invalid(reason) | Self::Failed(reason) => {
                formatter.write_str(reason)
            }
        }
    }
}

/// One deployment-authorized place packages may come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PackageSource {
    /// A directory of already-built distributions: an offline deployment, or a
    /// test. Used as `--find-links`.
    Directory(PathBuf),
    /// A PEP 503/691 simple index reached over HTTP(S). Used as `--index`.
    Index(String),
}

impl PackageSource {
    /// The text a plan records, so a plan says what it resolved against.
    pub(crate) fn as_str(&self) -> String {
        match self {
            Self::Directory(path) => format!("directory:{}", path.display()),
            Self::Index(url) => format!("index:{url}"),
        }
    }
}

/// How long one build step may take. Installing a real dependency set from a
/// cold cache is not fast, and a premature timeout would leave a half-built
/// staging directory behind rather than a wrong environment.
const STEP_TIMEOUT: Duration = Duration::from_secs(600);
/// How long a single index or artifact request may take.
const HTTP_TIMEOUT: Duration = Duration::from_secs(120);
/// Output retained from one build step; the interesting part is the error tail.
const MAX_STEP_OUTPUT: usize = 256 * 1024;
/// Largest single artifact this host will download.
const MAX_ARTIFACT_BYTES: u64 = 512 * 1024 * 1024;

/// The deployment's authorized preparation.
#[derive(Debug, Clone)]
pub(crate) struct EnvironmentProvisionConfig {
    /// The pinned `uv` executable.
    pub(crate) uv: PathBuf,
    /// Its expected SHA-256, checked before every use.
    pub(crate) uv_digest: String,
    /// The base interpreter the environment is built on. Its identity is part
    /// of the plan, because a wide "Python 3.12" is not a build identity.
    pub(crate) python: PathBuf,
    /// Authorized package sources, tried in order.
    pub(crate) sources: Vec<PackageSource>,
    /// Writable staging root. Nothing here is ever published content.
    pub(crate) build_root: PathBuf,
    /// The Bubblewrap executable, as the rest of the host resolves it.
    pub(crate) bwrap: PathBuf,
}

impl EnvironmentProvisionConfig {
    pub(crate) fn step_timeout(&self) -> Duration {
        STEP_TIMEOUT
    }

    pub(crate) fn http_timeout(&self) -> Duration {
        HTTP_TIMEOUT
    }

    pub(crate) fn max_step_output(&self) -> usize {
        MAX_STEP_OUTPUT
    }

    pub(crate) fn max_artifact_bytes(&self) -> u64 {
        MAX_ARTIFACT_BYTES
    }

    /// The `uv` binary, after checking it is still the one the deployment pinned.
    pub(crate) fn verified_uv(&self) -> Result<PathBuf, ProvisionError> {
        let observed = sha256_file(&self.uv)
            .map_err(|error| ProvisionError::Failed(format!("uv cannot be read: {error}")))?;
        if observed != self.uv_digest {
            return Err(ProvisionError::Failed(format!(
                "the configured uv binary is not the pinned one (expected {}, found {observed})",
                self.uv_digest
            )));
        }
        Ok(self.uv.clone())
    }
}

/// Read the deployment's authorized preparation, if it configured any.
///
/// All-or-nothing on purpose: a half-configured deployment would otherwise fail
/// later with a worse message, or worse, silently resolve against a default
/// index the operator never authorized. Setting none of the variables means
/// "this deployment does not prepare environments", which is not an error.
pub(crate) fn from_env(
    state_root: &Path,
    bwrap: PathBuf,
) -> Result<Option<EnvironmentProvisionConfig>, String> {
    const VARIABLES: &[&str] = &[
        "ANCHOR_ENVIRONMENT_SOURCES",
        "ANCHOR_ENVIRONMENT_UV",
        "ANCHOR_ENVIRONMENT_UV_SHA256",
        "ANCHOR_ENVIRONMENT_PYTHON",
        "ANCHOR_ENVIRONMENT_BUILD_ROOT",
    ];
    let mut values = std::collections::BTreeMap::new();
    for name in VARIABLES {
        if let Ok(value) = std::env::var(name)
            && !value.trim().is_empty()
        {
            values.insert(*name, value);
        }
    }
    if values.is_empty() {
        return Ok(None);
    }
    let required = |name: &str| -> Result<String, String> {
        values.get(name).cloned().ok_or_else(|| {
            format!("{name} is required once any ANCHOR_ENVIRONMENT_* variable is set")
        })
    };

    let sources = parse_sources(&required("ANCHOR_ENVIRONMENT_SOURCES")?)?;
    let uv = PathBuf::from(required("ANCHOR_ENVIRONMENT_UV")?);
    if !uv.is_absolute() {
        return Err("ANCHOR_ENVIRONMENT_UV must be an absolute path".into());
    }
    let uv_digest = required("ANCHOR_ENVIRONMENT_UV_SHA256")?;
    if uv_digest.len() != 64 || !uv_digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("ANCHOR_ENVIRONMENT_UV_SHA256 must be a 64-character hex digest".into());
    }
    let python = PathBuf::from(required("ANCHOR_ENVIRONMENT_PYTHON")?);
    if !python.is_absolute() {
        return Err("ANCHOR_ENVIRONMENT_PYTHON must be an absolute path".into());
    }
    let build_root = values
        .get("ANCHOR_ENVIRONMENT_BUILD_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| state_root.join("environment-build"));
    if !build_root.is_absolute() {
        return Err("ANCHOR_ENVIRONMENT_BUILD_ROOT must be an absolute path".into());
    }
    Ok(Some(EnvironmentProvisionConfig {
        uv,
        uv_digest,
        python,
        sources,
        build_root,
        bwrap,
    }))
}

fn parse_sources(value: &str) -> Result<Vec<PackageSource>, String> {
    let mut sources = Vec::new();
    for entry in value.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            return Err("ANCHOR_ENVIRONMENT_SOURCES has an empty entry".into());
        }
        if entry.starts_with("https://") || entry.starts_with("http://") {
            sources.push(PackageSource::Index(entry.trim_end_matches('/').to_owned()));
        } else {
            let path = PathBuf::from(entry);
            if !path.is_absolute() {
                return Err(format!(
                    "package source `{entry}` must be an absolute directory or an http(s) index"
                ));
            }
            sources.push(PackageSource::Directory(path));
        }
    }
    if sources.is_empty() {
        return Err("ANCHOR_ENVIRONMENT_SOURCES must name at least one source".into());
    }
    Ok(sources)
}

/// SHA-256 of one file, in the same hex form the Library uses for identities.
pub(crate) fn sha256_file(path: &Path) -> Result<String, String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// SHA-256 of a byte string.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod config_tests {
    use super::*;

    #[test]
    fn a_half_configured_deployment_is_an_error_rather_than_a_default() {
        let error = parse_sources(" ").unwrap_err();
        assert!(error.contains("empty entry"), "{error}");
        assert!(parse_sources("/tmp/a,,/tmp/b").is_err());
        let error = parse_sources("relative/dir").unwrap_err();
        assert!(error.contains("absolute directory"), "{error}");
        assert_eq!(
            parse_sources("/opt/wheels, https://pypi.org/simple/").unwrap(),
            vec![
                PackageSource::Directory(PathBuf::from("/opt/wheels")),
                PackageSource::Index("https://pypi.org/simple".to_owned()),
            ]
        );
    }

    #[test]
    fn the_recorded_source_text_distinguishes_a_directory_from_an_index() {
        assert_eq!(
            PackageSource::Directory(PathBuf::from("/opt/wheels")).as_str(),
            "directory:/opt/wheels"
        );
        assert_eq!(
            PackageSource::Index("https://pypi.org/simple".to_owned()).as_str(),
            "index:https://pypi.org/simple"
        );
    }
}
