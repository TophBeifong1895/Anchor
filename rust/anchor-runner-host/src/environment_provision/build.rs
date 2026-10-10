//! Building the environment inside Bubblewrap, with no network at all.
//!
//! The build is deliberately the *only* step that runs foreign bytes, so it runs
//! where the host, not the request, decides what is available: a writable
//! staging directory, a read-only wheelhouse, the pinned `uv`, and the
//! interpreter prefix the plan named. The adapter for this build is constructed
//! **without** `allow_network`, so a network request is refused by policy rather
//! than by remembering to pass `NetworkPolicy::Disabled`.
//!
//! Installation is `--no-index --find-links <wheelhouse> --require-hashes`, so
//! even inside the sandbox the locked digests decide what may be installed.

use std::path::{Path, PathBuf};

use anchor_runtime::{NetworkPolicy, ReadOnlyInput, SandboxPort, SandboxRequest, SandboxStatus};
use anchor_sandbox_bwrap::{BubblewrapPolicy, BubblewrapSandbox};

use super::config::{EnvironmentProvisionConfig, ProvisionError};
use super::resolve::{ResolvedRequirements, lock_path};

/// A built (but not yet published) environment tree.
#[derive(Debug, Clone)]
pub(crate) struct BuiltEnvironment {
    /// The tree to publish: it already has `bin/` at its root.
    pub(crate) tree: PathBuf,
    pub(crate) files: usize,
    pub(crate) bytes: u64,
}

/// The adapter one build runs in.
///
/// `allow_network` is intentionally left unset: this sandbox cannot run a
/// network-enabled request, whatever a later caller asks for. The pinned `uv`
/// directory is authorized once so every step finds the same tool through PATH.
pub(crate) fn provision_sandbox(
    config: &EnvironmentProvisionConfig,
) -> Result<BubblewrapSandbox, ProvisionError> {
    let uv = config.verified_uv()?;
    let uv_dir = uv
        .parent()
        .ok_or_else(|| ProvisionError::Failed("the uv binary has no directory".to_owned()))?
        .to_path_buf();
    // The deployment named this root; the adapter makes sure it exists before it
    // canonicalizes it into the sandbox policy.
    std::fs::create_dir_all(&config.build_root).map_err(|error| {
        ProvisionError::Failed(format!(
            "the environment build root {} is unavailable: {error}",
            config.build_root.display()
        ))
    })?;
    let policy = BubblewrapPolicy::new(config.bwrap.clone(), ["uv"])
        .authorize_workspace_root(&config.build_root)
        .authorize_tool_dir(&uv_dir);
    BubblewrapSandbox::new(policy).map_err(|error| {
        ProvisionError::Failed(format!(
            "the environment build sandbox is unavailable: {error}"
        ))
    })
}

/// One invocation's authority, both halves of it.
///
/// A sandbox that carries grants is only the *authority* half; the same mounts
/// must also appear in the request, or the process runs without them. Keeping
/// both halves in one value is what stops a later edit from applying only one.
pub(crate) struct Authority {
    pub(crate) sandbox: BubblewrapSandbox,
    grants: Vec<ReadOnlyInput>,
    tool_dirs: Vec<PathBuf>,
}

impl Authority {
    pub(crate) fn request(&self, workspace: &Path, command: &[String]) -> SandboxRequest {
        let mut request = SandboxRequest::new(workspace.to_path_buf(), command.to_vec());
        request.network = NetworkPolicy::Disabled;
        request.readonly_inputs = self.grants.clone();
        request.tool_dirs = self.tool_dirs.clone();
        request
    }
}

/// Assemble both halves of one invocation's authority.
pub(crate) fn authority(
    sandbox: &BubblewrapSandbox,
    grants: Vec<ReadOnlyInput>,
    tool_dirs: &[PathBuf],
) -> Result<Authority, ProvisionError> {
    let scoped = sandbox
        .with_readonly_grants(&grants)
        .and_then(|sandbox| sandbox.with_executable_dirs(tool_dirs))
        .map_err(|error| {
            ProvisionError::Failed(format!(
                "the build authority could not be assembled: {error}"
            ))
        })?;
    Ok(Authority {
        sandbox: scoped,
        grants,
        tool_dirs: tool_dirs.to_vec(),
    })
}

