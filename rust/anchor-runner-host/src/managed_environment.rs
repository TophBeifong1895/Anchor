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

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anchor_library::EnvironmentStore;
use anchor_runtime::graph::EnvironmentBinding;
use anchor_runtime::{ReadOnlyInput, SandboxRequest};
use anchor_sandbox_bwrap::BubblewrapSandbox;

pub(crate) struct PreparedEnvironment {
    grant: ReadOnlyInput,
    bin: Option<PathBuf>,
}

/// Verify one frozen binding and describe how to mount it.
pub(crate) fn prepare(
    binding: &EnvironmentBinding,
    library_root: &Path,
) -> Result<PreparedEnvironment, String> {
    EnvironmentStore::new(library_root)
        .show(&binding.id)
        .map_err(|error| {
            format!(
                "dependency environment `{}` ({}) is not usable: {error}",
                binding.name, binding.id
            )
        })?;
    let published = library_root.join("envs").join(&binding.id);
    // A published environment contributes commands only through `bin/`. Without
    // it the invocation would run with the deployment's own interpreters while
    // the Run records that it used this environment — the false fact the contract
    // forbids.
    let bin = published.join("bin");
    if !bin.is_dir() {
        return Err(format!(
            "dependency environment `{}` ({}) has no `bin` directory, so it cannot provide any command",
            binding.name, binding.id
        ));
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
    environments: &std::collections::BTreeMap<String, EnvironmentBinding>,
    library_root: Option<&Path>,
) -> Result<std::collections::BTreeMap<String, PreparedEnvironment>, String> {
    let mut prepared = std::collections::BTreeMap::new();
    for binding in environments.values() {
        let Some(anchor_runtime::graph::EnvironmentSite::PluginServer { plugin, server, .. }) =
            binding.site.as_ref()
        else {
            continue;
        };
        let Some(root) = library_root else {
            return Err(
                "no library root is configured for a declared dependency environment".to_owned(),
            );
        };
        prepared.insert(format!("{plugin}/{server}"), prepare(binding, root)?);
    }
    Ok(prepared)
}

/// Prepare a frozen environment for one invocation, if it declared one.
pub(crate) fn prepare_for(
    binding: Option<&EnvironmentBinding>,
    library_root: Option<&Path>,
) -> Result<Option<PreparedEnvironment>, String> {
    let Some(binding) = binding else {
        return Ok(None);
    };
    let root = library_root
        .ok_or("no library root is configured for a declared dependency environment")?;
    prepare(binding, root).map(Some)
}
