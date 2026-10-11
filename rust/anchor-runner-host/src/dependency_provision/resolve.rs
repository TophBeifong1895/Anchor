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
    DependencyProvisionConfig, PackageSource, ProvisionError, sha256_file, sha256_hex,
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

/// Whether a recorded interpreter constraint is a plain version prefix.
///
/// The deployment builds on one interpreter, so the field names the version it
/// must be — `3`, `3.12`, `3.12.13` — not a PEP 440 specifier. Accepting
/// `>=3.11` here and rejecting it later would record a requirement that can
/// never be provided.
///
/// `uv`'s `--index` *adds to* its default index rather than replacing it, so a
/// first authorized index given as `--index` leaves PyPI reachable: a package
/// missing from the authorized source is then resolved against an index the
/// deployment never authorized, and the digests in the plan the human approves
/// come from there. Measured with uv 0.11.6: `--index` against an index that
/// answers 404 produced a lock with PyPI's digest for the package, while
/// `--default-index` refused instead. The first index therefore *replaces* the
/// default, the rest extend it, and directories stay `--find-links`.
fn source_arguments(sources: &[PackageSource]) -> Result<Vec<String>, ProvisionError> {
    let mut arguments = Vec::new();
    let mut indexes = 0usize;
    for source in sources {
        match source {
            PackageSource::Directory(path) => {
                if !path.is_dir() {
                    return Err(ProvisionError::Failed(format!(
                        "the authorized package directory {} does not exist",
                        path.display()
                    )));
                }
                arguments.push("--find-links".to_owned());
                arguments.push(path.to_string_lossy().into_owned());
            }
            PackageSource::Index(url) => {
                indexes += 1;
                // The first index replaces the implicit default; nothing may be
                // reachable that the deployment did not write down.
                arguments.push(
                    if indexes == 1 {
                        "--default-index"
                    } else {
                        "--index"
                    }
                    .to_owned(),
                );
                arguments.push(url.clone());
            }
        }
    }
    if indexes == 0 {
        // With only directories authorized, an implicit PyPI lookup would be a
        // network call the operator never allowed.
        arguments.push("--no-index".to_owned());
    }
    Ok(arguments)
}

/// Exactly what the resolver process may see.
///
/// A source set is only a boundary if nothing else can widen it: `UV_INDEX`,
/// `UV_DEFAULT_INDEX`, `PIP_INDEX_URL`, a proxy, a TLS opt-in or `UV_OFFLINE`
/// inherited from the host would each change which indexes are consulted. So the
/// child gets this list and nothing else.
pub(crate) fn resolver_environment(cache: &Path, build_root: &Path) -> Vec<(String, String)> {
    vec![
        ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
        ("HOME".to_owned(), build_root.to_string_lossy().into_owned()),
        (
            "UV_CACHE_DIR".to_owned(),
            cache.to_string_lossy().into_owned(),
        ),
        ("UV_PYTHON_DOWNLOADS".to_owned(), "never".to_owned()),
    ]
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
    config: &DependencyProvisionConfig,
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
    for argument in source_arguments(&config.sources)? {
        command.arg(argument);
    }
    let cache = config.build_root.join("cache");
    std::fs::create_dir_all(&cache).map_err(|error| {
        ProvisionError::Failed(format!(
            "the uv cache {} is unavailable: {error}",
            cache.display()
        ))
    })?;
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
        // Nothing from the host environment may reach the resolver: the source
        // set is whatever the deployment wrote down. The working directory is
        // the staging directory, so no `uv.toml` or `pyproject.toml` above it
        // can add sources either.
        .env_clear()
        .envs(resolver_environment(&cache, &config.build_root))
        .current_dir(staging.path())
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
    config: &DependencyProvisionConfig,
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
            // The probe must see what a Run would see: `PYTHONPATH`,
            // `PYTHONHOME` or a startup hook would change the version and prefix
            // this plan records as a fact.
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
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

#[cfg(test)]
fn verify_interpreter(resolved: &ResolvedRequirements) -> Result<(), ProvisionError> {
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

#[cfg(test)]
fn is_interpreter_constraint(value: &str) -> bool {
    let value = value.trim();
    let parts = value.split('.').collect::<Vec<_>>();
    !value.is_empty()
        && value.len() <= 16
        && parts.len() <= 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
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
    fn an_interpreter_constraint_is_a_version_prefix_not_a_specifier() {
        for accepted in ["3", "3.12", "3.12.13", " 3.12 "] {
            assert!(is_interpreter_constraint(accepted), "{accepted}");
        }
        for refused in [
            "python>=3.11",
            ">=3.11",
            "~=3.12",
            "3.x",
            "",
            ".",
            "3.",
            "3.12.13.1",
        ] {
            assert!(!is_interpreter_constraint(refused), "{refused}");
        }
    }

    #[test]
    fn the_first_authorized_index_replaces_the_default_one() {
        // A directory source must exist to be named, so the case uses a real one.
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().to_path_buf();
        let first = PackageSource::Index("https://mirror.internal/simple".to_owned());
        let second = PackageSource::Index("https://mirror.other/simple".to_owned());

        // One index: it must *replace* uv's default, or a package missing from
        // it would be resolved against PyPI, which nobody authorized.
        let arguments = source_arguments(std::slice::from_ref(&first)).unwrap();
        assert_eq!(
            arguments,
            vec![
                "--default-index".to_owned(),
                "https://mirror.internal/simple".to_owned()
            ]
        );
        assert!(
            !arguments.iter().any(|argument| argument == "--no-index"),
            "an authorized index means an index is allowed"
        );

        // More than one: the first replaces, the rest extend.
        let arguments = source_arguments(&[first.clone(), second.clone()]).unwrap();
        assert_eq!(arguments[0], "--default-index");
        assert_eq!(arguments[2], "--index");
        assert_eq!(arguments[3], "https://mirror.other/simple");

        // Directories only: no index at all, implicit or otherwise.
        let arguments = source_arguments(&[PackageSource::Directory(directory)]).unwrap();
        assert!(
            arguments.contains(&"--no-index".to_owned()),
            "{arguments:?}"
        );
        assert!(
            !arguments
                .iter()
                .any(|argument| argument == "--default-index" || argument == "--index"),
            "a directory-only deployment must not consult any index: {arguments:?}"
        );
    }

    #[test]
    fn the_resolver_sees_only_the_deployment_and_the_staging_directory() {
        let environment = resolver_environment(Path::new("/build/cache"), Path::new("/build"));
        let mut names = environment
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>();
        names.sort_unstable();
        assert_eq!(
            names,
            vec!["HOME", "PATH", "UV_CACHE_DIR", "UV_PYTHON_DOWNLOADS"],
            "anything else could widen the authorized source set"
        );
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
