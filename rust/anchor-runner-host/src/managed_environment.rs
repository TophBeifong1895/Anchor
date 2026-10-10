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
/// Two causes, because the boundary has to keep them apart: a Run that recorded
/// an environment the published store can no longer honor is a *corrupt Run*,
/// while a deployment that cannot serve environments at all is an *unsupported
/// capability*. Collapsing them into one string is how the AgentNode path used
/// to relabel the first as the second.
#[derive(Debug)]
pub(crate) enum Unusable {
    /// The recorded fact cannot be honored: the content is missing, changed,
    /// writable again, or provides nothing to run.
    Recorded(String),
    /// This host cannot serve the binding at all.
    Unconfigured(String),
}

impl std::fmt::Display for Unusable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Recorded(reason) | Self::Unconfigured(reason) => formatter.write_str(reason),
        }
    }
}

impl Unusable {
    /// The GraphRunner's class for this cause, so every path agrees.
    pub(crate) fn into_graph_error(self) -> anchor_runtime::graph::GraphError {
        use anchor_runtime::graph::GraphError;
        match self {
            Self::Recorded(reason) => GraphError::CorruptRun(reason),
            Self::Unconfigured(reason) => GraphError::Unsupported(reason),
        }
    }

    fn recorded(message: impl Into<String>) -> Self {
        Self::Recorded(message.into())
    }

    fn unconfigured(message: impl Into<String>) -> Self {
        Self::Unconfigured(message.into())
    }
}

#[derive(Debug)]
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
        Unusable::recorded(format!(
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
        return Err(Unusable::recorded(format!(
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
        let Some(server_id) = binding
            .site
            .as_ref()
            .and_then(anchor_runtime::graph::EnvironmentSite::plugin_server_id)
        else {
            continue;
        };
        let Some(root) = library_root else {
            return Err(Unusable::unconfigured(
                "no library root is configured for a declared dependency environment",
            ));
        };
        prepared.insert(server_id, prepare(binding, root)?);
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
        .map_err(|error| {
            Unusable::unconfigured(format!("environment preparation failed: {error}"))
        })?
}

/// The same preparation for a set of Plugin servers, off the async worker.
pub(crate) async fn prepare_servers_async(
    environments: BTreeMap<String, EnvironmentBinding>,
    library_root: Option<PathBuf>,
) -> Result<BTreeMap<PluginServerId, PreparedEnvironment>, Unusable> {
    tokio::task::spawn_blocking(move || prepare_servers(&environments, library_root.as_deref()))
        .await
        .map_err(|error| {
            Unusable::unconfigured(format!("environment preparation failed: {error}"))
        })?
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
        Unusable::unconfigured(
            "no library root is configured for a declared dependency environment",
        )
    })?;
    prepare(binding, root).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anchor_library::EnvironmentStore;

    /// A published tree that provides a command.
    fn candidate(root: &Path) -> PathBuf {
        let directory = root.join("candidate");
        std::fs::create_dir_all(directory.join("bin")).unwrap();
        std::fs::write(directory.join("bin/python"), "#!/bin/sh\n").unwrap();
        directory
    }

    #[test]
    fn a_recorded_fact_that_cannot_be_honored_is_a_corrupt_run() {
        let root = tempfile::tempdir().unwrap();
        let library = root.path().join("library");
        let store = EnvironmentStore::new(&library);
        let published = store.publish(candidate(root.path())).unwrap();
        let binding =
            anchor_runtime::graph::EnvironmentBinding::node_task("work", "research", &published.id);
        assert!(prepare(&binding, &library).is_ok());

        // The published content changes behind the Run's back.
        let tree = store.published_path(&published.id);
        let mut stack = vec![tree.clone()];
        while let Some(directory) = stack.pop() {
            for entry in std::fs::read_dir(&directory).unwrap() {
                let entry = entry.unwrap();
                let metadata = std::fs::symlink_metadata(entry.path()).unwrap();
                if metadata.is_dir() {
                    stack.push(entry.path());
                }
                let mut permissions = metadata.permissions();
                use std::os::unix::fs::PermissionsExt;
                permissions.set_mode(permissions.mode() | 0o700);
                std::fs::set_permissions(entry.path(), permissions).unwrap();
            }
        }
        std::fs::write(tree.join("bin/python"), "#!/bin/sh\ntampered\n").unwrap();

        let unusable = prepare(&binding, &library).unwrap_err();
        assert!(matches!(unusable, Unusable::Recorded(_)), "{unusable:?}");
        // The class the GraphRunner sees, which is the whole point of the split.
        assert!(matches!(
            unusable.into_graph_error(),
            anchor_runtime::graph::GraphError::CorruptRun(_)
        ));
    }

    #[test]
    fn a_deployment_that_cannot_serve_environments_is_unsupported_not_corrupt() {
        let binding = anchor_runtime::graph::EnvironmentBinding::node_task(
            "work",
            "research",
            "a".repeat(64),
        );
        let unusable = prepare_for(Some(&binding), None).unwrap_err();
        assert!(
            matches!(unusable, Unusable::Unconfigured(_)),
            "{unusable:?}"
        );
        assert!(matches!(
            unusable.into_graph_error(),
            anchor_runtime::graph::GraphError::Unsupported(_)
        ));
        // A node that declared nothing needs no library root at all.
        assert!(prepare_for(None, None).unwrap().is_none());
    }
}
