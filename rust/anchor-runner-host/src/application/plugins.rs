use super::graphs::{ImpactCoverage, ImpactScan, coverage_of};
use super::{ApplicationError, RunApplication, is_unfinished};
use anchor_graph_host::FilePluginCatalog;
use anchor_library::{
    Checkout, InstallError, InstallOutcome, InstallRequest, Library, PluginReview,
};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};

/// What a human is asked to approve for one Plugin candidate.
///
/// The digest is the one review produced, so the approval is bound to exactly
/// the tree that would be published. `existing_digest` records what the Library
/// holds for the same id at review time, which is what the unchanged check
/// compares against after the answer arrives.
#[derive(Debug, Clone)]
pub(crate) struct PluginInstallPrecondition {
    pub(crate) review: PluginReview,
    pub(crate) directory: PathBuf,
    pub(crate) existing_digest: Option<String>,
    /// What replacing this Plugin touches. Changing a Plugin's manifest is a
    /// *binding* change: the Runs below froze the digest they were admitted
    /// with, so they will refuse to continue until the operator decides.
    pub(crate) impact: PluginImpact,
}

/// Who currently depends on one Plugin, as a derived view.
///
/// Never stored: it is a projection over the Graph catalog and the Run records,
/// so it cannot drift from them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PluginImpact {
    /// Saved Graphs that bind this Plugin.
    pub(crate) graphs: Vec<String>,
    /// Runs that are not terminal and froze a binding for this Plugin. These are
    /// the ones a replacement can strand.
    pub(crate) unfinished_runs: Vec<String>,
    /// How much of the catalog this report actually saw. A Graph this host
    /// cannot load is a gap, never a silent omission: the operator is being
    /// asked to trust a count.
    pub(crate) coverage: ImpactCoverage,
}

impl PluginInstallPrecondition {
    pub(crate) fn replacing(&self) -> bool {
        self.existing_digest.is_some()
    }

    /// True when the Library already holds this exact tree.
    pub(crate) fn already_installed(&self) -> bool {
        self.existing_digest.as_deref() == Some(self.review.digest.as_str())
    }
}

impl RunApplication {
    pub(crate) async fn install_plugin(
        &self,
        library_root: PathBuf,
        request: InstallRequest,
        checkout: Option<Arc<dyn Checkout>>,
    ) -> Result<InstallOutcome, ApplicationError> {
        let catalog_guard = self.graph_catalog_mutation_guard().await;
        tokio::task::spawn_blocking(move || {
            let _catalog_guard = catalog_guard;
            let library = Library::new(library_root);
            let installed = match checkout {
                Some(checkout) => library.install_with_checkout(&request, checkout.as_ref()),
                None => library.install(&request),
            };
            installed.map_err(|failure| match failure {
                InstallError::Io(_)
                | InstallError::PublicationUncertain
                | InstallError::CheckoutFailed
                | InstallError::CheckoutTimedOut => ApplicationError::Storage(failure.to_string()),
                _ => ApplicationError::Invalid(failure.to_string()),
            })
        })
        .await
        .map_err(|_| ApplicationError::Storage("Plugin installer task failed".into()))?
    }

    /// Review a candidate and freeze what the operator is about to approve.
    ///
    /// Read-only: it publishes nothing and needs no Library write access.
    pub(crate) async fn plugin_install_precondition(
        &self,
        library_root: PathBuf,
        directory: PathBuf,
        id: String,
    ) -> Result<PluginInstallPrecondition, ApplicationError> {
        let reviewed = directory.clone();
        let (review, existing_digest) = tokio::task::spawn_blocking(move || {
            let review = Library::new(&library_root)
                .review_directory(&id, &reviewed)
                .map_err(plugin_error)?;
            let existing_digest = FilePluginCatalog::new(&library_root)
                .definition(&id)
                .ok()
                .map(|definition| definition.digest);
            Ok::<_, ApplicationError>((review, existing_digest))
        })
        .await
        .map_err(|_| ApplicationError::Storage("Plugin review task failed".into()))??;
        // What the replacement would reach is a host projection over the Graph
        // catalog and the Run records, so it is read here rather than guessed.
        let impact = self.plugin_impact(&review.id)?;
        Ok(PluginInstallPrecondition {
            review,
            directory,
            existing_digest,
            impact,
        })
    }

