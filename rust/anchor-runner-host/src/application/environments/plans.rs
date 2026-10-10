//! Publishing one prepared environment directory behind a native approval.

use super::*;
use crate::application::graphs::{EnvironmentReferences, ImpactCoverage};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum EnvironmentPlanStatus {
    Requested,
    Published {
        id: String,
        name_version: u64,
    },
    Refused,
    /// The human approved, but the act did not complete: the plan must not read
    /// as an unanswered request.
    Aborted {
        reason: String,
    },
}

/// What a human is asked to approve for one environment candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EnvironmentPlan {
    pub(crate) requester: EnvironmentPlanRequester,
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
    /// The impact range shown with the form, so the record can prove what the
    /// human was told. Absent in records written before this field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) references: Option<EnvironmentReferences>,
    pub(crate) status: EnvironmentPlanStatus,
    /// When this attempt was first recorded. Zero in records written before the
    /// field existed, which sorts them oldest.
    #[serde(default)]
    pub(crate) created: u64,
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

impl EnvironmentPlan {
    /// The identity of one approval attempt.
    ///
    /// Content identity plus name is *not* unique: two turns may ask to publish
    /// the same candidate under the same name, and the second must not overwrite
    /// the first one's outcome. The asking turn is part of the identity, so each
    /// attempt keeps its own durable record and its own terminal state.
    pub(crate) fn key(&self) -> String {
        format!(
            "{}-{}-{}-{}",
            self.digest, self.name, self.requester.session, self.requester.turn
        )
    }
}

impl RunApplication {
    fn environment_plan_path(&self, key: &str) -> PathBuf {
        self.data_root
            .join("environment-plans")
            .join(format!("{key}.json"))
    }

    /// Record the plan before a human is asked, so what they approved is durable.
    pub(crate) fn write_environment_plan(
        &self,
        plan: &EnvironmentPlan,
    ) -> Result<(), ApplicationError> {
        let path = self.environment_plan_path(&plan.key());
        let mut plan = plan.clone();
        // Re-writing a plan keeps its original creation time, so retention still
        // sees the attempt that was actually approved.
        if plan.created == 0 {
            plan.created = read_created(&path)
                .unwrap_or_else(|| crate::application::metadata::now_nanos() as u64);
        }
        write_json_durable(&path, &plan)?;
        if !matches!(plan.status, EnvironmentPlanStatus::Requested) {
            prune_environment_records(&self.data_root, "environment-plans", |bytes| {
                serde_json::from_slice::<EnvironmentPlan>(bytes)
                    .ok()
                    .map(|plan| {
                        (
                            plan.name.clone(),
                            plan.created,
                            matches!(plan.status, EnvironmentPlanStatus::Requested),
                        )
                    })
            })?;
        }
        Ok(())
    }

    /// Freeze what the operator is about to approve for one candidate.
    pub(crate) async fn environment_publish_precondition(
        &self,
        library_root: PathBuf,
        directory: PathBuf,
        name: String,
        requester: EnvironmentPlanRequester,
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
        // The human is shown the impact range, and the record keeps it. A report
        // that could not be produced is recorded as exactly that: `None` would be
        // indistinguishable from a record written before the field existed.
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
            requester,
            directory,
            name,
            references,
            digest: content.id,
            bytes: content.bytes,
            files: content.files,
            observed_id: existing.as_ref().map(|entry| entry.id.clone()),
            observed_present,
            observed_name_version: existing.as_ref().map(|entry| entry.version),
            status: EnvironmentPlanStatus::Requested,
            created: 0,
        };
        self.write_environment_plan(&plan)?;
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
            // plan is the guard. Re-hashing first would copy the whole tree twice.
            // The name must still be where the human saw it.
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

/// Every durable plan, ordered by name and content identity: what was requested
/// and how it ended.
///
/// This is the read model the approval record needs; without it a published
/// environment and an unanswered request are indistinguishable to an operator.
pub(crate) fn list_environment_plans(
    data_root: &std::path::Path,
) -> Result<RecordList<EnvironmentPlan>, ApplicationError> {
    let mut listed = list_json_records::<EnvironmentPlan>(data_root, "environment-plans")?;
    // Bounded after sorting: which plans a listing shows must not depend on the
    // order the filesystem happens to return.
    listed.truncated = bound_projection(&mut listed.records, |left, right| {
        (left.name.as_str(), left.digest.as_str())
            .cmp(&(right.name.as_str(), right.digest.as_str()))
    });
    Ok(listed)
}

#[cfg(test)]
pub(crate) fn read_environment_plan(
    data_root: &std::path::Path,
    plan: &EnvironmentPlan,
) -> Result<Option<EnvironmentPlan>, ApplicationError> {
    let path = data_root
        .join("environment-plans")
        .join(format!("{}.json", plan.key()));
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| ApplicationError::Storage(error.to_string())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(ApplicationError::Storage(error.to_string())),
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
            .environment_publish_precondition(library, directory, "research".into(), requester())
            .await
            .unwrap_err();
        assert!(matches!(error, ApplicationError::Invalid(_)), "{error:?}");
        assert!(
            list_environment_plans(&root.path().join("state"))
                .unwrap()
                .records
                .is_empty(),
            "no plan is recorded before a human is asked"
        );
    }

