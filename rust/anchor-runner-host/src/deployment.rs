//! Deployment identities shared by entrypoints and node authorization.
//!
//! These values come from operator configuration, never Graph or Session input.

use std::path::PathBuf;

/// The deployment's Pilot Graph, whose required Plugins are host-authorized.
/// HTTP mutation protection and node mounting must use this same derivation.
pub(crate) fn configured_pilot_graph() -> Option<String> {
    std::env::var("ANCHOR_RUNNER_GRAPH_NAME")
        .ok()
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .or_else(|| {
            std::env::var_os("ANCHOR_RUNNER_BUNDLE_ROOT")
                .map(PathBuf::from)
                .and_then(|root| {
                    root.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                })
                .filter(|name| !name.is_empty())
        })
}