    /// Which Graphs and unfinished Runs depend on one Plugin right now.
    fn plugin_impact(&self, id: &str) -> Result<PluginImpact, ApplicationError> {
        let (bundles, coverage) = match self.impact_scan() {
            ImpactScan::Read {
                bundles,
                considered,
                total,
                missing,
            } => (bundles, coverage_of(considered, total, missing)),
            ImpactScan::Unreadable(reason) => (Vec::new(), ImpactCoverage::Unknown { reason }),
        };
        let mut graphs = Vec::new();
        for (name, bundle) in &bundles {
            if bundle.plugins.iter().any(|binding| binding.id == id) {
                graphs.push(name.clone());
            }
        }
        graphs.sort();
        let mut unfinished_runs = Vec::new();
        for (run_id, record) in self.records()? {
            if is_unfinished(record.status) && record.plugin_bindings.contains_key(id) {
                unfinished_runs.push(run_id);
            }
        }
        unfinished_runs.sort();
        Ok(PluginImpact {
            graphs,
            unfinished_runs,
            coverage,
        })
    }

    /// Publish the reviewed candidate only if nothing moved since the review.
    ///
    /// A confirmation the operator gave for one digest must never install a
    /// different tree, so the candidate is reviewed again and the Library state
    /// is re-read before anything is written.
    /// `acknowledged_bound_runs` is the operator's separate decision about the
    /// non-terminal Runs this replacement strands: a Plugin manifest is part of
    /// the binding those Runs froze, so replacing it needs more than the
    /// approval to overwrite the id. Keeping the decision here rather than only
    /// in the form makes it a property of the operation.
    pub(crate) async fn install_plugin_if_unchanged(
        &self,
        library_root: PathBuf,
        precondition: PluginInstallPrecondition,
        acknowledged_bound_runs: bool,
    ) -> Result<InstallOutcome, ApplicationError> {
        if !precondition.impact.unfinished_runs.is_empty() && !acknowledged_bound_runs {
            return Err(ApplicationError::Conflict(format!(
                "{} unfinished Run(s) bind Plugin `{}`; confirm that they will refuse to continue",
                precondition.impact.unfinished_runs.len(),
                precondition.review.id
            )));
        }
        let catalog_guard = self.graph_catalog_mutation_guard().await;
        tokio::task::spawn_blocking(move || {
            let _catalog_guard = catalog_guard;
            let library = Library::new(&library_root);
            let reviewed = library
                .review_directory(&precondition.review.id, &precondition.directory)
                .map_err(plugin_error)?;
            if reviewed.digest != precondition.review.digest {
                return Err(ApplicationError::Conflict(
                    "the Plugin candidate changed while the user was deciding; ask again".into(),
                ));
            }
            let existing_digest = FilePluginCatalog::new(&library_root)
                .definition(&precondition.review.id)
                .ok()
                .map(|definition| definition.digest);
            if existing_digest != precondition.existing_digest {
                return Err(ApplicationError::Conflict(
                    "the installed Plugin changed while the user was deciding; ask again".into(),
                ));
            }
            library
                .install_directory(
                    &precondition.review.id,
                    &precondition.directory,
                    precondition.replacing(),
                )
                .map_err(plugin_error)
        })
        .await
        .map_err(|_| ApplicationError::Storage("Plugin installer task failed".into()))?
    }
}

