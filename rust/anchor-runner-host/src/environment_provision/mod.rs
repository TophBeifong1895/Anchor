//! Authorized environment preparation: the builder behind the steward's asks.
//!
//! A dependency environment used to arrive as a directory somebody prepared on
//! the host. This module closes that gap: it turns a recorded *requirement* into
//! a published, immutable environment, in four steps that each keep their own
//! boundary.
//!
//! 1. **Resolve** (`resolve`) runs the deployment-provided `uv` against the
//!    deployment's authorized sources and freezes the exact locked set —
//!    package versions plus artifact hashes. This is the trusted adapter, and it
//!    is the only place that needs the network.
//! 2. **Fetch** (`fetch`) copies exactly the artifacts whose bytes hash to the
//!    locked digests into a wheelhouse. Content is identified by hash, never by
//!    a model-supplied URL, so the index document cannot substitute content.
//! 3. **Build** (`build`) runs `uv` inside Bubblewrap with **network disabled at
//!    the policy level**, creating the virtual environment and installing only
//!    from that wheelhouse with `--require-hashes`. The sandbox that runs it
//!    cannot be asked for network at all, so a later bug cannot turn it on.
//! 4. **Verify** (`verify`) runs the *published* tree at its final read-only
//!    path, because a venv can still carry absolute paths that only break once
//!    the tree has moved.
//!
//! What the deployment authorizes lives in [`config`]: which sources may be
//! used, which interpreter may be the base build, and which `uv` binary may be
//! executed (checked by digest). None of it is reachable from a model argument
//! or from an editable Graph field.

mod build;
mod config;
mod fetch;
mod materialize;
mod resolve;

pub(crate) use build::{
    authority, build_environment, provision_sandbox, run_step, verify_published,
};
pub(crate) use config::{EnvironmentProvisionConfig, PackageSource, ProvisionError, from_env};
pub(crate) use fetch::{fetch_wheelhouse, normalize_name, verify_index_json};
pub(crate) use resolve::{
    LockedPackage, ResolvedRequirements, is_interpreter_constraint, resolve_requirements,
};

use anchor_sandbox_bwrap::BubblewrapSandbox;

/// The deployment's provisioner: what it authorized, and the sandbox builds run in.
///
/// Built once, when the host starts, so a deployment that configured sources but
/// no usable sandbox refuses preparation instead of failing halfway through one.
pub(crate) struct EnvironmentProvisioner {
    pub(crate) config: EnvironmentProvisionConfig,
    pub(crate) sandbox: BubblewrapSandbox,
}

impl EnvironmentProvisioner {
    pub(crate) fn new(config: EnvironmentProvisionConfig) -> Result<Self, ProvisionError> {
        let sandbox = provision_sandbox(&config)?;
        Ok(Self { config, sandbox })
    }
}

#[cfg(test)]
pub(crate) mod test_support;
#[cfg(test)]
mod tests;
