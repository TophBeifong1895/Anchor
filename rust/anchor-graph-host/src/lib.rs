//! Standalone Graph bundle loading and minimal in-process host adapter for
//! recursive Graph calls.
//!
//! The host supplies a graph catalog, the shared FileRunStore, artifact port,
//! node executor, and run control. Child Runs are always executed by the same
//! `GraphRunner` implementation. Bundle loading is read-only; result
//! materialization, session handoff, and platform APIs are outside this adapter.

mod bundle;
mod dependencies;
mod graph_call;
mod graph_catalog;
mod mcp;
mod plugin_authorization;
mod plugin_catalog;
mod plugin_configuration;
mod resources;

pub use bundle::{
    BundlePluginReference, FileGraphBundleLoader, GraphBundleManifest, LoadedGraphBundle,
};
pub use dependencies::{
    DEPENDENCIES_DIRECTORY, MutablePaths, PluginDependencies, STORE_DIRECTORY, VIEW_TOKEN,
    VIEWS_DIRECTORY, mutable_paths_from_manifest, plugin_view_path, resolved_library_root,
    store_root, views_root,
};
pub use graph_call::{ChildExecutionPort, InProcessGraphHost, child_run_id};
pub use graph_catalog::{ChildRunControl, GraphCatalog, PluginReferenceLeaseFuture};
pub use mcp::{McpServerDefinition, host_environment_references};
pub use plugin_catalog::{
    ChannelDefinition, FilePluginCatalog, PluginCatalog, PluginDefinition, PluginMetadata,
};
pub use plugin_configuration::{
    PluginConfiguration, PluginConfigurationField, PluginConfigurationFieldType,
    mcp_environment_fields,
};

use anchor_runtime::graph::GraphError;

fn invalid(message: String) -> GraphError {
    GraphError::InvalidSnapshot(message)
}

#[cfg(test)]
mod plugin_test_support;