fn plugin_error(failure: InstallError) -> ApplicationError {
    match failure {
        InstallError::Io(_)
        | InstallError::PublicationUncertain
        | InstallError::CheckoutFailed
        | InstallError::CheckoutTimedOut => ApplicationError::Storage(failure.to_string()),
        _ => ApplicationError::Invalid(failure.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{fs, path::Path};

    fn candidate(root: &Path, marker: &str) -> PathBuf {
        let directory = root.join(format!("candidate-{marker}"));
        fs::create_dir_all(directory.join("skills/check")).unwrap();
        fs::write(
            directory.join("plugin.json"),
            json!({
                "name": "Candidate",
                "description": marker,
                "skills": "skills",
                "mcpServers": {"probe": {"command": "bin/run"}}
            })
            .to_string(),
        )
        .unwrap();
        fs::write(directory.join("skills/check/SKILL.md"), "# check\n").unwrap();
        directory
    }

    fn application(root: &Path) -> RunApplication {
        RunApplication::new(root.join("state"), root.to_path_buf())
    }

    fn installed_digest(library_root: &Path, id: &str) -> Option<String> {
        FilePluginCatalog::new(library_root)
            .definition(id)
            .ok()
            .map(|definition| definition.digest)
    }

    #[tokio::test]
    async fn a_precondition_reports_creation_then_an_identical_install() {
        let temporary = tempfile::tempdir().unwrap();
        let library_root = temporary.path().join("library");
        let application = application(temporary.path());
        let source = candidate(temporary.path(), "first");

        let before = application
            .plugin_install_precondition(library_root.clone(), source.clone(), "demo".into())
            .await
            .unwrap();
        assert!(!before.replacing());
        assert!(!before.already_installed());

        application
            .install_plugin_if_unchanged(library_root.clone(), before.clone(), false)
            .await
            .unwrap();
        assert_eq!(
            installed_digest(&library_root, "demo").as_deref(),
            Some(before.review.digest.as_str())
        );

        let after = application
            .plugin_install_precondition(library_root, source, "demo".into())
            .await
            .unwrap();
        assert!(after.replacing());
        assert!(
            after.already_installed(),
            "the same candidate must be reported as already installed"
        );
    }

    #[tokio::test]
    async fn a_candidate_that_changed_after_review_is_never_installed() {
        let temporary = tempfile::tempdir().unwrap();
        let library_root = temporary.path().join("library");
        let application = application(temporary.path());
        let source = candidate(temporary.path(), "first");
        let precondition = application
            .plugin_install_precondition(library_root.clone(), source.clone(), "demo".into())
            .await
            .unwrap();

        // The tree the operator looked at is no longer the tree on disk.
        fs::write(
            source.join("skills/check/EXTRA.md"),
            "changed after review\n",
        )
        .unwrap();

        let failure = application
            .install_plugin_if_unchanged(library_root.clone(), precondition, false)
            .await
            .unwrap_err();
        assert!(
            matches!(failure, ApplicationError::Conflict(_)),
            "{failure:?}"
        );
        assert!(installed_digest(&library_root, "demo").is_none());
    }

    #[tokio::test]
    async fn a_library_that_changed_after_review_is_not_overwritten() {
        let temporary = tempfile::tempdir().unwrap();
        let library_root = temporary.path().join("library");
        let application = application(temporary.path());
        let source = candidate(temporary.path(), "first");
        let precondition = application
            .plugin_install_precondition(library_root.clone(), source, "demo".into())
            .await
            .unwrap();

        // Another writer installed a different version after the review.
        let other = candidate(temporary.path(), "second");
        Library::new(&library_root)
            .install_directory("demo", &other, false)
            .unwrap();
        let installed = installed_digest(&library_root, "demo");

        let failure = application
            .install_plugin_if_unchanged(library_root.clone(), precondition, false)
            .await
            .unwrap_err();
        assert!(
            matches!(failure, ApplicationError::Conflict(_)),
            "{failure:?}"
        );
        assert_eq!(installed_digest(&library_root, "demo"), installed);
    }

    #[tokio::test]
    async fn an_approved_precondition_replaces_only_the_reviewed_version() {
        let temporary = tempfile::tempdir().unwrap();
        let library_root = temporary.path().join("library");
        let application = application(temporary.path());
        let first = candidate(temporary.path(), "first");
        let first_precondition = application
            .plugin_install_precondition(library_root.clone(), first, "demo".into())
            .await
            .unwrap();
        application
            .install_plugin_if_unchanged(library_root.clone(), first_precondition, false)
            .await
            .unwrap();

        let second = candidate(temporary.path(), "second");
        let precondition = application
            .plugin_install_precondition(library_root.clone(), second, "demo".into())
            .await
            .unwrap();
        assert!(precondition.replacing());
        assert!(!precondition.already_installed());
        let outcome = application
            .install_plugin_if_unchanged(library_root.clone(), precondition.clone(), false)
            .await
            .unwrap();
        assert_eq!(outcome.digest, precondition.review.digest);
        assert_eq!(
            installed_digest(&library_root, "demo").as_deref(),
            Some(precondition.review.digest.as_str())
        );
    }
}

#[cfg(test)]
mod impact_tests {
    use super::*;
    use anchor_graph_host::PluginCatalog;
    use anchor_runtime::graph::{GraphRunRecord, RunStore};
    use serde_json::json;
    use std::fs;

    fn candidate(root: &std::path::Path, marker: &str) -> PathBuf {
        let directory = root.join(format!("candidate-{marker}"));
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("plugin.json"),
            json!({
                "name": "Candidate",
                "description": marker,
                "mcpServers": {"probe": {"command": "bin/run"}}
            })
            .to_string(),
        )
        .unwrap();
        directory
    }

    /// A non-terminal Run that froze a binding for this Plugin.
    fn unfinished_run(root: &std::path::Path, id: &str) -> String {
        let state = root.join("state");
        let store = anchor_runtime::graph::FileRunStore::new(state.join("runs"));
        let mut record = GraphRunRecord::create_with_id(
            serde_json::from_value(json!({
                "objective": "fixture",
                "entry": "only",
                "agents": {},
                "ops": {"run": {"run": "true"}},
                "nodes": [{"id": "only", "op": "run"}],
                "edges": []
            }))
            .unwrap(),
            json!({}),
            "run-bound",
        )
        .unwrap();
        record.plugin_bindings.insert(
            id.to_owned(),
            anchor_runtime::graph::PluginBinding {
                id: id.to_owned(),
                digest: "a".repeat(64),
                resources: Vec::new(),
                mcp_servers: vec!["probe".into()],
                environments: Default::default(),
            },
        );
        record.plugin_bindings_initialized = true;
        store.save(&record).unwrap();
        record.run_id
    }

    #[tokio::test]
    async fn replacing_a_plugin_that_unfinished_runs_bind_needs_a_second_decision() {
        let root = tempfile::tempdir().unwrap();
        let library_root = root.path().join("library");
        let application = RunApplication::new(root.path().join("state"), root.path().to_path_buf());
        let source = candidate(root.path(), "first");
        application
            .install_plugin_if_unchanged(
                library_root.clone(),
                application
                    .plugin_install_precondition(
                        library_root.clone(),
                        source.clone(),
                        "demo".into(),
                    )
                    .await
                    .unwrap(),
                false,
            )
            .await
            .unwrap();
        let run = unfinished_run(root.path(), "demo");

        // A candidate that differs from what is installed is a binding change.
        let next = candidate(root.path(), "second");
        let precondition = application
            .plugin_install_precondition(library_root.clone(), next, "demo".into())
            .await
            .unwrap();
        assert_eq!(precondition.impact.unfinished_runs, vec![run.clone()]);
        assert!(precondition.impact.graphs.is_empty());

        // Without the acknowledgement the replacement is refused...
        let error = application
            .install_plugin_if_unchanged(library_root.clone(), precondition.clone(), false)
            .await
            .unwrap_err();
        assert!(
            matches!(error, ApplicationError::Conflict(ref reason) if reason.contains("unfinished Run")),
            "{error:?}"
        );
        // ...and with it, it happens.
        application
            .install_plugin_if_unchanged(library_root.clone(), precondition.clone(), true)
            .await
            .unwrap();
        assert_eq!(
            FilePluginCatalog::new(&library_root)
                .definition("demo")
                .unwrap()
                .digest,
            precondition.review.digest
        );
    }

    /// A Graph this host cannot load is a *gap* in the report, never a silent
    /// omission: the operator is being asked to trust a count.
    #[tokio::test]
    async fn an_unreadable_graph_is_reported_as_a_gap_not_left_out() {
        let root = tempfile::tempdir().unwrap();
        let catalog = root.path().join("catalog");
        let application = RunApplication::new(root.path().join("state"), catalog.clone());

        // A loadable bundle that binds the Plugin.
        let bound = catalog.join("bound");
        fs::create_dir_all(bound.join("plugins/demo")).unwrap();
        fs::write(
            bound.join("plugins/demo/plugin.json"),
            json!({"name":"Demo","mcpServers":{"probe":{"command":"bin/run"}}}).to_string(),
        )
        .unwrap();
        let binding = FilePluginCatalog::new(&bound)
            .resolve(&["demo".to_owned()])
            .unwrap()
            .remove(0);
        fs::write(
            bound.join("graph.json"),
            json!({
                "objective": "uses the plugin",
                "entry": "only",
                "agents": {"worker": {"model": "fixture"}},
                "ops": {},
                "nodes": [{"id": "only", "agent": "worker", "plugins": ["demo"]}],
                "edges": []
            })
            .to_string(),
        )
        .unwrap();
        fs::write(
            bound.join("manifest.json"),
            json!({
                "format": 1,
                "graph": "graph.json",
                "plugins": [{
                    "id": "demo",
                    "digest": binding.digest,
                    "resources": binding.resources,
                    "mcp_servers": binding.mcp_servers,
                }]
            })
            .to_string(),
        )
        .unwrap();

        // A Graph directory with no manifest cannot be loaded at all.
        fs::create_dir_all(catalog.join("broken")).unwrap();
        fs::write(
            catalog.join("broken/graph.json"),
            json!({
                "objective": "broken",
                "entry": "only",
                "agents": {},
                "ops": {"run": {"run": "true"}},
                "nodes": [{"id": "only", "op": "run"}],
                "edges": []
            })
            .to_string(),
        )
        .unwrap();

        let impact = application.plugin_impact("demo").unwrap();
        assert_eq!(impact.graphs, vec!["bound".to_owned()], "{impact:?}");
        match impact.coverage {
            ImpactCoverage::Partial { missing, .. } => {
                assert_eq!(missing.len(), 1, "{missing:?}");
                assert_eq!(missing[0].graph, "broken");
                assert!(!missing[0].reason.is_empty(), "{missing:?}");
            }
            other => panic!("a Graph this host cannot load is a gap, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_plugin_nobody_binds_installs_without_the_extra_decision() {
        let root = tempfile::tempdir().unwrap();
        let library_root = root.path().join("library");
        let application = RunApplication::new(root.path().join("state"), root.path().to_path_buf());
        let precondition = application
            .plugin_install_precondition(
                library_root.clone(),
                candidate(root.path(), "alone"),
                "solo".into(),
            )
            .await
            .unwrap();
        assert!(precondition.impact.unfinished_runs.is_empty());
        application
            .install_plugin_if_unchanged(library_root, precondition, false)
            .await
            .unwrap();
    }
}
