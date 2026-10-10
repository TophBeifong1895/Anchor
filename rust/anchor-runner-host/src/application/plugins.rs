use super::{ApplicationError, RunApplication};
use anchor_graph_host::FilePluginCatalog;
use anchor_library::{
    Checkout, InstallError, InstallOutcome, InstallRequest, Library, PluginReview,
};
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
        tokio::task::spawn_blocking(move || {
            let review = Library::new(&library_root)
                .review_directory(&id, &directory)
                .map_err(plugin_error)?;
            let existing_digest = FilePluginCatalog::new(&library_root)
                .definition(&id)
                .ok()
                .map(|definition| definition.digest);
            Ok(PluginInstallPrecondition {
                review,
                directory,
                existing_digest,
            })
        })
        .await
        .map_err(|_| ApplicationError::Storage("Plugin review task failed".into()))?
    }

    /// Publish the reviewed candidate only if nothing moved since the review.
    ///
    /// A confirmation the operator gave for one digest must never install a
    /// different tree, so the candidate is reviewed again and the Library state
    /// is re-read before anything is written.
    pub(crate) async fn install_plugin_if_unchanged(
        &self,
        library_root: PathBuf,
        precondition: PluginInstallPrecondition,
    ) -> Result<InstallOutcome, ApplicationError> {
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