/// Build the environment for one approved plan.
///
/// The scratch tree lives under the deployment's own build root — the sandbox
/// would refuse anything else — and is keyed by the lock digest, so two builds of
/// the same approved plan share one staging directory and neither is ever
/// published content. The returned tree is what the caller publishes.
pub(crate) async fn build_environment(
    config: &EnvironmentProvisionConfig,
    sandbox: &BubblewrapSandbox,
    resolved: &ResolvedRequirements,
    wheelhouse: &Path,
) -> Result<BuiltEnvironment, ProvisionError> {
    let staging = config.build_root.join(&resolved.lock_digest);
    let input = staging.join("input");
    let workspace = staging.join("root");
    cleanup(&staging)?;
    std::fs::create_dir_all(&input)
        .and_then(|_| std::fs::create_dir_all(&workspace))
        .map_err(|error| ProvisionError::Failed(format!("build staging failed: {error}")))?;
    let lock = lock_path(&input);
    std::fs::write(&lock, &resolved.lock).map_err(|error| {
        ProvisionError::Failed(format!("the lock could not be staged: {error}"))
    })?;

    let uv = config.verified_uv()?;
    super::resolve::verify_interpreter(resolved)?;
    let uv_dir = uv
        .parent()
        .ok_or_else(|| ProvisionError::Failed("the uv binary has no directory".to_owned()))?
        .to_path_buf();
    // The build needs exactly three things it did not have before: the pinned
    // tool, the artifacts, and the interpreter the plan named.
    let authority = authority(
        sandbox,
        vec![
            ReadOnlyInput::new(&uv, &uv),
            ReadOnlyInput::new(wheelhouse, wheelhouse),
            ReadOnlyInput::new(&input, &input),
        ],
        &[uv_dir],
    )?;

    let uv_arg = sandbox_path(&uv);
    let python_arg = sandbox_path(&resolved.interpreter_path);
    let lock_arg = sandbox_path(&lock);
    let wheelhouse_arg = sandbox_path(wheelhouse);

    let venv = run_step(
        config,
        &authority,
        &workspace,
        &[
            uv_arg.clone(),
            "venv".into(),
            "--relocatable".into(),
            "--python".into(),
            python_arg,
            "/workspace".into(),
        ],
        "creating the virtual environment",
    )
    .await?;
    let _ = venv;

    run_step(
        config,
        &authority,
        &workspace,
        &[
            uv_arg,
            "pip".into(),
            "install".into(),
            "--python".into(),
            "/workspace/bin/python".into(),
            "--no-index".into(),
            "--find-links".into(),
            wheelhouse_arg,
            "--require-hashes".into(),
            "--requirement".into(),
            lock_arg,
        ],
        "installing the locked set",
    )
    .await?;

    // A venv is full of links to the deployment's interpreter; the Library only
    // publishes regular files and directories, so make the tree its own.
    super::materialize::materialize(&workspace, &resolved.interpreter_path)?;

    let (files, bytes) = tree_size(&workspace)?;
    Ok(BuiltEnvironment {
        tree: workspace,
        files,
        bytes,
    })
}

