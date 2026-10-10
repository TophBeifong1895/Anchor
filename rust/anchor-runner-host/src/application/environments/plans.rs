//! Publishing one prepared environment directory behind a native approval.

use super::*;
use crate::application::graphs::{EnvironmentReferences, ImpactCoverage};
use std::path::PathBuf;

/// What a human is asked to approve for one environment candidate.
///
/// Frozen in memory before the form is shown and re-checked after the answer:
/// the content identity, the name, and where the name pointed when the human saw
/// it. Nothing here is written down — the ask and the act happen inside one call,
/// so there is no second record to keep consistent with the Library.
#[derive(Debug, Clone)]
pub(crate) struct EnvironmentPlan {
    /// The candidate directory, as the assistant named it inside its workspace.
    pub(crate) directory: PathBuf,
    /// The deployment name the human is asked to point at the content.
    pub(crate) name: String,
    /// The content identity the candidate would publish as.
    pub(crate) digest: String,
    /// What the human is approving: the size and file count of that content.
    pub(crate) bytes: u64,
    pub(crate) files: usize,
    /// What the name pointed at when the form was shown.
    pub(crate) observed_id: Option<String>,
    /// Whether that content still verifies; a name pointing at missing or
    /// tampered content is not "already published".
    pub(crate) observed_present: bool,
    /// The name-table version the human saw; `None` means the name was absent.
    pub(crate) observed_name_version: Option<u64>,
    /// The impact range shown with the form.
    pub(crate) references: Option<EnvironmentReferences>,
}

#[derive(Debug, Clone)]
pub(crate) struct EnvironmentPublishPrecondition {
    pub(crate) plan: EnvironmentPlan,
}

impl EnvironmentPublishPrecondition {
    /// True when the name already points at exactly this content.
    pub(crate) fn already_published(&self) -> bool {
        self.plan.observed_present
            && self.plan.observed_id.as_deref() == Some(self.plan.digest.as_str())
    }

    /// True when the name currently points at something else.
    pub(crate) fn replacing(&self) -> bool {
        self.plan.observed_id.is_some()
    }
}

impl RunApplication {
    /// Freeze what the operator is about to approve for one candidate.
    pub(crate) async fn environment_publish_precondition(
        &self,
        library_root: PathBuf,
        directory: PathBuf,
        name: String,
    ) -> Result<EnvironmentPublishPrecondition, ApplicationError> {
        // The name is validated *before* the form, not after publication:
        // publishing content and only then failing to point a name at it would
        // leave an unnamed tree in the Library behind an approved form.
        if !anchor_runtime::graph::is_environment_name(&name) {
            return Err(ApplicationError::Invalid(
                "environment name must be at most 64 characters of letters, digits, dot, dash and underscore"
                    .to_owned(),
            ));
        }
        // A published environment contributes commands only through `bin/`, so a
        // candidate without one could never be used and would only record a false
        // fact. This is the same rule Run admission enforces, checked before a
        // human is asked to approve anything.
        if !directory.join("bin").is_dir() {
            return Err(ApplicationError::Invalid(
                "the environment candidate has no `bin` directory, so it could not provide any command"
                    .to_owned(),
            ));
        }
        // The same overlap rule `publish` enforces, checked before a human clicks.
        let library = std::fs::canonicalize(&library_root).unwrap_or_else(|_| library_root.clone());
        let candidate = std::fs::canonicalize(&directory).unwrap_or_else(|_| directory.clone());
        if candidate.starts_with(&library) || library.starts_with(&candidate) {
            return Err(ApplicationError::Invalid(
                "the environment candidate must not live inside the Library".to_owned(),
            ));
        }
        // Digesting copies the whole tree, so it must not block a runtime worker.
        let digest_directory = directory.clone();
        let content = tokio::task::spawn_blocking(move || {
            EnvironmentStore::digest_candidate(&digest_directory)
        })
        .await
        .map_err(|_| ApplicationError::Storage("environment candidate task failed".into()))?
        .map_err(|error| environment_error("the environment candidate was rejected", error))?;
        let store = EnvironmentStore::new(&library_root);
        let existing = store
            .names()
            .map_err(|error| environment_error("the environment name table is unavailable", error))?
            .into_iter()
            .find(|entry| entry.name == name);
        let observed_present = match existing.as_ref() {
            // A name that points at this content is only "already published" when
            // the content is still there and still hashes to that identity.
            Some(entry) if entry.id == content.id => store.show(&entry.id).is_ok(),
            _ => false,
        };
        // The human is shown the impact range; it stays in memory with the rest
        // of the precondition.
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
        let plan = EnvironmentPlan {
            directory,
            name,
            references,
            digest: content.id,
            bytes: content.bytes,
            files: content.files,
            observed_id: existing.as_ref().map(|entry| entry.id.clone()),
            observed_present,
            observed_name_version: existing.as_ref().map(|entry| entry.version),
        };
        Ok(EnvironmentPublishPrecondition { plan })
    }

