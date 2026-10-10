//! Turning an approved requirement into a published environment.
//!
//! This is the step the environment draft calls the builder. It has two halves
//! with a human in between, and both halves live inside one call:
//!
//! - **before the form**: read the recorded requirement, resolve it against the
//!   deployment's authorized sources, and freeze — interpreter, locked packages
//!   with their digests, the exact `requirements.txt`, and where the name points
//!   right now. A human then approves *that* resolution.
//! - **after approval**: re-check that the name is still where they saw it,
//!   fetch exactly the approved bytes, build offline in the sandbox, verify the
//!   published tree where it now lives, and only then switch the name.
//!
//! The frozen identity is the lock digest, not the finished content: the content
//! digest cannot be known before the build, and pretending otherwise would mean
//! approving something nobody has seen. Nothing is persisted: the approval is not
//! a queue, and a restart replays nothing.

use std::path::PathBuf;
use std::sync::Arc;

use super::*;
use crate::application::graphs::{EnvironmentReferences, ImpactCoverage};
use crate::environment_provision::{
    EnvironmentProvisioner, LockedPackage, PackageSource, ProvisionError, ResolvedRequirements,
    build_environment, fetch_wheelhouse, resolve_requirements, verify_index_json, verify_published,
};

/// What a human is asked to approve for one provisioned environment.
#[derive(Debug, Clone)]
pub(crate) struct EnvironmentProvisionPlan {
    pub(crate) name: String,
    /// The deployment sources the resolution was allowed to use.
    pub(crate) sources: Vec<String>,
    pub(crate) interpreter: String,
    pub(crate) interpreter_path: PathBuf,
    pub(crate) interpreter_digest: String,
    /// The locked set and its identity: the file the build installs, and the
    /// digest that names it.
    pub(crate) lock_digest: String,
    pub(crate) packages: Vec<LockedPackage>,
    /// The impact range shown with the form.
    pub(crate) references: Option<EnvironmentReferences>,
    pub(crate) observed_id: Option<String>,
    pub(crate) observed_name_version: Option<u64>,
}

#[derive(Debug, Clone)]
pub(crate) struct EnvironmentProvisionPrecondition {
    pub(crate) plan: EnvironmentProvisionPlan,
    resolved: ResolvedRequirements,
}

/// What one completed provision produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProvisionedEnvironment {
    pub(crate) id: String,
    pub(crate) name_version: u64,
    pub(crate) files: usize,
    pub(crate) bytes: u64,
}

impl EnvironmentProvisionPrecondition {
    pub(crate) fn replacing(&self) -> bool {
        self.plan.observed_id.is_some()
    }
}

impl RunApplication {
    pub(crate) fn with_environment_provisioner(
        mut self,
        provisioner: Arc<EnvironmentProvisioner>,
    ) -> Self {
        self.environment_provision = Some(provisioner);
        self
    }

    fn environment_provisioner(&self) -> Result<&Arc<EnvironmentProvisioner>, ApplicationError> {
        self.environment_provision.as_ref().ok_or_else(|| {
            ApplicationError::Invalid(
                "this deployment does not authorize environment preparation".to_owned(),
            )
        })
    }

