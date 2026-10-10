//! Turning an ask into one locked, hash-pinned set.
//!
//! Resolution is the trusted half of preparation: the deployment's own `uv`
//! runs against the deployment's authorized sources, and what comes back is a
//! `requirements.txt` whose every artifact is pinned by SHA-256. That text *is*
//! the plan's identity — the human approves it, the build installs exactly it,
//! and any later difference is a different plan.

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::config::{
    EnvironmentProvisionConfig, PackageSource, ProvisionError, sha256_file, sha256_hex,
};

/// One distribution in the locked set.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LockedPackage {
    pub(crate) name: String,
    pub(crate) version: String,
    /// Every artifact digest the lock accepts for this distribution. The fetch
    /// step accepts an artifact whose bytes hash to one of them, and the install
    /// is run with `--require-hashes`, so the lock — not the index — decides.
    pub(crate) hashes: Vec<String>,
}

/// A resolved ask: the interpreter it will be built on and the locked set.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResolvedRequirements {
    /// The observed interpreter version, e.g. `3.12.13`.
    pub(crate) interpreter: String,
    /// The interpreter binary as the plan will run it.
    pub(crate) interpreter_path: PathBuf,
    /// The interpreter's installation prefix. The published environment runs on
    /// this deployment interpreter, so the plan records where it came from; a
    /// prefix the sandbox does not mount is refused rather than published into
    /// an environment that only works at build time.
    pub(crate) interpreter_prefix: PathBuf,
    /// SHA-256 of the base interpreter binary, so the plan names a build, not a
    /// version range.
    pub(crate) interpreter_digest: String,
    /// The exact `requirements.txt` the build will install.
    pub(crate) lock: String,
    /// SHA-256 of `lock`, which is also the plan's identity.
    pub(crate) lock_digest: String,
    pub(crate) packages: Vec<LockedPackage>,
}

/// Confirm the interpreter is still the build the plan recorded.
///
/// The digest is taken while resolving and written into the plan; a binary that
/// changed in between would make the plan's own fact false, and the published
/// content would carry a different interpreter than the human approved.
pub(crate) fn verify_interpreter(resolved: &ResolvedRequirements) -> Result<(), ProvisionError> {
    let observed = sha256_file(&resolved.interpreter_path).map_err(|error| {
        ProvisionError::Failed(format!("the planned interpreter cannot be read: {error}"))
    })?;
    if observed != resolved.interpreter_digest {
        return Err(ProvisionError::Failed(format!(
            "the planned interpreter changed since it was resolved (approved {}, found {observed})",
            resolved.interpreter_digest
        )));
    }
    Ok(())
}

/// The major and minor release of a reported interpreter version.
fn release(version: &str) -> (u32, u32) {
    let mut parts = version
        .split('.')
        .map(|part| part.parse::<u32>().unwrap_or(0));
    (parts.next().unwrap_or(0), parts.next().unwrap_or(0))
}

/// Directories the host's execution sandbox mounts for every invocation. A
/// published environment's base interpreter must come from one of them.
const SANDBOX_VISIBLE_ROOTS: &[&str] = &["/usr", "/bin", "/lib", "/lib64", "/sbin"];

