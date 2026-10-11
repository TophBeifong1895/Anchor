use super::graphs::{ImpactCoverage, ImpactScan, coverage_of};
use super::{ApplicationError, RunApplication};
use crate::dependency_provision::{
    LockedPackage, PackageSource, ProvisionError, ResolvedRequirements, resolve_requirements,
    verify_index_json,
};
use anchor_graph_host::{FilePluginCatalog, plugin_view_path, resolved_library_root};
use anchor_library::{InstallError, InstallOutcome, Library, PluginReview};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

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
    /// The resolved dependency set this approval covers, if the Plugin declares
    /// one. The resolution is frozen here: what the human approves is what gets
    /// installed, and a declaration that cannot be solved never reaches a form.
    pub(crate) dependencies: Option<PluginDependencyPlan>,
    /// The same resolution in the shape the install step consumes. Carried, not
    /// displayed: it is what the approved set will be reproduced from.
    pub(crate) resolved: Option<ResolvedRequirements>,
}

/// The deployment-authorized, resolved answer to one Plugin's declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PluginDependencyPlan {
    /// The deployment sources the resolution was allowed to use.
    pub(crate) sources: Vec<String>,
    pub(crate) interpreter: String,
    pub(crate) interpreter_path: PathBuf,
    pub(crate) interpreter_digest: String,
    /// The identity of the locked set, which is what the install reproduces.
    pub(crate) lock_digest: String,
    pub(crate) packages: Vec<LockedPackage>,
    /// Where the view will live. Absolute, and derived exactly as the launcher
    /// derives it, so the form can show the interpreter the server will run on.
    pub(crate) view: PathBuf,
    /// The declaration digest that names the view.
    pub(crate) declaration_digest: String,
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
        let review_root = library_root.clone();
        let (review, existing_digest) = tokio::task::spawn_blocking(move || {
            let review = Library::new(&review_root)
                .review_directory(&id, &reviewed)
                .map_err(plugin_error)?;
            let existing_digest = FilePluginCatalog::new(&review_root)
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
        // A declaration is solved before anybody is asked to approve anything:
        // approving a Plugin whose packages cannot be resolved would be
        // approving an installation that cannot happen.
        let (dependencies, resolved) = self.plugin_dependency_plan(&library_root, &review).await?;
        Ok(PluginInstallPrecondition {
            review,
            directory,
            existing_digest,
            impact,
            dependencies,
            resolved,
        })
    }

    /// Solve one Plugin's declaration against the deployment's authorized sources.
    ///
    /// Fail closed: a Plugin that declares dependencies on a deployment that
    /// authorizes no package installation is refused here, before anything is
    /// written, instead of being installed into a state where its tools can
    /// never start.
    async fn plugin_dependency_plan(
        &self,
        library_root: &Path,
        review: &PluginReview,
    ) -> Result<(Option<PluginDependencyPlan>, Option<ResolvedRequirements>), ApplicationError>
    {
        if review.dependencies.is_empty() {
            return Ok((None, None));
        }
        // Fail closed with a reason the operator can act on: a Plugin whose
        // dependencies cannot be installed must not be installed at all, because
        // its tools could never start.
        let provisioner = self.dependency_provisioner().map_err(|_| {
            ApplicationError::Invalid(format!(
                "Plugin `{}` declares dependencies, but this deployment does not authorize installing them (no ANCHOR_ENVIRONMENT_* configuration)",
                review.id
            ))
        })?;
        let declaration_digest = review.dependencies.declaration_digest();
        let sources = provisioner
            .config
            .sources
            .iter()
            .map(|source| source.as_str())
            .collect::<Vec<_>>();
        let resolved = resolve_requirements(&provisioner.config, None, &review.dependencies.python)
            .await
            .map_err(|error| {
                ApplicationError::Invalid(format!(
                    "Plugin `{}` dependency resolution failed: {error}",
                    review.id
                ))
            })?;
        // An index that answers the package page with HTML resolves fine and
        // fetches never, so that is found now — with the failure the operator can
        // act on, before they are asked to approve the set.
        //
        // The check mirrors the fetch rather than being stricter than it: a
        // package is deliverable as soon as one authorized index answers a
        // PEP 691 page, and a deployment that also authorizes a directory may
        // still deliver a package no index has (the fetch reports that per
        // source).
        let has_directory = provisioner
            .config
            .sources
            .iter()
            .any(|source| matches!(source, PackageSource::Directory(_)));
        for package in &resolved.packages {
            let mut reasons = Vec::new();
            let mut served = false;
            for source in &provisioner.config.sources {
                if let PackageSource::Index(index) = source {
                    match verify_index_json(index, &package.name).await {
                        Ok(()) => {
                            served = true;
                            break;
                        }
                        Err(error) => reasons.push(error.to_string()),
                    }
                }
            }
            if !served && !has_directory {
                return Err(ApplicationError::Invalid(format!(
                    "Plugin `{}` dependency resolution failed: no authorized index serves `{}` as a PEP 691 JSON package page ({})",
                    review.id,
                    package.name,
                    reasons.join("; ")
                )));
            }
        }
        let root = resolved_library_root(library_root);
        let view = plugin_view_path(&root, &review.id, &declaration_digest);
        let plan = PluginDependencyPlan {
            sources,
            interpreter: resolved.interpreter.clone(),
            interpreter_path: resolved.interpreter_path.clone(),
            interpreter_digest: resolved.interpreter_digest.clone(),
            lock_digest: resolved.lock_digest.clone(),
            packages: resolved.packages.clone(),
            view,
            declaration_digest,
        };
        Ok((Some(plan), Some(resolved)))
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
            if bundle.plugins.iter().any(|plugin| plugin == id) {
                graphs.push(name.clone());
            }
        }
        graphs.sort();
        // Upgrading is always allowed (standard invariant 7): a Run uses the
        // *current* Plugin and observes it from its next node call, so there is no
        // stranded-Run question to ask and nothing to confirm.
        Ok(PluginImpact { graphs, coverage })
    }

    /// Publish the reviewed candidate only if nothing moved since the review.
    ///
    /// A confirmation the operator gave for one digest must never install a
    /// different tree, so the candidate is reviewed again and the Library state
    /// is re-read before anything is written.
    /// An upgrade needs no second decision: a Run uses the current Plugin and
    /// takes the new code from its next node call (standard invariants 6 and 7).
    pub(crate) async fn install_plugin_if_unchanged(
        &self,
        library_root: PathBuf,
        precondition: PluginInstallPrecondition,
    ) -> Result<InstallOutcome, ApplicationError> {
        // Repairing the view of an already-published tree publishes nothing, so
        // it is not a replacement.
        let already_installed = precondition.already_installed();
        // One writer for the whole operation: the catalog guard covers the
        // dependency landing as well as the Plugin publication, so two installs
        // cannot interleave their renames.
        let catalog_guard = self.graph_catalog_mutation_guard().await;
        let id = precondition.review.id.clone();
        let directory = precondition.directory.clone();
        let replacing = precondition.replacing() && !already_installed;
        {
            let library_root = library_root.clone();
            let id = id.clone();
            let directory = directory.clone();
            let expected_digest = precondition.review.digest.clone();
            let expected_existing = precondition.existing_digest.clone();
            tokio::task::spawn_blocking(move || {
                let library = Library::new(&library_root);
                let reviewed = library
                    .review_directory(&id, &directory)
                    .map_err(plugin_error)?;
                if reviewed.digest != expected_digest {
                    return Err(ApplicationError::Conflict(
                        "the Plugin candidate changed while the user was deciding; ask again"
                            .into(),
                    ));
                }
                let existing_digest = FilePluginCatalog::new(&library_root)
                    .definition(&id)
                    .ok()
                    .map(|definition| definition.digest);
                if existing_digest != expected_existing {
                    return Err(ApplicationError::Conflict(
                        "the installed Plugin changed while the user was deciding; ask again"
                            .into(),
                    ));
                }
                Ok::<(), ApplicationError>(())
            })
            .await
            .map_err(|_| ApplicationError::Storage("Plugin installer task failed".into()))??;
        }
        // The installation lock is taken *before* the dependencies land and held
        // until the Plugin is published. The two steps are otherwise independent,
        // and in the window between them a concurrent reclamation would see a view
        // whose Plugin is not installed yet and judge it an orphan (the install
        // would then succeed with a missing view). Reclamation takes the same lock
        // without waiting, so this cannot deadlock.
        let lease_root = library_root.clone();
        let lease = tokio::task::spawn_blocking(move || {
            Library::new(&lease_root)
                .install_lease()
                .map_err(plugin_error)
        })
        .await
        .map_err(|_| ApplicationError::Storage("Plugin installer task failed".into()))??;
        // Dependencies land before the Plugin is published: a failed solve,
        // fetch, unpack or verification must leave the Library and every
        // existing view exactly as they were.
        let mut landed = None;
        if let Some(resolved) = precondition.resolved.clone() {
            let provisioner = Arc::clone(self.dependency_provisioner()?);
            let declaration_digest = precondition
                .dependencies
                .as_ref()
                .map(|plan| plan.declaration_digest.clone())
                .ok_or_else(|| {
                    ApplicationError::Storage(
                        "the approved Plugin dependency plan is incomplete".into(),
                    )
                })?;
            let landing_root = library_root.clone();
            landed = Some(
                crate::plugin_dependencies::land(
                    &provisioner,
                    &landing_root,
                    &id,
                    &declaration_digest,
                    &resolved,
                )
                .await
                .map_err(plugin_dependency_error)?,
            );
        }
        if already_installed {
            // The Library already holds this exact tree. The only thing this
            // operation can still add is the view the declaration maps to, which
            // is how a deleted or never-built view is repaired through the one
            // installation gate — the Plugin bytes are not rewritten.
            let root = library_root.clone();
            let id = id.clone();
            return tokio::task::spawn_blocking(move || {
                let _catalog_guard = catalog_guard;
                let catalog = FilePluginCatalog::new(&root);
                let definition = catalog
                    .definition(&id)
                    .map_err(|error| ApplicationError::Storage(error.to_string()))?;
                let directory = catalog
                    .plugin_directory(&id)
                    .map_err(|error| ApplicationError::Storage(error.to_string()))?;
                Ok(InstallOutcome {
                    id,
                    digest: definition.digest,
                    directory,
                })
            })
            .await
            .map_err(|_| ApplicationError::Storage("Plugin installer task failed".into()))?;
        }
        // A view this call created would become an orphan if the Plugin never
        // reached the Library, so it is removed again; a view that was already
        // there is left alone, because a Run may be using it and a retry reuses it.
        let created_view = landed
            .filter(|landed| !landed.reused)
            .map(|landed| landed.view);
        let published = tokio::task::spawn_blocking(move || {
            let _catalog_guard = catalog_guard;
            Library::new(&library_root)
                .install_directory_locked(&id, &directory, replacing, &lease)
                .map_err(plugin_error)
        })
        .await
        .map_err(|_| ApplicationError::Storage("Plugin installer task failed".into()))?;
        match published {
            Ok(outcome) => Ok(outcome),
            Err(error) => {
                if let Some(view) = created_view {
                    let _ = std::fs::remove_dir_all(&view);
                }
                Err(error)
            }
        }
    }
}