/// Run the published tree at its final path, which is the only way to know the
/// environment still works after it moved.
///
/// The interpreter is asked for its own prefix and its installed distributions:
/// a venv whose absolute paths survived the move reports the published path and
/// exactly the locked set, and a broken one reports neither.
pub(crate) async fn verify_published(
    config: &EnvironmentProvisionConfig,
    sandbox: &BubblewrapSandbox,
    published: &Path,
    resolved: &ResolvedRequirements,
) -> Result<(), ProvisionError> {
    let authority = authority(
        sandbox,
        vec![ReadOnlyInput::new(published, published)],
        // The environment's own `bin` is what a Run authorizes too, so the probe
        // runs the published interpreter the same way an execution would.
        std::slice::from_ref(&published.join("bin")),
    )?;
    // The probe reports its own view of the world rather than failing with a
    // traceback, so a broken environment is explained instead of merely rejected.
    const PROBE: &str = "import json, sys\n\
        report = {\n\
        \x20   'version': '%d.%d.%d' % sys.version_info[:3],\n\
        \x20   'prefix': sys.prefix,\n\
        \x20   'base_prefix': sys.base_prefix,\n\
        \x20   'path': sys.path,\n\
        \x20   'distributions': [],\n\
        }\n\
        try:\n\
        \x20   import importlib.metadata as metadata\n\
        \x20   report['distributions'] = sorted(\n\
        \x20       [d.metadata['Name'], d.version] for d in metadata.distributions())\n\
        except Exception as error:\n\
        \x20   report['error'] = repr(error)\n\
        print(json.dumps(report))";
    // Verification needs a writable workspace like every sandbox request, but it
    // must not be the published tree: the probe reads the environment through the
    // read-only grant, exactly as an execution would.
    let workspace = config.build_root.join("verify-workspace");
    std::fs::create_dir_all(&workspace).map_err(|error| {
        ProvisionError::Failed(format!(
            "the verification workspace is unavailable: {error}"
        ))
    })?;
    let output = run_step(
        config,
        &authority,
        &workspace,
        &[
            sandbox_path(&published.join("bin/python")),
            "-c".into(),
            PROBE.into(),
        ],
        "running the published environment",
    )
    .await?;
    let report: serde_json::Value = serde_json::from_str(output.trim()).map_err(|error| {
        ProvisionError::Failed(format!(
            "the published environment reported no usable result: {error}"
        ))
    })?;
    let prefix = report
        .get("prefix")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if prefix != published.to_string_lossy() {
        return Err(ProvisionError::Failed(format!(
            "the published environment runs with prefix `{prefix}` instead of its published path, so it is not usable where it was published"
        )));
    }
    let version = report
        .get("version")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if version != resolved.interpreter {
        return Err(ProvisionError::Failed(format!(
            "the published environment runs Python {version}, not the planned {}",
            resolved.interpreter
        )));
    }
    if let Some(error) = report.get("error").and_then(serde_json::Value::as_str) {
        return Err(ProvisionError::Failed(format!(
            "the published environment cannot read its own installed set ({error}); it reports base prefix `{}`",
            report
                .get("base_prefix")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("?")
        )));
    }
    let mut observed: Vec<(String, String)> = report
        .get("distributions")
        .and_then(serde_json::Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    let pair = entry.as_array()?;
                    Some((
                        pair.first()?.as_str()?.to_owned(),
                        pair.get(1)?.as_str()?.to_owned(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    observed.sort();
    let mut planned: Vec<(String, String)> = resolved
        .packages
        .iter()
        .map(|package| (package.name.clone(), package.version.clone()))
        .collect();
    planned.sort();
    if observed.len() != planned.len()
        || observed
            .iter()
            .zip(&planned)
            .any(|(left, right)| !same_distribution(left, right))
    {
        return Err(ProvisionError::Failed(format!(
            "the published environment holds {observed:?}, not the approved {planned:?}"
        )));
    }
    Ok(())
}

fn same_distribution(left: &(String, String), right: &(String, String)) -> bool {
    super::fetch::normalize_name(&left.0) == super::fetch::normalize_name(&right.0)
        && left.1 == right.1
}

/// Run one build step and return its stdout, or the reason it failed.
///
/// `tool_dirs` are the directories this exact request is allowed to resolve a
/// command from; the adapter accepts them only because the sandbox already
/// carries the matching host grant.
async fn run_step(
    config: &EnvironmentProvisionConfig,
    authority: &Authority,
    workspace: &Path,
    command: &[String],
    what: &str,
) -> Result<String, ProvisionError> {
    let mut request = authority.request(workspace, command);
    request.timeout = config.step_timeout();
    request.max_output_bytes = config.max_step_output();
    request.environment = [
        ("UV_OFFLINE", "1"),
        ("UV_NO_CACHE", "1"),
        ("UV_PYTHON_DOWNLOADS", "never"),
        ("UV_LINK_MODE", "copy"),
    ]
    .into_iter()
    .map(|(key, value)| anchor_runtime::SandboxEnvironment::new(key, value))
    .collect();
    request.validate().map_err(|error| {
        ProvisionError::Failed(format!("{what} could not be requested: {error}"))
    })?;
    let result = authority.sandbox.run(request).await.map_err(|error| {
        ProvisionError::Failed(format!("{what} could not be sandboxed: {error}"))
    })?;
    match result.status {
        SandboxStatus::Completed if result.exit_code == Some(0) => Ok(result.stdout),
        SandboxStatus::Completed => Err(ProvisionError::Failed(format!(
            "{what} failed (exit {:?}): {}",
            result.exit_code,
            super::resolve::tail(&format!("{}{}", result.stdout, result.stderr))
        ))),
        SandboxStatus::TimedOut => Err(ProvisionError::Failed(format!("{what} timed out"))),
        SandboxStatus::Cancelled | SandboxStatus::NotExecuted => Err(ProvisionError::Failed(
            format!("{what} did not run: {}", result.reason),
        )),
    }
}

/// A host path as the sandbox sees it: the host path itself, because every grant
/// in this module is mounted at its own location.
fn sandbox_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn cleanup(path: &Path) -> Result<(), ProvisionError> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(ProvisionError::Failed(format!(
            "the staging directory {} could not be cleared: {error}",
            path.display()
        ))),
    }
}

fn tree_size(root: &Path) -> Result<(usize, u64), ProvisionError> {
    let mut files = 0usize;
    let mut bytes = 0u64;
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = std::fs::read_dir(&directory).map_err(|error| {
            ProvisionError::Failed(format!("{} is not readable: {error}", directory.display()))
        })?;
        for entry in entries.flatten() {
            let metadata = entry
                .metadata()
                .map_err(|error| ProvisionError::Failed(error.to_string()))?;
            if metadata.is_dir() {
                pending.push(entry.path());
            } else {
                files += 1;
                bytes += metadata.len();
            }
        }
    }
    Ok((files, bytes))
}