/// Resolve an ask into one locked set, using only authorized sources.
pub(crate) async fn resolve_requirements(
    config: &EnvironmentProvisionConfig,
    constraint: Option<&str>,
    packages: &[String],
) -> Result<ResolvedRequirements, ProvisionError> {
    if packages.is_empty() {
        return Err(ProvisionError::Invalid(
            "the requirement names no packages, so there is nothing to resolve".to_owned(),
        ));
    }
    for package in packages {
        if !valid_package_spec(package) {
            return Err(ProvisionError::Invalid(format!(
                "`{package}` is not a plain package requirement (name, optionally with a version specifier)"
            )));
        }
    }
    std::fs::create_dir_all(&config.build_root).map_err(|error| {
        ProvisionError::Failed(format!(
            "the build root {} is not writable: {error}",
            config.build_root.display()
        ))
    })?;

    let (interpreter, interpreter_path, interpreter_prefix, interpreter_digest) =
        observed_interpreter(config, constraint).await?;
    let uv = config.verified_uv()?;

    // The lock must not depend on where the ask happened to be resolved, or two
    // identical asks would get two identities.
    let staging = tempfile::Builder::new()
        .prefix("resolve-")
        .tempdir_in(&config.build_root)
        .map_err(|error| ProvisionError::Failed(format!("resolution staging failed: {error}")))?;
    let input = staging.path().join("requirements.in");
    let output = staging.path().join("requirements.txt");
    std::fs::write(&input, format!("{}\n", packages.join("\n"))).map_err(|error| {
        ProvisionError::Failed(format!("the ask could not be written: {error}"))
    })?;

    let major_minor = interpreter.split('.').take(2).collect::<Vec<_>>().join(".");
    let mut command = tokio::process::Command::new(&uv);
    command.arg("pip").arg("compile");
    let mut indexes = 0usize;
    for source in &config.sources {
        match source {
            PackageSource::Directory(path) => {
                if !path.is_dir() {
                    return Err(ProvisionError::Failed(format!(
                        "the authorized package directory {} does not exist",
                        path.display()
                    )));
                }
                command.arg("--find-links").arg(path);
            }
            PackageSource::Index(url) => {
                indexes += 1;
                command.arg("--index").arg(url);
            }
        }
    }
    if indexes == 0 {
        // With only directories authorized, an implicit PyPI lookup would be a
        // network call the operator never allowed.
        command.arg("--no-index");
    }
    command
        .arg("--generate-hashes")
        .arg("--only-binary")
        .arg(":all:")
        .arg("--no-header")
        .arg("--no-annotate")
        .arg("--python-version")
        .arg(&major_minor)
        .arg("--output-file")
        .arg(&output)
        .arg(&input)
        .env("UV_CACHE_DIR", config.build_root.join("cache"))
        .env("UV_PYTHON_DOWNLOADS", "never")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let result = tokio::time::timeout(config.step_timeout(), command.output())
        .await
        .map_err(|_| ProvisionError::Failed("resolving dependencies timed out".to_owned()))?
        .map_err(|error| ProvisionError::Failed(format!("uv could not be run: {error}")))?;
    if !result.status.success() {
        return Err(ProvisionError::Failed(format!(
            "dependency resolution failed: {}",
            tail(&String::from_utf8_lossy(&result.stderr))
        )));
    }
    let lock = std::fs::read_to_string(&output).map_err(|error| {
        ProvisionError::Failed(format!("resolution wrote no readable lock: {error}"))
    })?;
    let packages = parse_lock(&lock)?;
    if packages.is_empty() {
        return Err(ProvisionError::Failed(
            "resolution produced an empty locked set".to_owned(),
        ));
    }
    Ok(ResolvedRequirements {
        interpreter,
        interpreter_path,
        interpreter_prefix,
        interpreter_digest,
        lock_digest: sha256_hex(lock.as_bytes()),
        lock,
        packages,
    })
}

