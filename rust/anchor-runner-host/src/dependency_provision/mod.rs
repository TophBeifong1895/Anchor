//! Authorized Python dependency resolution and package preparation.
//!
//! Host configuration selects a shared interpreter, a verified uv executable
//! and approved sources. Plugin and development declarations use this adapter;
//! Library owns the shared package bytes and their read-only views.
//!
//! 1. **Resolve** (`resolve`) runs the verified `uv` against the
//!    Host's authorized sources and records the exact locked set —
//!    package versions plus artifact hashes. This is the trusted adapter, and it
//!    is the only place that needs the network.
//! 2. **Fetch** (`fetch`) copies exactly the artifacts whose bytes hash to the
//!    locked digests into a wheelhouse. Content is identified by hash, never by
//!    a model-supplied URL, so the index document cannot substitute content.
//! 3. **Build** (`build`) runs foreign build inputs inside Bubblewrap with
//!    network disabled. Shared-store landing and view assembly reuse that
//!    sandbox, without granting package code task-network authority.
//! 4. **Verify** checks the resulting read-only view at its final path, because
//!    Python configuration and scripts can contain absolute references.
//!
//! The input contract lives in [`config`]. RuntimeConfiguration selects either
//! Host-managed settings or explicitly deployment-owned fields; a Graph,
//! candidate or development declaration cannot replace those permissions.

mod build;
mod config;
mod fetch;
mod resolve;

pub(crate) use build::{authority, provision_sandbox, run_step_with_access};
pub(crate) use config::{
    DependencyProvisionConfig, PYTHON_INTERPRETER_ENV, PackageSource, ProvisionError, from_env,
};
pub(crate) use fetch::{fetch_wheelhouse_with_access, normalize_name, verify_index_json};
pub(crate) use resolve::{LockedPackage, ResolvedRequirements, resolve_requirements_with_access};

#[cfg(test)]
pub(crate) use fetch::fetch_wheelhouse;
#[cfg(test)]
pub(crate) use resolve::resolve_requirements;

use anchor_sandbox_bwrap::BubblewrapSandbox;

#[derive(Clone, Debug)]
pub(crate) struct DependencyAccess {
    pub(crate) network: bool,
    pub(crate) cancellation: anchor_runtime::Cancellation,
}

impl Default for DependencyAccess {
    fn default() -> Self {
        Self {
            network: true,
            cancellation: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }
}

impl DependencyAccess {
    pub(crate) async fn cancelled(&self) {
        while !self.cancellation.load(std::sync::atomic::Ordering::Acquire) {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    pub(crate) fn check(&self) -> Result<(), ProvisionError> {
        if self.cancellation.load(std::sync::atomic::Ordering::Acquire) {
            Err(ProvisionError::Failed(
                "dependency preparation was cancelled".into(),
            ))
        } else {
            Ok(())
        }
    }
}

/// The Host's approved preparation inputs and isolated build sandbox.
///
/// Constructed with a RuntimeSnapshot; an active Pilot Turn retains that exact
/// instance. Invalid inputs or an unavailable sandbox refuse preparation.
pub(crate) struct DependencyProvisioner {
    pub(crate) config: DependencyProvisionConfig,
    pub(crate) sandbox: BubblewrapSandbox,
}

impl DependencyProvisioner {
    pub(crate) fn new(config: DependencyProvisionConfig) -> Result<Self, ProvisionError> {
        let sandbox = provision_sandbox(&config)?;
        Ok(Self { config, sandbox })
    }
}

#[cfg(test)]
pub(crate) mod test_support;
#[cfg(test)]
mod tests;
