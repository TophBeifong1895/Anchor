//! How a frozen dependency-environment binding becomes per-invocation authority.
//!
//! Both the AgentNode tool path and the OpNode command path must run with exactly
//! the published tree the Run recorded — mounted read-only at its own published
//! path, so absolute references inside the environment keep working — and neither
//! may fall back to the global tool-environment aggregate.
//!
//! A frozen environment has two halves. The *authority* half extends one
//! invocation's sandbox with a host grant, because a Run's policy is built before
//! its called Graphs are known: a child Run's environments cannot be in the
//! parent's policy. The *request* half carries the mount and the PATH entry, which
//! the adapter then accepts because the grant authorizes exactly that pair.
//!
//! The recorded digest is re-verified while preparing, once per node invocation.
//! A published tree that changed after admission must refuse to run rather than
//! execute bytes the Run never recorded; the check is per invocation (not per
//! command) because re-hashing an environment is not free.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anchor_library::EnvironmentStore;
use anchor_runtime::graph::{EnvironmentBinding, PluginServerId};
use anchor_runtime::{ReadOnlyInput, SandboxRequest};
use anchor_sandbox_bwrap::BubblewrapSandbox;

/// Why one frozen binding cannot be used.
///
/// One cause, one message, one type: each boundary decides only which error
/// class its own interface uses (`GraphError::CorruptRun` where a Run's recorded
/// fact cannot be honored, `String` where the host returns a tool error).
#[derive(Debug)]
pub(crate) struct Unusable(String);

impl std::fmt::Display for Unusable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Unusable {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

pub(crate) struct PreparedEnvironment {
    grant: ReadOnlyInput,
    bin: Option<PathBuf>,
}

/// Verify one frozen binding and describe how to mount it.
pub(crate) fn prepare(
    binding: &EnvironmentBinding,
    library_root: &Path,
) -> Result<PreparedEnvironment, Unusable> {
    let store = EnvironmentStore::new(library_root);
    store.show(&binding.id).map_err(|error| {
        Unusable::new(format!(
            "dependency environment `{}` ({}) is not usable: {error}",
            binding.name, binding.id
        ))
    })?;
    // The store owns its layout; nothing here re-derives `<root>/envs/<id>`.
    let published = store.published_path(&binding.id);
    // A published environment contributes commands only through `bin/`. Without
    // it the invocation would run with the deployment's own interpreters while
    // the Run records that it used this environment — the false fact the contract
    // forbids.
    let bin = published.join("bin");
    if !bin.is_dir() {
        return Err(Unusable::new(format!(
            "dependency environment `{}` ({}) has no `bin` directory, so it cannot provide any command",
            binding.name, binding.id
        )));
    }
    Ok(PreparedEnvironment {
        grant: ReadOnlyInput::new(&published, &published),
        bin: Some(bin),
    })
}

impl PreparedEnvironment {
    /// Grant this invocation exactly the tree and PATH entry it recorded.
    pub(crate) fn authorize(
        &self,
        sandbox: Arc<BubblewrapSandbox>,
    ) -> Result<Arc<BubblewrapSandbox>, String> {
        let mut extended = sandbox
            .with_readonly_grants(std::slice::from_ref(&self.grant))
            .map_err(|error| error.to_string())?;
        if let Some(bin) = self.bin.as_ref() {
            extended = extended
                .with_executable_dirs(std::slice::from_ref(bin))
                .map_err(|error| error.to_string())?;
        }
        Ok(Arc::new(extended))
    }

    /// Put the mount and the PATH entry into this invocation's request.
    pub(crate) fn apply(&self, request: &mut SandboxRequest) {
        request.readonly_inputs.push(self.grant.clone());
        if let Some(bin) = self.bin.as_ref() {
            request.tool_dirs.push(bin.clone());
        }
    }
}

/// Prepare the frozen environments of this invocation's Plugin servers.
///
/// Keyed by `<plugin>/<server>`: the launcher knows both when it starts one
/// stdio server, and each server gets exactly the environment its declaration
/// froze.
pub(crate) fn prepare_servers(
    environments: &BTreeMap<String, EnvironmentBinding>,
    library_root: Option<&Path>,
) -> Result<BTreeMap<PluginServerId, PreparedEnvironment>, Unusable> {
    let mut prepared = BTreeMap::new();
    for binding in environments.values() {
        let Some(anchor_runtime::graph::EnvironmentSite::PluginServer { plugin, server, .. }) =
            binding.site.as_ref()
        else {
            continue;
        };
        let Some(root) = library_root else {
            return Err(Unusable::new(
                "no library root is configured for a declared dependency environment",
            ));
        };
        prepared.insert(PluginServerId::new(plugin, server), prepare(binding, root)?);
    }
    Ok(prepared)
}

/// The same preparation, off the async worker: verifying a published tree
/// re-hashes it, which must not block the runtime thread.
pub(crate) async fn prepare_for_async(
    binding: Option<EnvironmentBinding>,
    library_root: Option<PathBuf>,
) -> Result<Option<PreparedEnvironment>, Unusable> {
    tokio::task::spawn_blocking(move || prepare_for(binding.as_ref(), library_root.as_deref()))
        .await
        .map_err(|error| Unusable::new(format!("environment preparation failed: {error}")))?
}

/// The same preparation for a set of Plugin servers, off the async worker.
pub(crate) async fn prepare_servers_async(
    environments: BTreeMap<String, EnvironmentBinding>,
    library_root: Option<PathBuf>,
) -> Result<BTreeMap<PluginServerId, PreparedEnvironment>, Unusable> {
    tokio::task::spawn_blocking(move || prepare_servers(&environments, library_root.as_deref()))
        .await
        .map_err(|error| Unusable::new(format!("environment preparation failed: {error}")))?
}

/// Prepare a frozen environment for one invocation, if it declared one.
pub(crate) fn prepare_for(
    binding: Option<&EnvironmentBinding>,
    library_root: Option<&Path>,
) -> Result<Option<PreparedEnvironment>, Unusable> {
    let Some(binding) = binding else {
        return Ok(None);
    };
    let root = library_root.ok_or_else(|| {
        Unusable::new("no library root is configured for a declared dependency environment")
    })?;
    prepare(binding, root).map(Some)
}
