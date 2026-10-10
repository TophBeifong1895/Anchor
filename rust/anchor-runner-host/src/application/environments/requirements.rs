//! What the assistant needs an environment to provide.

use super::*;

/// A durable *requirement*: what the assistant needs an environment to provide.
///
/// Not a build order: there is no builder yet, so the record is the ask a human
/// fulfils (by publishing content themselves) or declines. It is keyed by name and
/// holds every ask that name received, because one turn's ask must not erase
/// another's. Whether the need has been *provided* is derived at read time from
/// the name table — never stored, because the name can move again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EnvironmentRequirement {
    /// Record format, so a later host can migrate instead of guessing.
    #[serde(default = "environment_requirement_format")]
    pub(crate) format: u32,
    pub(crate) name: String,
    /// Oldest first, bounded.
    pub(crate) requests: Vec<EnvironmentRequirementAsk>,
    /// Asks dropped by the bound, so a truncated record says so.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub(crate) dropped_asks: usize,
    /// What the name pointed at when the *first* ask was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) observed: Option<EnvironmentNameObservation>,
}

fn environment_requirement_format() -> u32 {
    1
}

fn is_zero(value: &usize) -> bool {
    *value == 0
}

/// One ask: who wants what, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EnvironmentRequirementAsk {
    pub(crate) requester: EnvironmentPlanRequester,
    pub(crate) purpose: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) interpreter: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) packages: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) notes: Option<String>,
}

/// The name table's position when an ask was recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EnvironmentNameObservation {
    pub(crate) id: String,
    pub(crate) name_version: u64,
}

/// The requirement read model, tolerant of records a newer host wrote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct EnvironmentRequirements {
    pub(crate) requirements: Vec<EnvironmentRequirement>,
    /// Records this host could not read; a listing must not imply completeness.
    pub(crate) unreadable: usize,
    /// True when there were more records than this projection returns.
    pub(crate) truncated: bool,
}

impl RunApplication {
    /// The recorded ask for one name, if any.
    pub(crate) fn environment_requirement(
        &self,
        name: &str,
    ) -> Result<Option<EnvironmentRequirement>, ApplicationError> {
        read_environment_requirement(&self.environment_requirement_path(name))
    }

    fn environment_requirement_path(&self, name: &str) -> PathBuf {
        self.data_root
            .join("environment-requirements")
            .join(format!("{name}.json"))
    }

    /// Add this turn's ask for one name without dropping other turns' asks.
    pub(crate) fn ask_environment_requirement(
        &self,
        library_root: &std::path::Path,
        name: &str,
        ask: EnvironmentRequirementAsk,
    ) -> Result<EnvironmentRequirement, ApplicationError> {
        // Two turns may ask for the same name at once; the read-modify-write must
        // not lose one of them.
        let _guard = self
            .state_mutations
            .lock()
            .map_err(|_| ApplicationError::Storage("state writer is unavailable".into()))?;
        if !anchor_runtime::graph::is_environment_name(name) {
            return Err(ApplicationError::Invalid(
                "environment name must be an identifier of [A-Za-z0-9._-]".to_owned(),
            ));
        }
        let path = self.environment_requirement_path(name);
        let mut requirement = match read_environment_requirement(&path)? {
            Some(requirement) => requirement,
            None => EnvironmentRequirement {
                format: environment_requirement_format(),
                name: name.to_owned(),
                requests: Vec::new(),
                dropped_asks: 0,
                // The baseline is taken once, when the name is first asked for.
                observed: EnvironmentStore::new(library_root)
                    .names()
                    .map_err(|error| {
                        environment_error("the environment name table is unavailable", error)
                    })?
                    .into_iter()
                    .find(|entry| entry.name == name)
                    .map(|entry| EnvironmentNameObservation {
                        id: entry.id,
                        name_version: entry.version,
                    }),
            },
        };
        // One turn keeps one ask for a name: re-asking updates it.
        requirement.requests.retain(|existing| {
            !(existing.requester.owner == ask.requester.owner
                && existing.requester.session == ask.requester.session
                && existing.requester.turn == ask.requester.turn)
        });
        requirement.requests.push(ask);
        // Bounded: an unbounded file is a file nobody can read. The owner with the
        // most asks loses first, so one turn cannot evict another owner's ask, and
        // whatever is dropped is counted rather than silently forgotten.
        const MAX_ASKS: usize = 20;
        while requirement.requests.len() > MAX_ASKS {
            let mut counts: std::collections::BTreeMap<(String, String, String), usize> =
                std::collections::BTreeMap::new();
            for ask in &requirement.requests {
                *counts
                    .entry((
                        ask.requester.owner.clone(),
                        ask.requester.session.clone(),
                        ask.requester.turn.clone(),
                    ))
                    .or_default() += 1;
            }
            let Some(heaviest) = counts
                .into_iter()
                .max_by_key(|(_, count)| *count)
                .map(|(key, _)| key)
            else {
                break;
            };
            let position = requirement.requests.iter().position(|ask| {
                (
                    ask.requester.owner.clone(),
                    ask.requester.session.clone(),
                    ask.requester.turn.clone(),
                ) == heaviest
            });
            match position {
                Some(position) => {
                    requirement.requests.remove(position);
                    requirement.dropped_asks += 1;
                }
                None => break,
            }
        }
        write_json_durable(&path, &requirement)?;
        Ok(requirement)
    }
}

/// Read one requirement record, or `None` when the name was never asked for.
fn read_environment_requirement(
    path: &std::path::Path,
) -> Result<Option<EnvironmentRequirement>, ApplicationError> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| ApplicationError::Storage(error.to_string())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(ApplicationError::Storage(error.to_string())),
    }
}

/// Every durable requirement, ordered by name.
///
/// Unlike the publish plans, this listing tolerates a record it cannot read: the
/// projection reports how many were skipped instead of failing entirely.
pub(crate) fn list_environment_requirements(
    data_root: &std::path::Path,
) -> Result<EnvironmentRequirements, ApplicationError> {
    let listed =
        list_json_records::<EnvironmentRequirement>(data_root, "environment-requirements")?;
    let mut requirements = listed.records;
    let truncated = bound_projection(&mut requirements, |left, right| left.name.cmp(&right.name));
    Ok(EnvironmentRequirements {
        requirements,
        unreadable: listed.unreadable,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unreadable_record_is_counted_rather_than_hidden() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("state/environment-requirements");
        std::fs::create_dir_all(&directory).unwrap();
        // One good record, one a newer host wrote (unknown field), one garbage.
        std::fs::write(
            directory.join("good.json"),
            r#"{"format":1,"name":"good","requests":[],"dropped_asks":0}"#,
        )
        .unwrap();
        std::fs::write(
            directory.join("newer.json"),
            r#"{"format":1,"name":"newer","requests":[],"dropped_asks":0,"future_field":true}"#,
        )
        .unwrap();
        std::fs::write(directory.join("garbage.json"), "not json").unwrap();
        // A record the store never wrote is not content: only `.json` is read.
        std::fs::write(directory.join("notes.txt"), "not a record").unwrap();

        let listed = list_environment_requirements(&root.path().join("state")).unwrap();
        assert_eq!(listed.requirements.len(), 1, "{listed:?}");
        assert_eq!(listed.requirements[0].name, "good");
        // Two records could not be read, and the listing says so instead of
        // reporting "one need" over a store the host only half understands.
        assert_eq!(listed.unreadable, 2);
        assert!(!listed.truncated);
    }
}
