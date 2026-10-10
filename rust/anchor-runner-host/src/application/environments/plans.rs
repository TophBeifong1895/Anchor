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
    /// Nobody was ever asked, or nobody could answer any more: the Pilot stopped
    /// while the form was open, the form could not be delivered, or the asking
    /// Turn was already over when the host started. Kept apart from `Aborted`
    /// (approved, then not done) and `Refused` (a human answered "no") because
    /// the three are different facts (§D2B).
    Cancelled {
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

    /// The published identity the name already points at, when it points at
    /// exactly this candidate's content.
    ///
    /// `already_published` proves the content is present and matches; the name
    /// table always records the version that goes with an identity, so both are
    /// there when it is.
    pub(crate) fn observed_publication(&self) -> Option<(String, u64)> {
        if !self.already_published() {
            return None;
        }
        Some((
            self.plan.observed_id.clone()?,
            self.plan.observed_name_version?,
        ))
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
                    .map(|plan| Retention {
                        name: plan.name.clone(),
                        identity: plan.digest.clone(),
                        created: plan.created,
                        pending: matches!(plan.status, EnvironmentPlanStatus::Requested),
                        published: match &plan.status {
                            EnvironmentPlanStatus::Published { id, .. } => Some(id.clone()),
                            _ => None,
                        },
                    })
            })?;
        }
        Ok(())
    }

    /// Record a re-call that found the name already pointing at this content.
    ///
    /// The publication really happened — the name resolves to exactly this
    /// candidate — so the record ends as that fact instead of staying a request
    /// nobody answered (§D3A).
    pub(crate) fn record_observed_publication(
        &self,
        precondition: &EnvironmentPublishPrecondition,
    ) -> Result<EnvironmentPlan, ApplicationError> {
        let (id, name_version) = precondition.observed_publication().ok_or_else(|| {
            ApplicationError::Conflict(
                "the environment name table did not confirm what the name points at".to_owned(),
            )
        })?;
        let mut plan = precondition.plan.clone();
        plan.status = EnvironmentPlanStatus::Published { id, name_version };
        self.write_environment_plan(&plan)?;
        Ok(plan)
    }

    /// Settle every `Requested` plan whose asking Turn can no longer be answered.
    ///
    /// Whether a person can still answer is the platform Session's fact, so the
    /// caller supplies it; the host only reads it. A record this host cannot read
    /// is left alone. Returns how many records were settled.
    pub(crate) fn settle_abandoned_environment_plans(
        &self,
        turn_is_running: impl Fn(&EnvironmentPlanRequester) -> Result<bool, ApplicationError>,
    ) -> Result<usize, ApplicationError> {
        settle_json_records(
            &self.data_root,
            "environment-plans",
            |plan: EnvironmentPlan| {
                if !matches!(plan.status, EnvironmentPlanStatus::Requested)
                    || turn_is_running(&plan.requester)?
                {
                    return Ok(None);
                }
                let mut settled = plan;
                settled.status = EnvironmentPlanStatus::Cancelled {
                    reason: ABANDONED_REQUEST_REASON.to_owned(),
                };
                Ok(Some(settled))
            },
        )
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
mod record_compatibility_tests {
    use super::super::EnvironmentProvisionPlan;
    use super::*;

    /// T8: a *compatibility guard*, not a check of this slice's new behaviour —
    /// it also passes before the change, and it only goes red when a later slice
    /// adds a required record field (or drops `#[serde(default)]`).
    ///
    /// A deployment that upgrades must keep reading the records it already
    /// wrote, so every field the code can add stays optional on the wire.
    ///
    /// These literals are the exact shape the previous host wrote: no
    /// `references`, no `created`, and only the four statuses that existed then.
    #[test]
    fn records_written_before_this_slice_still_deserialize() {
        let old_plans = [
            r#"{"requester":{"owner":"local","session":"pilot","turn":"turn-1"},"directory":"/prepared","name":"research","digest":"aa","bytes":1,"files":1,"observed_id":null,"observed_present":false,"observed_name_version":null,"status":{"status":"requested"}}"#,
            r#"{"requester":{"owner":"local","session":"pilot","turn":"turn-1"},"directory":"/prepared","name":"research","digest":"aa","bytes":1,"files":1,"observed_id":"aa","observed_present":true,"observed_name_version":1,"status":{"status":"published","id":"aa","name_version":1}}"#,
            r#"{"requester":{"owner":"local","session":"pilot","turn":"turn-1"},"directory":"/prepared","name":"research","digest":"aa","bytes":1,"files":1,"observed_id":null,"observed_present":false,"observed_name_version":null,"status":{"status":"refused"}}"#,
            r#"{"requester":{"owner":"local","session":"pilot","turn":"turn-1"},"directory":"/prepared","name":"research","digest":"aa","bytes":1,"files":1,"observed_id":null,"observed_present":false,"observed_name_version":null,"status":{"status":"aborted","reason":"the candidate changed"}}"#,
        ];
        for old in old_plans {
            let plan: EnvironmentPlan = serde_json::from_str(old)
                .unwrap_or_else(|error| panic!("old plan record no longer reads: {error}: {old}"));
            assert_eq!(plan.created, 0, "an absent creation time sorts oldest");
            assert_eq!(plan.references, None);
        }

        let old_provisions = [
            r#"{"requester":{"owner":"local","session":"pilot","turn":"turn-1"},"name":"research","ask":{"requester":{"owner":"local","session":"pilot","turn":"turn-1"},"purpose":"analyse documents","packages":["pandas==2.2.0"]},"sources":[],"interpreter":"3.12.13","interpreter_path":"/usr/bin/python3.12","interpreter_prefix":"/usr","interpreter_digest":"aa","lock_digest":"bb","lock":"","packages":[],"observed_id":null,"observed_present":false,"observed_name_version":null,"status":{"status":"requested"}}"#,
            r#"{"requester":{"owner":"local","session":"pilot","turn":"turn-1"},"name":"research","ask":{"requester":{"owner":"local","session":"pilot","turn":"turn-1"},"purpose":"analyse documents","packages":["pandas==2.2.0"]},"sources":[],"interpreter":"3.12.13","interpreter_path":"/usr/bin/python3.12","interpreter_prefix":"/usr","interpreter_digest":"aa","lock_digest":"bb","lock":"","packages":[],"observed_id":null,"observed_present":false,"observed_name_version":null,"status":{"status":"refused"}}"#,
            r#"{"requester":{"owner":"local","session":"pilot","turn":"turn-1"},"name":"research","ask":{"requester":{"owner":"local","session":"pilot","turn":"turn-1"},"purpose":"analyse documents","packages":["pandas==2.2.0"]},"sources":[],"interpreter":"3.12.13","interpreter_path":"/usr/bin/python3.12","interpreter_prefix":"/usr","interpreter_digest":"aa","lock_digest":"bb","lock":"","packages":[],"observed_id":null,"observed_present":false,"observed_name_version":null,"status":{"status":"provisioned","id":"cc","name_version":1,"files":2,"bytes":3}}"#,
            r#"{"requester":{"owner":"local","session":"pilot","turn":"turn-1"},"name":"research","ask":{"requester":{"owner":"local","session":"pilot","turn":"turn-1"},"purpose":"analyse documents","packages":["pandas==2.2.0"]},"sources":[],"interpreter":"3.12.13","interpreter_path":"/usr/bin/python3.12","interpreter_prefix":"/usr","interpreter_digest":"aa","lock_digest":"bb","lock":"","packages":[],"observed_id":null,"observed_present":false,"observed_name_version":null,"status":{"status":"aborted","reason":"the fetch failed"}}"#,
        ];
        for old in old_provisions {
            let plan: EnvironmentProvisionPlan = serde_json::from_str(old)
                .unwrap_or_else(|error| panic!("old provision record no longer reads: {error}"));
            assert_eq!(plan.created, 0);
            assert_eq!(plan.references, None);
        }
    }

    /// The new terminal state is a durable fact like the others: it must survive
    /// a write/read round trip in both record families.
    #[test]
    fn the_cancelled_terminal_state_round_trips_in_both_families() {
        let cancelled_plan = EnvironmentPlan {
            requester: crate::application::environments::test_support::requester(),
            directory: PathBuf::from("/prepared"),
            name: "research".into(),
            digest: "a".repeat(64),
            bytes: 1,
            files: 1,
            observed_id: None,
            observed_present: false,
            observed_name_version: None,
            references: None,
            status: EnvironmentPlanStatus::Cancelled {
                reason: "nobody answered".into(),
            },
            created: 7,
        };
        let bytes = serde_json::to_vec(&cancelled_plan).unwrap();
        assert_eq!(
            serde_json::from_slice::<EnvironmentPlan>(&bytes).unwrap(),
            cancelled_plan
        );
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["status"]["status"], "cancelled");
        assert_eq!(value["status"]["reason"], "nobody answered");
    }
}