    /// Publish and point the name, or refuse if the approved content moved.
    pub(crate) async fn publish_environment_if_unchanged(
        &self,
        library_root: PathBuf,
        precondition: EnvironmentPublishPrecondition,
    ) -> Result<(String, u64), ApplicationError> {
        let _mutation_guard = self.graph_catalog_mutation_guard().await;
        tokio::task::spawn_blocking(move || {
            let plan = &precondition.plan;
            let store = EnvironmentStore::new(&library_root);
            // The candidate is not re-hashed here: `publish` returns the identity
            // of the copy it actually made, and comparing that with the approved
            // precondition is the guard. Re-hashing first would copy the whole
            // tree twice. The name must still be where the human saw it.
            let current = store
                .names()
                .map_err(|error| {
                    environment_error("the environment name table is unavailable", error)
                })?
                .into_iter()
                .find(|entry| entry.name == plan.name);
            let current_version = current.as_ref().map(|entry| entry.version);
            let current_id = current.as_ref().map(|entry| entry.id.clone());
            if current_version != plan.observed_name_version || current_id != plan.observed_id {
                return Err(ApplicationError::Conflict(
                    "the environment name changed since it was approved".into(),
                ));
            }
            // The identity is computed from publish's own copy and compared
            // *before* anything becomes visible, so an approval can never publish
            // a tree it did not approve — and the tree is copied only once.
            let published = store
                .publish_expecting(&plan.directory, Some(&plan.digest))
                .map_err(|error| match error {
                    EnvironmentError::UnexpectedContent { .. } => ApplicationError::Conflict(
                        "the environment candidate changed since it was approved".into(),
                    ),
                    other => {
                        environment_error("the environment candidate could not be published", other)
                    }
                })?;
            let switch = store
                .set_name(&plan.name, &published.id, plan.observed_name_version)
                .map_err(|error| {
                    environment_error("the environment name could not be switched", error)
                })?;
            Ok((published.id, switch.version))
        })
        .await
        .map_err(|_| ApplicationError::Storage("environment publisher task failed".into()))?
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::*;
    use std::fs;

    #[tokio::test]
    async fn a_candidate_that_could_not_provide_a_command_is_refused_before_the_form() {
        let root = tempfile::tempdir().unwrap();
        let application = application(root.path());
        let library = root.path().join("library");
        // No `bin/`: publishing it would record an environment that provides
        // nothing, which Run admission would refuse later anyway.
        let directory = root.path().join("candidate-empty");
        fs::create_dir_all(directory.join("lib")).unwrap();
        fs::write(directory.join("lib/data.txt"), "data").unwrap();
        let error = application
            .environment_publish_precondition(library, directory, "research".into())
            .await
            .unwrap_err();
        assert!(matches!(error, ApplicationError::Invalid(_)), "{error:?}");
        assert!(
            !root.path().join("state/environment-plans").exists(),
            "no plan is recorded before a human is asked"
        );
    }

    #[tokio::test]
    async fn a_name_pointing_at_missing_content_is_not_already_published() {
        let root = tempfile::tempdir().unwrap();
        let application = application(root.path());
        let library = root.path().join("library");
        let store = EnvironmentStore::new(&library);
        let directory = candidate(root.path(), "one");
        let published = store.publish(&directory).unwrap();
        store.set_name("research", &published.id, None).unwrap();

        // The published tree disappears (same owner, restored write bits first,
        // exactly as an operator pruning a broken environment would).
        force_writable(&library.join("envs").join(&published.id));
        fs::remove_dir_all(library.join("envs").join(&published.id)).unwrap();

        let precondition = application
            .environment_publish_precondition(library.clone(), directory, "research".into())
            .await
            .unwrap();
        assert!(
            !precondition.already_published(),
            "a name whose content is missing must not report as already published"
        );
        // Publishing it again restores the content and keeps the name.
        let (id, version) = application
            .publish_environment_if_unchanged(library.clone(), precondition)
            .await
            .unwrap();
        assert_eq!(id, published.id);
        assert_eq!(version, 2, "the switch is a fresh version of the same name");
        assert!(store.show(&id).is_ok());
    }

    #[tokio::test]
    async fn publishing_an_approved_candidate_points_the_name() {
        let root = tempfile::tempdir().unwrap();
        let application = application(root.path());
        let library = root.path().join("library");
        let directory = candidate(root.path(), "one");

        let precondition = application
            .environment_publish_precondition(library.clone(), directory.clone(), "research".into())
            .await
            .unwrap();
        assert!(!precondition.replacing());
        assert!(!precondition.already_published());
        assert_eq!(precondition.plan.digest.len(), 64);

        let (id, version) = application
            .publish_environment_if_unchanged(library.clone(), precondition.clone())
            .await
            .unwrap();
        assert_eq!(id, precondition.plan.digest);
        assert_eq!(version, 1);
        let names = EnvironmentStore::new(&library).names().unwrap();
        assert_eq!(names.len(), 1);
        assert_eq!(names[0].name, "research");
        assert_eq!(names[0].id, id);

        // Re-publishing the same content is a no-op the caller can short-circuit.
        let again = application
            .environment_publish_precondition(library.clone(), directory, "research".into())
            .await
            .unwrap();
        assert!(again.already_published());
    }

    /// §5.5: the whole publication flow leaves no plan ledger behind.
    #[tokio::test]
    async fn a_successful_publication_writes_no_plan_directory() {
        let root = tempfile::tempdir().unwrap();
        let application = application(root.path());
        let library = root.path().join("library");
        let directory = candidate(root.path(), "one");
        let precondition = application
            .environment_publish_precondition(library.clone(), directory, "research".into())
            .await
            .unwrap();
        application
            .publish_environment_if_unchanged(library, precondition)
            .await
            .unwrap();

        let state = root.path().join("state");
        assert!(
            !state.join("environment-plans").exists(),
            "no publication plan file is written"
        );
        assert!(
            !state.join("environment-provision-plans").exists(),
            "no provision plan file is written"
        );
    }

    #[tokio::test]
    async fn an_approval_binds_to_the_content_and_the_name_the_human_saw() {
        let root = tempfile::tempdir().unwrap();
        let application = application(root.path());
        let library = root.path().join("library");
        let directory = candidate(root.path(), "one");

        // The candidate changed after approval: refuse.
        let precondition = application
            .environment_publish_precondition(library.clone(), directory.clone(), "research".into())
            .await
            .unwrap();
        fs::write(directory.join("marker.txt"), "two").unwrap();
        let error = application
            .publish_environment_if_unchanged(library.clone(), precondition)
            .await
            .unwrap_err();
        assert!(matches!(error, ApplicationError::Conflict(_)), "{error:?}");
        let store = EnvironmentStore::new(&library);
        assert!(store.names().unwrap().is_empty());
        assert!(
            store.list().unwrap().is_empty(),
            "an unapproved candidate must not become a published environment"
        );

        // The name moved after approval: refuse instead of overwriting.
        let directory = candidate(root.path(), "one");
        let precondition = application
            .environment_publish_precondition(library.clone(), directory.clone(), "research".into())
            .await
            .unwrap();
        let other = candidate(root.path(), "other");
        let store = EnvironmentStore::new(&library);
        let published = store.publish(&other).unwrap();
        store.set_name("research", &published.id, None).unwrap();
        let error = application
            .publish_environment_if_unchanged(library.clone(), precondition)
            .await
            .unwrap_err();
        assert!(matches!(error, ApplicationError::Conflict(_)), "{error:?}");
        assert_eq!(store.names().unwrap()[0].id, published.id);
    }

    #[tokio::test]
    async fn replacing_an_existing_name_is_reported_as_such() {
        let root = tempfile::tempdir().unwrap();
        let application = application(root.path());
        let library = root.path().join("library");
        let store = EnvironmentStore::new(&library);
        let first = candidate(root.path(), "one");
        let published = store.publish(&first).unwrap();
        store.set_name("research", &published.id, None).unwrap();

        let second = candidate(root.path(), "two");
        let precondition = application
            .environment_publish_precondition(library.clone(), second, "research".into())
            .await
            .unwrap();
        assert!(precondition.replacing());
        assert!(!precondition.already_published());
        assert_eq!(precondition.plan.observed_name_version, Some(1));
        assert_eq!(
            precondition.plan.observed_id.as_deref(),
            Some(published.id.as_str())
        );

        let (id, version) = application
            .publish_environment_if_unchanged(library, precondition)
            .await
            .unwrap();
        assert_ne!(id, published.id);
        assert_eq!(version, 2, "a switch bumps the name version");
    }
}