    #[tokio::test]
    async fn the_read_model_shows_how_each_request_ended() {
        let root = tempfile::tempdir().unwrap();
        let application = application(root.path());
        let library = root.path().join("library");
        let data_root = root.path().join("state");

        // Requested, then published.
        let directory = candidate(root.path(), "one");
        let precondition = application
            .environment_publish_precondition(
                library.clone(),
                directory,
                "research".into(),
                requester(),
            )
            .await
            .unwrap();
        let (id, version) = application
            .publish_environment_if_unchanged(library.clone(), precondition.clone())
            .await
            .unwrap();
        let mut published = precondition.plan.clone();
        published.status = EnvironmentPlanStatus::Published {
            id: id.clone(),
            name_version: version,
        };
        application.write_environment_plan(&published).unwrap();

        // A second request that the human refused.
        let refused_directory = candidate(root.path(), "two");
        let refused = application
            .environment_publish_precondition(
                library,
                refused_directory,
                "other".into(),
                requester(),
            )
            .await
            .unwrap();
        let mut refused_plan = refused.plan.clone();
        refused_plan.status = EnvironmentPlanStatus::Refused;
        application.write_environment_plan(&refused_plan).unwrap();

        let plans = list_environment_plans(&data_root).unwrap().records;
        assert_eq!(plans.len(), 2);
        let published = plans
            .iter()
            .find(|plan| plan.name == "research")
            .expect("the published plan is listed");
        assert_eq!(
            published.status,
            EnvironmentPlanStatus::Published {
                id,
                name_version: 1
            }
        );
        assert_eq!(
            published.files, 2,
            "the projection carries what was approved"
        );
        let refused = plans
            .iter()
            .find(|plan| plan.name == "other")
            .expect("the refused plan is listed");
        assert_eq!(refused.status, EnvironmentPlanStatus::Refused);
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
            .environment_publish_precondition(
                library.clone(),
                directory,
                "research".into(),
                requester(),
            )
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

    /// Published trees are read-only; a test that prunes one needs the bits back.

    #[tokio::test]
    async fn publishing_an_approved_candidate_records_the_plan_and_points_the_name() {
        let root = tempfile::tempdir().unwrap();
        let application = application(root.path());
        let library = root.path().join("library");
        let directory = candidate(root.path(), "one");

        let precondition = application
            .environment_publish_precondition(
                library.clone(),
                directory.clone(),
                "research".into(),
                requester(),
            )
            .await
            .unwrap();
        assert!(!precondition.replacing());
        assert!(!precondition.already_published());
        let plan = read_environment_plan(&root.path().join("state"), &precondition.plan)
            .unwrap()
            .expect("the plan is durable before the human is asked");
        assert_eq!(plan.status, EnvironmentPlanStatus::Requested);
        assert_eq!(plan.requester.turn, "turn-1");

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
            .environment_publish_precondition(
                library.clone(),
                directory,
                "research".into(),
                requester(),
            )
            .await
            .unwrap();
        assert!(again.already_published());
    }

    #[tokio::test]
    async fn an_approval_binds_to_the_content_and_the_name_the_human_saw() {
        let root = tempfile::tempdir().unwrap();
        let application = application(root.path());
        let library = root.path().join("library");
        let directory = candidate(root.path(), "one");

        // The candidate changed after approval: refuse.
        let precondition = application
            .environment_publish_precondition(
                library.clone(),
                directory.clone(),
                "research".into(),
                requester(),
            )
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
            .environment_publish_precondition(
                library.clone(),
                directory.clone(),
                "research".into(),
                requester(),
            )
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
            .environment_publish_precondition(
                library.clone(),
                second,
                "research".into(),
                requester(),
            )
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

#[cfg(test)]
mod retention_tests {
    use super::*;
    use crate::application::environments::test_support::*;

    /// Each approval attempt has its own identity, which is why records
    /// accumulate at all: a new digest (or turn) is a new file.
    fn plan(name: &str, key: &str, status: EnvironmentPlanStatus, created: u64) -> EnvironmentPlan {
        EnvironmentPlan {
            requester: requester(),
            directory: PathBuf::from("/prepared"),
            name: name.into(),
            // A distinct identity per attempt, padded to the 64-hex shape the
            // record validator expects.
            digest: format!("{key:0<64}").chars().take(64).collect(),
            bytes: 1,
            files: 1,
            observed_id: None,
            observed_present: false,
            observed_name_version: None,
            references: None,
            status,
            created,
        }
    }

    #[test]
    fn one_name_keeps_its_newest_terminal_plans_and_never_drops_a_pending_one() {
        let root = tempfile::tempdir().unwrap();
        let application = application(root.path());
        // A pending ask that a human may still answer, recorded first.
        let pending = plan("research", "a", EnvironmentPlanStatus::Requested, 1);
        application.write_environment_plan(&pending).unwrap();

        let total = crate::application::environments::ENVIRONMENT_RECORDS_PER_NAME + 5;
        for index in 0..total {
            let created = 100 + index as u64;
            application
                .write_environment_plan(&plan(
                    "research",
                    &format!("{index:064x}").replace('0', "b"),
                    EnvironmentPlanStatus::Published {
                        id: "c".repeat(64),
                        name_version: index as u64 + 1,
                    },
                    created,
                ))
                .unwrap();
        }

        let listed = list_environment_plans(&root.path().join("state")).unwrap();
        let pending_now = listed
            .records
            .iter()
            .filter(|plan| matches!(plan.status, EnvironmentPlanStatus::Requested))
            .count();
        let terminal = listed.records.len() - pending_now;
        assert_eq!(pending_now, 1, "a pending record is never pruned");
        // The bound counts the pending record too: a name keeps at most this
        // many records, and the pending one is never the one that goes.
        assert_eq!(
            terminal,
            crate::application::environments::ENVIRONMENT_RECORDS_PER_NAME - 1,
            "{:?}",
            listed.records
        );
        // The newest survive, the oldest are the ones that went.
        let mut created = listed
            .records
            .iter()
            .filter(|plan| !matches!(plan.status, EnvironmentPlanStatus::Requested))
            .map(|plan| plan.created)
            .collect::<Vec<_>>();
        created.sort();
        let kept = crate::application::environments::ENVIRONMENT_RECORDS_PER_NAME - 1;
        assert_eq!(created.first().copied(), Some(100 + (total - kept) as u64));
        assert_eq!(created.last().copied(), Some(100 + total as u64 - 1));
    }

    #[test]
    fn a_bound_that_nothing_reaches_removes_nothing() {
        let root = tempfile::tempdir().unwrap();
        let application = application(root.path());
        for index in 0..3 {
            application
                .write_environment_plan(&plan(
                    "research",
                    &format!("d{index}"),
                    EnvironmentPlanStatus::Refused,
                    index,
                ))
                .unwrap();
        }
        let listed = list_environment_plans(&root.path().join("state")).unwrap();
        assert_eq!(listed.records.len(), 3);
    }
}