#[cfg(test)]
mod retention_tests {
    use super::super::test_support::*;
    use super::*;

    /// One durable record, as the approval paths write it.
    ///
    /// `identity` is the candidate the attempt was about and `turn` is which
    /// attempt this is: two turns asking about one candidate under one name are
    /// two records that share a name *and* an identity, which is exactly what
    /// retention has to collapse.
    fn plan(
        name: &str,
        identity: &str,
        turn: &str,
        status: EnvironmentPlanStatus,
        created: u64,
    ) -> EnvironmentPlan {
        EnvironmentPlan {
            requester: EnvironmentPlanRequester {
                owner: "local".into(),
                session: "pilot".into(),
                turn: turn.into(),
            },
            directory: PathBuf::from("/prepared"),
            name: name.into(),
            // Padded to the 64-hex shape a content digest has. The fill is a
            // letter so that `other-1` and `other-10` stay different identities.
            digest: format!("{identity:x<64}").chars().take(64).collect(),
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

    /// T4: a model that keeps asking about one candidate and never gets an
    /// answer cannot grow the directory without bound, and each deletion is
    /// reported as `pruned` instead of being silent.
    #[test]
    fn repeated_cancellations_of_one_candidate_are_bounded_and_reported_as_pruned() {
        let root = tempfile::tempdir().unwrap();
        let application = application(root.path());
        let attempts = 12u64;
        for index in 0..attempts {
            application
                .write_environment_plan(&plan(
                    "research",
                    "candidate",
                    &format!("turn-{index}"),
                    EnvironmentPlanStatus::Cancelled {
                        reason: "the Pilot stopped".into(),
                    },
                    100 + index,
                ))
                .unwrap();
        }

        let listed = list_environment_plans(&root.path().join("state")).unwrap();
        assert_eq!(
            listed.records.len(),
            1,
            "one candidate keeps only its newest terminal state: {:?}",
            listed.records
        );
        assert!(matches!(
            listed.records[0].status,
            EnvironmentPlanStatus::Cancelled { .. }
        ));
        assert_eq!(listed.records[0].created, 100 + attempts - 1);
        assert_eq!(listed.pruned, attempts as usize - 1);
    }

    /// I3: a pending ask is a question a human may still answer, so retention
    /// never drops it — not even when it is the oldest record of its candidate.
    #[test]
    fn a_pending_request_is_never_pruned() {
        let root = tempfile::tempdir().unwrap();
        let application = application(root.path());
        application
            .write_environment_plan(&plan(
                "research",
                "candidate",
                "turn-pending",
                EnvironmentPlanStatus::Requested,
                1,
            ))
            .unwrap();
        for index in 0..5u64 {
            application
                .write_environment_plan(&plan(
                    "research",
                    "candidate",
                    &format!("turn-{index}"),
                    EnvironmentPlanStatus::Refused,
                    100 + index,
                ))
                .unwrap();
        }

        let listed = list_environment_plans(&root.path().join("state")).unwrap();
        assert_eq!(
            listed
                .records
                .iter()
                .filter(|plan| matches!(plan.status, EnvironmentPlanStatus::Requested))
                .count(),
            1,
            "the unanswered question is still there: {:?}",
            listed.records
        );
        assert_eq!(listed.records.len(), 2, "{:?}", listed.records);
    }

    /// T5/I3: the record a name may currently resolve to is not provably
    /// redundant, so retention keeps it however many newer terminal records pile
    /// up beside it — and it keeps records of *other* candidates too, because a
    /// newer outcome of a different candidate proves nothing about this one.
    #[test]
    fn the_publication_a_name_currently_points_at_survives_retention() {
        let root = tempfile::tempdir().unwrap();
        let application = application(root.path());
        let published_id = "a".repeat(64);
        application
            .write_environment_plan(&plan(
                "research",
                "published-candidate",
                "turn-published",
                EnvironmentPlanStatus::Published {
                    id: published_id.clone(),
                    name_version: 1,
                },
                1,
            ))
            .unwrap();
        for index in 0..25u64 {
            application
                .write_environment_plan(&plan(
                    "research",
                    &format!("other-{index}"),
                    &format!("turn-{index}"),
                    EnvironmentPlanStatus::Refused,
                    100 + index,
                ))
                .unwrap();
        }

        let listed = list_environment_plans(&root.path().join("state")).unwrap();
        assert!(
            listed.records.iter().any(|plan| plan.status
                == EnvironmentPlanStatus::Published {
                    id: published_id.clone(),
                    name_version: 1,
                }),
            "the record the name points through must survive: {:?}",
            listed.records
        );
        assert_eq!(
            listed.pruned, 0,
            "records of different candidates are not provably redundant"
        );
    }

    /// Records of *different* candidates say nothing about each other, so the
    /// new policy deletes none of them: only a newer terminal state of the same
    /// name *and* the same candidate is provably redundant (D1B).
    #[test]
    fn records_of_different_candidates_are_never_pruned() {
        let root = tempfile::tempdir().unwrap();
        let application = application(root.path());
        for index in 0..3u64 {
            application
                .write_environment_plan(&plan(
                    "research",
                    &format!("candidate-{index}"),
                    &format!("turn-{index}"),
                    EnvironmentPlanStatus::Refused,
                    index,
                ))
                .unwrap();
        }

        let listed = list_environment_plans(&root.path().join("state")).unwrap();
        assert_eq!(listed.records.len(), 3, "{:?}", listed.records);
        assert_eq!(listed.pruned, 0);
    }

    /// T6/I3: retention must not delete what it does not understand, and a
    /// listing must say that it could not read it.
    #[test]
    fn a_record_retention_cannot_read_is_counted_and_never_deleted() {
        let root = tempfile::tempdir().unwrap();
        let application = application(root.path());
        let broken = root.path().join("state/environment-plans/broken.json");
        std::fs::create_dir_all(broken.parent().unwrap()).unwrap();
        std::fs::write(&broken, "{ this is not a record").unwrap();
        for index in 0..5u64 {
            application
                .write_environment_plan(&plan(
                    "research",
                    "candidate",
                    &format!("turn-{index}"),
                    EnvironmentPlanStatus::Cancelled {
                        reason: "the Pilot stopped".into(),
                    },
                    100 + index,
                ))
                .unwrap();
        }

        assert!(
            broken.is_file(),
            "an unreadable record is left exactly where it was"
        );
        let listed = list_environment_plans(&root.path().join("state")).unwrap();
        assert_eq!(listed.unreadable, 1);
        assert_eq!(listed.records.len(), 1, "{:?}", listed.records);
        assert_eq!(listed.pruned, 4);
    }
}