/// The base interpreter the plan will name: its version, its binary, the prefix
/// it needs to run, and the digest that identifies the build.
async fn observed_interpreter(
    config: &EnvironmentProvisionConfig,
    constraint: Option<&str>,
) -> Result<(String, PathBuf, PathBuf, String), ProvisionError> {
    let python = std::fs::canonicalize(&config.python).map_err(|error| {
        ProvisionError::Failed(format!(
            "the configured interpreter {} is unavailable: {error}",
            config.python.display()
        ))
    })?;
    let digest = sha256_file(&python).map_err(|error| {
        ProvisionError::Failed(format!("the interpreter cannot be read: {error}"))
    })?;
    let output = tokio::time::timeout(
        Duration::from_secs(30),
        tokio::process::Command::new(&python)
            .arg("-c")
            .arg("import sys; print('%d.%d.%d' % sys.version_info[:3]); print(sys.base_prefix)")
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| ProvisionError::Failed("the interpreter did not answer in time".to_owned()))?
    .map_err(|error| {
        ProvisionError::Failed(format!("the interpreter could not be run: {error}"))
    })?;
    if !output.status.success() {
        return Err(ProvisionError::Failed(format!(
            "the configured interpreter reported no version: {}",
            tail(&String::from_utf8_lossy(&output.stderr))
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    let version = lines.next().unwrap_or_default().to_owned();
    let prefix = lines.next().unwrap_or_default();
    if version.is_empty() || !version.split('.').all(|part| part.parse::<u32>().is_ok()) {
        return Err(ProvisionError::Failed(format!(
            "the configured interpreter reported an unusable version `{version}`"
        )));
    }
    let prefix = std::fs::canonicalize(prefix).map_err(|error| {
        ProvisionError::Failed(format!(
            "the interpreter reported an unusable prefix `{prefix}`: {error}"
        ))
    })?;
    // A Run's sandbox mounts these roots for every invocation. If the base
    // interpreter lives outside them, the environment would build here and then
    // fail to run where it was published — so refuse now, while a human can
    // still change the deployment.
    if !SANDBOX_VISIBLE_ROOTS
        .iter()
        .any(|root| prefix.starts_with(root))
    {
        return Err(ProvisionError::Unconfigured(format!(
            "the configured interpreter runs from {}, which a Run's sandbox does not mount; put the base interpreter under one of {} so the published environment can run",
            prefix.display(),
            SANDBOX_VISIBLE_ROOTS.join(", ")
        )));
    }
    if release(&version) < (3, 8) {
        // `importlib.metadata` is how the published environment reports what it
        // holds, and it only exists from 3.8 on. Refusing here keeps the plan
        // honest instead of publishing an environment this host cannot verify.
        return Err(ProvisionError::Unconfigured(format!(
            "this deployment builds environments on Python {version}; the builder needs 3.8 or newer"
        )));
    }
    if let Some(constraint) = constraint.map(str::trim).filter(|value| !value.is_empty())
        && (!version.starts_with(constraint)
            || version
                .chars()
                .nth(constraint.len())
                .is_some_and(|next| next != '.'))
    {
        return Err(ProvisionError::Invalid(format!(
            "this deployment builds environments on Python {version}, which does not satisfy the requested `{constraint}`"
        )));
    }
    Ok((version, python, prefix, digest))
}

/// A plan entry is a project name with at most version specifiers — never an
/// option, a path, a URL or an environment marker. This text is written into a
/// `requirements.in` that `uv` parses as options and requirements alike, so a
/// leading `-` or an embedded `/` would be a way to ask for something the
/// deployment never authorized.
pub(crate) fn valid_package_spec(spec: &str) -> bool {
    if spec.is_empty() || spec.len() > 128 {
        return false;
    }
    let mut name_end = spec.len();
    for (index, character) in spec.char_indices() {
        if !(character.is_ascii_alphanumeric()
            || character == '.'
            || character == '_'
            || character == '-')
        {
            name_end = index;
            break;
        }
    }
    let (name, rest) = spec.split_at(name_end);
    if name.is_empty()
        || !name
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric())
        || name.ends_with(['.', '-', '_'])
    {
        return false;
    }
    for specifier in rest.split(',').filter(|entry| !entry.is_empty()) {
        if !valid_version_specifier(specifier) {
            return false;
        }
    }
    // Nothing after the name is fine; a bare comma is not.
    rest.is_empty() || !rest.ends_with(',')
}

fn valid_version_specifier(specifier: &str) -> bool {
    let operator = ["===", "==", "!=", "<=", ">=", "~=", "<", ">"]
        .into_iter()
        .find(|operator| specifier.starts_with(operator));
    let Some(operator) = operator else {
        return false;
    };
    let version = &specifier[operator.len()..];
    !version.is_empty()
        && version
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || ".*+!_-".contains(character))
}

/// Read back the locked set `uv pip compile` wrote.
///
/// Continuation lines are joined first, because a requirement with several
/// accepted artifacts is several physical lines.
pub(crate) fn parse_lock(lock: &str) -> Result<Vec<LockedPackage>, ProvisionError> {
    let mut logical: Vec<String> = Vec::new();
    let mut current = String::new();
    for line in lock.lines() {
        let line = line.trim_end();
        if let Some(head) = line.strip_suffix('\\') {
            current.push_str(head);
            continue;
        }
        current.push_str(line);
        if !current.trim().is_empty() {
            logical.push(std::mem::take(&mut current));
        }
        current.clear();
    }
    if !current.trim().is_empty() {
        logical.push(current);
    }

    let mut packages = Vec::new();
    for line in logical {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut tokens = line.split_whitespace();
        let Some(requirement) = tokens.next() else {
            continue;
        };
        let Some((name, version)) = requirement.split_once("==") else {
            return Err(ProvisionError::Failed(format!(
                "the resolved lock contains an unpinned requirement `{requirement}`"
            )));
        };
        if name.is_empty() || version.is_empty() {
            return Err(ProvisionError::Failed(format!(
                "the resolved lock contains an unusable requirement `{requirement}`"
            )));
        }
        let mut hashes = Vec::new();
        for token in tokens {
            let Some(digest) = token.strip_prefix("--hash=sha256:") else {
                return Err(ProvisionError::Failed(format!(
                    "the resolved lock contains an unusable hash `{token}`"
                )));
            };
            if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(ProvisionError::Failed(format!(
                    "the resolved lock contains a malformed digest `{digest}`"
                )));
            }
            hashes.push(digest.to_ascii_lowercase());
        }
        if hashes.is_empty() {
            return Err(ProvisionError::Failed(format!(
                "the resolved lock pins `{requirement}` without a hash"
            )));
        }
        hashes.sort();
        hashes.dedup();
        packages.push(LockedPackage {
            name: name.to_owned(),
            version: version.to_owned(),
            hashes,
        });
    }
    Ok(packages)
}

/// The last few lines of a tool's output, which is where the reason is.
pub(crate) fn tail(output: &str) -> String {
    let lines: Vec<&str> = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let start = lines.len().saturating_sub(6);
    lines[start..].join("; ")
}

/// Where the lock file lives inside a staging directory.
pub(crate) fn lock_path(input_root: &Path) -> PathBuf {
    input_root.join("requirements.txt")
}

#[cfg(test)]
mod resolve_tests {
    use super::*;

    #[test]
    fn a_requirement_entry_cannot_carry_an_option_a_path_or_a_url() {
        assert!(valid_package_spec("requests"));
        assert!(valid_package_spec("requests==2.32.3"));
        assert!(valid_package_spec("uvicorn>=0.30,<1"));
        assert!(valid_package_spec("numpy==2.*"));
        // Anything that would be parsed as an option, a URL or a path.
        assert!(!valid_package_spec("-r other.txt"));
        assert!(!valid_package_spec("--index-url https://example.com"));
        assert!(!valid_package_spec(".."));
        assert!(!valid_package_spec("../escape"));
        assert!(!valid_package_spec("/etc/passwd"));
        assert!(!valid_package_spec("pkg @ https://example.com/pkg.whl"));
        assert!(!valid_package_spec("pkg; python_version<'3.10'"));
        assert!(!valid_package_spec("pkg=="));
        assert!(!valid_package_spec(""));
    }

    #[test]
    fn a_lock_keeps_every_artifact_the_human_approved() {
        let lock = "\
requests==2.32.3 \\
    --hash=sha256:1111111111111111111111111111111111111111111111111111111111111111 \\
    --hash=sha256:2222222222222222222222222222222222222222222222222222222222222222
idna==3.7 \\
    --hash=sha256:3333333333333333333333333333333333333333333333333333333333333333
";
        let packages = parse_lock(lock).unwrap();
        assert_eq!(packages.len(), 2);
        assert_eq!(packages[0].name, "requests");
        assert_eq!(packages[0].version, "2.32.3");
        assert_eq!(packages[0].hashes.len(), 2);
        assert_eq!(packages[1].name, "idna");
    }

    #[test]
    fn an_interpreter_that_changed_after_resolution_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let binary = root.path().join("python3");
        std::fs::write(&binary, b"first build").unwrap();
        let mut resolved = ResolvedRequirements {
            interpreter: "3.12.13".into(),
            interpreter_path: binary.clone(),
            interpreter_prefix: root.path().to_path_buf(),
            interpreter_digest: sha256_file(&binary).unwrap(),
            lock: String::new(),
            lock_digest: String::new(),
            packages: Vec::new(),
        };
        assert!(verify_interpreter(&resolved).is_ok());
        std::fs::write(&binary, b"another build").unwrap();
        let error = verify_interpreter(&resolved).unwrap_err();
        assert!(
            matches!(error, ProvisionError::Failed(ref reason) if reason.contains("changed since it was resolved")),
            "{error:?}"
        );
        // Repairing the record is not a way to make it true again for the
        // approved build: the digest is what was approved.
        resolved.interpreter_digest = sha256_file(&binary).unwrap();
        assert!(verify_interpreter(&resolved).is_ok());
    }

    #[test]
    fn an_unpinned_or_unhashed_lock_is_refused_rather_than_built() {
        let error = parse_lock("requests>=2.0\n").unwrap_err();
        assert!(matches!(error, ProvisionError::Failed(_)), "{error:?}");
        let error = parse_lock("requests==2.32.3\n").unwrap_err();
        assert!(
            matches!(error, ProvisionError::Failed(ref reason) if reason.contains("without a hash")),
            "{error:?}"
        );
        let error = parse_lock("requests==2.32.3 --hash=sha256:abc\n").unwrap_err();
        assert!(
            matches!(error, ProvisionError::Failed(ref reason) if reason.contains("malformed")),
            "{error:?}"
        );
    }
}
