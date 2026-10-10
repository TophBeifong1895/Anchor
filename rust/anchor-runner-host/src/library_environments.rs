//! The host's adapter for the Graph-call layer's environment resolver port.
//!
//! `anchor-graph-host` decides *when* a child Run must freeze its declared
//! environments; this host owns *how*, because the published store and the
//! deployment's Library root are host concerns.

use std::path::PathBuf;

use anchor_graph_host::EnvironmentResolver;
use anchor_library::EnvironmentStore;

pub(crate) struct LibraryEnvironmentResolver {
    root: PathBuf,
}

impl LibraryEnvironmentResolver {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

impl EnvironmentResolver for LibraryEnvironmentResolver {
    fn resolve(&self, name: &str) -> Result<String, String> {
        crate::application::resolve_name(&EnvironmentStore::new(&self.root), name)
    }
}
