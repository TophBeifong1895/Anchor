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

use super::config::{DependencyProvisionConfig, ProvisionError};

/// The adapter one build runs in.
///
/// `allow_network` is intentionally left unset: this sandbox cannot run a
/// network-enabled request, whatever a later caller asks for. The pinned `uv`
/// directory is authorized once so every step finds the same tool through PATH.
pub(crate) fn provision_sandbox(
    config: &DependencyProvisionConfig,
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

/// Run one build step and return its stdout, or the reason it failed.
///
/// `tool_dirs` are the directories this exact request is allowed to resolve a
/// command from; the adapter accepts them only because the sandbox already
/// carries the matching host grant.
pub(crate) async fn run_step(
    config: &DependencyProvisionConfig,
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