    /// Freeze the resolution a human is about to approve.
    pub(crate) async fn environment_provision_precondition(
        &self,
        library_root: PathBuf,
        name: String,
    ) -> Result<EnvironmentProvisionPrecondition, ApplicationError> {
        let provisioner = Arc::clone(self.environment_provisioner()?);
        if !anchor_runtime::graph::is_environment_name(&name) {
            return Err(ApplicationError::Invalid(
                "environment name must be an identifier of [A-Za-z0-9._-]".to_owned(),
            ));
        }
        // The precondition resolves what was *asked for*; the model cannot add
        // packages to the approval step.
        let requirement = self.environment_requirement(&name)?.ok_or_else(|| {
            ApplicationError::Invalid(format!(
                "no requirement has been recorded for `{name}`; record what it must provide first"
            ))
        })?;
        let ask = requirement.requests.last().cloned().ok_or_else(|| {
            ApplicationError::Invalid(format!("the requirement for `{name}` holds no ask"))
        })?;
        if ask.packages.is_empty() {
            return Err(ApplicationError::Invalid(format!(
                "the requirement for `{name}` names no packages, so there is nothing to build"
            )));
        }

        let sources = provisioner
            .config
            .sources
            .iter()
            .map(|source| source.as_str())
            .collect::<Vec<_>>();
        let resolved = resolve_requirements(
            &provisioner.config,
            ask.interpreter.as_deref(),
            &ask.packages,
        )
        .await
        .map_err(provision_error)?;

        // Every authorized index must be able to deliver what this plan will ask
        // it for, and that is proven now — before a human sees the form. An index
        // that answers the package page with HTML resolves fine and fetches
        // never, so approving the plan first would waste the approval.
        if let Some(package) = resolved.packages.first() {
            for source in &provisioner.config.sources {
                if let PackageSource::Index(index) = source {
                    verify_index_json(index, &package.name)
                        .await
                        .map_err(provision_error)?;
                }
            }
        }

        let store = EnvironmentStore::new(&library_root);
        let existing = store
            .names()
            .map_err(|error| environment_error("the environment name table is unavailable", error))?
            .into_iter()
            .find(|entry| entry.name == name);
        let references = Some(match self.environment_references(&name) {
            Ok(references) => references,
            Err(error) => EnvironmentReferences {
                name: name.clone(),
                nodes: Vec::new(),
                servers: Vec::new(),
                coverage: ImpactCoverage::Unknown {
                    reason: format!("the impact report could not be produced: {error:?}"),
                },
            },
        });
        let plan = EnvironmentProvisionPlan {
            name,
            sources,
            interpreter: resolved.interpreter.clone(),
            interpreter_path: resolved.interpreter_path.clone(),
            interpreter_digest: resolved.interpreter_digest.clone(),
            lock_digest: resolved.lock_digest.clone(),
            packages: resolved.packages.clone(),
            references,
            observed_id: existing.as_ref().map(|entry| entry.id.clone()),
            observed_name_version: existing.as_ref().map(|entry| entry.version),
        };
        Ok(EnvironmentProvisionPrecondition { plan, resolved })
    }

    /// Fetch, build, verify and publish the approved resolution.
    pub(crate) async fn provision_environment_if_unchanged(
        &self,
        library_root: PathBuf,
        precondition: EnvironmentProvisionPrecondition,
    ) -> Result<ProvisionedEnvironment, ApplicationError> {
        let provisioner = Arc::clone(self.environment_provisioner()?);
        // Held for the whole build, so it is deliberately *not* the Graph
        // catalog guard: editing a Graph must stay possible while a dependency
        // set downloads.
        let _build_guard = self.environment_build_guard().await;
        let plan = &precondition.plan;

        // The name must still be where the human saw it. Everything after this
        // is content-addressed, so nothing else about the plan can drift. The
        // name table is read under the Library's own lock, off the async worker.
        let name_root = library_root.clone();
        let name = plan.name.clone();
        let observed_version = plan.observed_name_version;
        let observed_id = plan.observed_id.clone();
        tokio::task::spawn_blocking(move || {
            let current = EnvironmentStore::new(&name_root)
                .names()?
                .into_iter()
                .find(|entry| entry.name == name);
            if current.as_ref().map(|entry| entry.version) != observed_version
                || current.as_ref().map(|entry| entry.id.clone()) != observed_id
            {
                return Err(EnvironmentError::VersionConflict {
                    name: name.clone(),
                    expected: observed_version.unwrap_or(0),
                    actual: current.as_ref().map(|entry| entry.version).unwrap_or(0),
                });
            }
            Ok::<(), EnvironmentError>(())
        })
        .await
        .map_err(|_| ApplicationError::Storage("environment name check task failed".into()))?
        .map_err(|_| {
            ApplicationError::Conflict(
                "the environment name changed since it was approved".to_owned(),
            )
        })?;

        // Fetched artifacts are cached per locked set: the same approved set does
        // not download the same bytes twice, and no fetch ever writes into the
        // staging tree the build then clears.
        let wheelhouse = provisioner
            .config
            .build_root
            .join("wheelhouse")
            .join(&plan.lock_digest);
        let build_wheelhouse = wheelhouse.clone();
        fetch_wheelhouse(&provisioner.config, &plan.packages, &wheelhouse)
            .await
            .map_err(provision_error)?;
        let built = build_environment(
            &provisioner.config,
            &provisioner.sandbox,
            &precondition.resolved,
            &build_wheelhouse,
        )
        .await
        .map_err(provision_error)?;

        let source = built.tree.clone();
        let publish_root = library_root.clone();
        let published = tokio::task::spawn_blocking(move || {
            EnvironmentStore::new(&publish_root).publish_expecting(&source, None)
        })
        .await
        .map_err(|_| ApplicationError::Storage("environment publisher task failed".into()))?
        .map_err(|error| {
            environment_error("the built environment could not be published", error)
        })?;

        // Verified before the name moves: an environment that cannot run where
        // it was published must never become the one a new Run resolves.
        let published_path = EnvironmentStore::new(&library_root).published_path(&published.id);
        if let Err(error) = verify_published(
            &provisioner.config,
            &provisioner.sandbox,
            &published_path,
            &precondition.resolved,
        )
        .await
        {
            return Err(ApplicationError::Invalid(format!(
                "the built environment did not verify, so the name was left alone: {error}"
            )));
        }

        let switch_root = library_root.clone();
        let switch_name = plan.name.clone();
        let switch_id = published.id.clone();
        let version = tokio::task::spawn_blocking(move || {
            EnvironmentStore::new(&switch_root).set_name(&switch_name, &switch_id, observed_version)
        })
        .await
        .map_err(|_| ApplicationError::Storage("environment name switch task failed".into()))?
        .map_err(|error| environment_error("the environment name could not be switched", error))?
        .version;
        Ok(ProvisionedEnvironment {
            id: published.id,
            name_version: version,
            files: built.files,
            bytes: built.bytes,
        })
    }
}