/// A dependency failure is the operator's to fix and names what failed, not a
/// transport or storage accident inside the installer.
fn plugin_dependency_error(error: ProvisionError) -> ApplicationError {
    match error {
        ProvisionError::Invalid(reason) | ProvisionError::Failed(reason) => {
            ApplicationError::Invalid(reason)
        }
        ProvisionError::Unconfigured(reason) => ApplicationError::Invalid(reason),
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
            .install_plugin_if_unchanged(library_root.clone(), before.clone())
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
            .install_plugin_if_unchanged(library_root.clone(), precondition)
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
            .install_plugin_if_unchanged(library_root.clone(), precondition)
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
            .install_plugin_if_unchanged(library_root.clone(), first_precondition)
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
            .install_plugin_if_unchanged(library_root.clone(), precondition.clone())
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
            },
        );
        record.plugin_bindings_initialized = true;
        store.save(&record).unwrap();
        record.run_id
    }

    #[tokio::test]
    async fn replacing_a_plugin_takes_effect_without_a_second_decision() {
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
            )
            .await
            .unwrap();
        // A Run that is not terminal binds the Plugin. Upgrading is always
        // allowed (standard invariant 7), so the impact report says nothing about
        // it and the replacement happens.
        let run = unfinished_run(root.path(), "demo");
        assert!(!run.is_empty());

        let next = candidate(root.path(), "second");
        let precondition = application
            .plugin_install_precondition(library_root.clone(), next, "demo".into())
            .await
            .unwrap();
        assert!(precondition.impact.graphs.is_empty());
        application
            .install_plugin_if_unchanged(library_root.clone(), precondition.clone())
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

    /// Only `ANCHOR_RUNNER_BUNDLE_ROOT` is set — the distribution's documented
    /// minimum. The deployment has no separate Library root, so the Graph catalog
    /// (here: the bundle's parent) *is* the Plugin home, and every caller takes that
    /// answer from one place: the API that installs a Plugin and the node call that
    /// runs it cannot disagree.
    ///
    /// A bundle directory itself is never the Library: the Plugin home is
    /// `<bundle parent>/plugins`.
    #[tokio::test]
    async fn a_deployment_with_only_a_bundle_root_has_one_library_root() {
        let root = tempfile::tempdir().unwrap();
        let state = root.path().join("state");
        let bundle = root.path().join("bundles/main");
        fs::create_dir_all(&bundle).unwrap();
        let _guard = crate::PROCESS_ENV_LOCK.lock().await;
        let previous = (
            std::env::var_os("ANCHOR_RUNNER_LIBRARY_ROOT"),
            std::env::var_os("ANCHOR_RUNNER_CATALOG_ROOT"),
            std::env::var_os("ANCHOR_RUNNER_BUNDLE_ROOT"),
        );
        unsafe {
            std::env::remove_var("ANCHOR_RUNNER_LIBRARY_ROOT");
            std::env::remove_var("ANCHOR_RUNNER_CATALOG_ROOT");
            std::env::set_var("ANCHOR_RUNNER_BUNDLE_ROOT", &bundle);
        }
        // The same root the API serves and the resolver binds from.
        let catalog_root = bundle.parent().unwrap();
        let library_root = crate::library_root(catalog_root);
        let application = RunApplication::new(state.clone(), catalog_root.to_path_buf());
        let source = candidate(root.path(), "only-bundle");
        let installed = application
            .install_plugin_if_unchanged(
                library_root.clone(),
                application
                    .plugin_install_precondition(library_root.clone(), source, "demo".into())
                    .await
                    .unwrap(),
            )
            .await
            .unwrap();
        // The Plugin landed in the one Plugin home, not beside the bundle.
        assert_eq!(installed.directory, catalog_root.join("plugins/demo"));
        assert!(!bundle.join("plugins").exists());
        assert!(
            crate::application::graphs::referenced_plugin_bindings(
                &library_root,
                &["demo".to_owned()]
            )
            .is_ok(),
            "the node call resolves the Plugin the API installed"
        );
        let (library, catalog, bundle) = previous;
        unsafe {
            match library {
                Some(value) => std::env::set_var("ANCHOR_RUNNER_LIBRARY_ROOT", value),
                None => std::env::remove_var("ANCHOR_RUNNER_LIBRARY_ROOT"),
            }
            match catalog {
                Some(value) => std::env::set_var("ANCHOR_RUNNER_CATALOG_ROOT", value),
                None => std::env::remove_var("ANCHOR_RUNNER_CATALOG_ROOT"),
            }
            match bundle {
                Some(value) => std::env::set_var("ANCHOR_RUNNER_BUNDLE_ROOT", value),
                None => std::env::remove_var("ANCHOR_RUNNER_BUNDLE_ROOT"),
            }
        }
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
        fs::create_dir_all(&bound).unwrap();
        // The Plugin lives in the Library, not in the bundle.
        fs::create_dir_all(catalog.join("plugins/demo")).unwrap();
        fs::write(
            catalog.join("plugins/demo/plugin.json"),
            json!({"name":"Demo","mcpServers":{"probe":{"command":"bin/run"}}}).to_string(),
        )
        .unwrap();
        FilePluginCatalog::new(&catalog)
            .resolve(&["demo".to_owned()])
            .unwrap();
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
                "plugins": [{"id": "demo"}]
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

    /// A deployment that authorizes no package installation refuses a Plugin
    /// that declares dependencies, before anything is written. Installing it
    /// would produce a Plugin whose tools can never start.
    #[tokio::test]
    async fn a_declaration_on_a_deployment_that_authorizes_no_install_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let library_root = root.path().join("library");
        let application = RunApplication::new(root.path().join("state"), root.path().to_path_buf());
        let source = candidate(root.path(), "dependencies");
        fs::write(
            source.join("plugin.json"),
            json!({
                "name": "Dependent",
                "mcpServers": {"probe": {"command": "${ANCHOR_PLUGIN_VIEW}/bin/python"}},
                "dependencies": {"python": ["idna==3.7"]}
            })
            .to_string(),
        )
        .unwrap();

        let failure = application
            .plugin_install_precondition(library_root.clone(), source, "dependent".into())
            .await
            .unwrap_err();
        assert!(
            matches!(failure, ApplicationError::Invalid(ref reason)
                if reason.contains("does not authorize installing them")),
            "{failure:?}"
        );
        assert!(
            !library_root.join("plugins/dependent").exists(),
            "a refused declaration must not be installed"
        );
        assert!(
            !library_root.join("dependencies").exists(),
            "a refused declaration must not create store or views"
        );
    }

    /// The catalog guard covers the dependency landing as well as the Plugin
    /// publication, so Graph editing waits for the whole install.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_install_holds_the_catalog_guard_for_its_whole_duration() {
        let root = tempfile::tempdir().unwrap();
        let library_root = root.path().join("library");
        let application = RunApplication::new(root.path().join("state"), root.path().to_path_buf());
        let source = candidate(root.path(), "guarded");
        let precondition = application
            .plugin_install_precondition(library_root.clone(), source, "guarded".into())
            .await
            .unwrap();

        let guard = application.graph_catalog_mutation_guard().await;
        let installer = application.clone();
        let installing = tokio::spawn(async move {
            installer
                .install_plugin_if_unchanged(library_root, precondition)
                .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            !installing.is_finished(),
            "the install must wait for the catalog guard this host already holds"
        );
        drop(guard);
        installing.await.unwrap().unwrap();
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
        application
            .install_plugin_if_unchanged(library_root, precondition)
            .await
            .unwrap();
    }
}