/// Map the builder's own error classes onto the host's.
pub(crate) fn provision_error(error: ProvisionError) -> ApplicationError {
    match error {
        ProvisionError::Invalid(reason) | ProvisionError::Unconfigured(reason) => {
            ApplicationError::Invalid(reason)
        }
        ProvisionError::Failed(reason) => ApplicationError::Storage(reason),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::environment_provision::test_support::*;

    fn provision_application(
        root: &std::path::Path,
        toolchain: &Toolchain,
        wheels: PathBuf,
    ) -> RunApplication {
        let provisioner = crate::environment_provision::EnvironmentProvisioner::new(config(
            root, toolchain, wheels,
        ))
        .expect("the deployment config builds a sandbox");
        crate::application::environments::test_support::application(root)
            .with_environment_provisioner(Arc::new(provisioner))
    }

    async fn ask(application: &RunApplication, library: &std::path::Path) {
        application
            .ask_environment_requirement(
                library,
                "research",
                crate::application::environments::EnvironmentRequirementAsk {
                    requester: crate::application::environments::test_support::requester(),
                    purpose: "analyse documents".into(),
                    interpreter: None,
                    packages: vec!["probepkg==0.1.0".into()],
                    notes: None,
                },
            )
            .unwrap();
    }

    /// The whole product path: a recorded requirement becomes a published
    /// environment, and the name follows only once the content verified.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_approved_requirement_becomes_a_published_environment() {
        let Some(toolchain) = toolchain() else {
            return;
        };
        let root = tempfile::tempdir().unwrap();
        let wheels = root.path().join("wheels");
        std::fs::create_dir_all(&wheels).unwrap();
        fixture_wheel(&wheels, "probepkg", "0.1.0");
        let library = root.path().join("library");
        let application = provision_application(root.path(), &toolchain, wheels);
        ask(&application, &library).await;

        let precondition = application
            .environment_provision_precondition(library.clone(), "research".into())
            .await
            .expect("the recorded requirement resolves");
        assert_eq!(precondition.plan.packages.len(), 1);
        assert!(!precondition.replacing());

        let provisioned = application
            .provision_environment_if_unchanged(library.clone(), precondition.clone())
            .await
            .expect("the approved plan builds and publishes");
        assert_eq!(provisioned.name_version, 1);
        assert!(
            provisioned.files > 0 && provisioned.bytes > 0,
            "{provisioned:?}"
        );
        let store = EnvironmentStore::new(&library);
        let names = store.names().unwrap();
        assert_eq!(names.len(), 1);
        assert_eq!(names[0].id, provisioned.id);
        assert!(
            store.show(&provisioned.id).is_ok(),
            "the name only moved onto verified content"
        );
        assert!(
            !root
                .path()
                .join("state/environment-provision-plans")
                .exists(),
            "the build writes no plan ledger"
        );

        // A second approval of the same requirement sees the name settled.
        let again = application
            .environment_provision_precondition(library, "research".into())
            .await
            .unwrap();
        assert!(again.plan.observed_id.is_some());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_name_that_moved_after_approval_is_refused_without_publishing() {
        let Some(toolchain) = toolchain() else {
            return;
        };
        let root = tempfile::tempdir().unwrap();
        let wheels = root.path().join("wheels");
        std::fs::create_dir_all(&wheels).unwrap();
        fixture_wheel(&wheels, "probepkg", "0.1.0");
        let library = root.path().join("library");
        let application = provision_application(root.path(), &toolchain, wheels);
        ask(&application, &library).await;
        let precondition = application
            .environment_provision_precondition(library.clone(), "research".into())
            .await
            .unwrap();

        // Somebody else points the name somewhere else while the human decides.
        let store = EnvironmentStore::new(&library);
        let other = crate::application::environments::test_support::candidate(root.path(), "other");
        let published = store.publish(&other).unwrap();
        store.set_name("research", &published.id, None).unwrap();

        let error = application
            .provision_environment_if_unchanged(library.clone(), precondition)
            .await
            .unwrap_err();
        assert!(matches!(error, ApplicationError::Conflict(_)), "{error:?}");
        assert_eq!(
            store.names().unwrap()[0].id,
            published.id,
            "the name was not moved by a refused approval"
        );
        assert_eq!(
            store.list().unwrap().len(),
            1,
            "a refused approval publishes nothing new"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_published_tree_that_does_not_verify_leaves_the_name_alone() {
        let Some(toolchain) = toolchain() else {
            return;
        };
        let root = tempfile::tempdir().unwrap();
        let wheels = root.path().join("wheels");
        std::fs::create_dir_all(&wheels).unwrap();
        fixture_wheel(&wheels, "probepkg", "0.1.0");
        let library = root.path().join("library");
        let application = provision_application(root.path(), &toolchain, wheels);
        ask(&application, &library).await;

        let mut precondition = application
            .environment_provision_precondition(library.clone(), "research".into())
            .await
            .unwrap();
        // The published tree is what has to agree with the approved plan. Here it
        // cannot: the plan the human approved names an interpreter the published
        // `bin/python` is not (a changed tree reports exactly this way), so the
        // verification step must refuse before any name moves.
        assert!(!precondition.resolved.interpreter.is_empty());
        precondition.resolved.interpreter = "0.0.0".to_owned();

        let error = application
            .provision_environment_if_unchanged(library.clone(), precondition)
            .await
            .unwrap_err();
        assert!(
            matches!(error, ApplicationError::Invalid(ref reason)
                if reason.contains("did not verify") && reason.contains("left alone")),
            "{error:?}"
        );

        let store = EnvironmentStore::new(&library);
        assert!(
            store.names().unwrap().is_empty(),
            "a tree that failed verification gets no name"
        );
        let retained = store.list().unwrap();
        assert_eq!(
            retained.len(),
            1,
            "the unnamed content is retained for inspection, not silently dropped"
        );
        assert!(store.show(&retained[0].id).is_ok());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_requirement_without_packages_is_refused_before_anything_exists() {
        let Some(toolchain) = toolchain() else {
            return;
        };
        let root = tempfile::tempdir().unwrap();
        let library = root.path().join("library");
        let application =
            provision_application(root.path(), &toolchain, root.path().join("wheels"));
        application
            .ask_environment_requirement(
                &library,
                "research",
                crate::application::environments::EnvironmentRequirementAsk {
                    requester: crate::application::environments::test_support::requester(),
                    purpose: "inspect".into(),
                    interpreter: None,
                    packages: Vec::new(),
                    notes: None,
                },
            )
            .unwrap();
        let error = application
            .environment_provision_precondition(library, "research".into())
            .await
            .unwrap_err();
        assert!(matches!(error, ApplicationError::Invalid(_)), "{error:?}");
        assert!(
            !root
                .path()
                .join("state/environment-provision-plans")
                .exists(),
            "nothing is recorded before a human could be asked"
        );
    }

    /// A deployment that never authorized preparation says so, and records nothing.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_deployment_without_authorized_preparation_refuses_to_build() {
        let root = tempfile::tempdir().unwrap();
        let library = root.path().join("library");
        let application = crate::application::environments::test_support::application(root.path());
        ask(&application, &library).await;
        let error = application
            .environment_provision_precondition(library, "research".into())
            .await
            .unwrap_err();
        assert!(
            matches!(error, ApplicationError::Invalid(ref reason) if reason.contains("does not authorize")),
            "{error:?}"
        );
    }
}
