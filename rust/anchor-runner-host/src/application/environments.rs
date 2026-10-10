//! Durable plans for publishing a dependency environment.
//!
//! This mirrors the Plugin install flow: the assistant may only name a candidate
//! directory inside its own workspace and the deployment name it wants; the human
//! approves the exact content in a native form; the host re-checks the plan after
//! the answer arrives, so an approval binds to the content that was shown.
//!
//! The plan record is written *before* the form and finalized after the act. It
//! is deliberately not a queue: an unanswered plan stays `requested` and is never
//! replayed automatically, which is the repository's rule for unfinished
//! questions.

use super::{ApplicationError, RunApplication};
use anchor_library::{EnvironmentError, EnvironmentStore};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EnvironmentPlanRequester {
    pub(crate) owner: String,
    pub(crate) session: String,
    pub(crate) turn: String,
}

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
        let path = self.environment_plan_path(&format!("{}-{}", plan.digest, plan.name));
        write_json_durable(&path, plan)
    }

    /// Freeze what the operator is about to approve for one candidate.
    pub(crate) async fn environment_publish_precondition(
        &self,
        library_root: PathBuf,
        directory: PathBuf,
        name: String,
        requester: EnvironmentPlanRequester,
    ) -> Result<EnvironmentPublishPrecondition, ApplicationError> {
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
        // The human is shown the impact range, and the record keeps it.
        let references = self.environment_references(&library_root, &name).ok();
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

fn environment_error(context: &str, error: EnvironmentError) -> ApplicationError {
    match error {
        EnvironmentError::VersionConflict { .. } => {
            ApplicationError::Conflict(format!("{context}: {error}"))
        }
        EnvironmentError::Io(_) | EnvironmentError::PublicationUncertain(_) => {
            ApplicationError::Storage(format!("{context}: {error}"))
        }
        _ => ApplicationError::Invalid(format!("{context}: {error}")),
    }
}

impl RunApplication {
    /// Whether this assistant's own Graph — or any Graph it calls — uses one
    /// environment name, on a node or on a bound Plugin server.
    ///
    /// The self-guard has to follow `Op.call`: the assistant may author a called
    /// Graph, so checking only its own top-level bundle would let it choose the
    /// dependency content of a Graph it then runs. An unreadable Graph refuses
    /// (fail closed) rather than reporting "no".
    pub(crate) fn assistant_uses_environment_name(
        &self,
        graph_name: &str,
        name: &str,
    ) -> Result<bool, ApplicationError> {
        const MAX_GRAPHS: usize = 50;
        let mut pending = vec![graph_name.to_owned()];
        let mut visited = std::collections::BTreeSet::new();
        while let Some(graph) = pending.pop() {
            if !visited.insert(graph.clone()) {
                continue;
            }
            if visited.len() > MAX_GRAPHS {
                return Err(ApplicationError::Invalid(
                    "the assistant Graph calls too many Graphs to verify its own dependencies"
                        .to_owned(),
                ));
            }
            let (_, bundle) = self.load_graph(&graph).map_err(|error| {
                ApplicationError::Invalid(format!(
                    "Graph `{graph}` could not be read to verify the assistant's own dependencies: {error:?}"
                ))
            })?;
            for node in &bundle.snapshot.nodes {
                if node.environment.as_deref() == Some(name) {
                    return Ok(true);
                }
                for plugin in &node.plugins {
                    let Some(binding) = bundle.plugins.iter().find(|binding| &binding.id == plugin)
                    else {
                        continue;
                    };
                    if binding
                        .environments
                        .values()
                        .any(|declared| declared == name)
                    {
                        return Ok(true);
                    }
                }
                // Follow Op.call edges.
                if let Some(op) = node.op.as_ref().and_then(|op| bundle.snapshot.ops.get(op))
                    && let Some(callee) = op
                        .get("call")
                        .and_then(|call| call.get("graph"))
                        .and_then(serde_json::Value::as_str)
                {
                    pending.push(callee.to_owned());
                }
            }
        }
        Ok(false)
    }
}

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
}

/// Write one durable JSON record: durable directory chain, fsynced file and
/// directory, unique temporary name so two writers cannot rename over each other.
fn write_json_durable(
    path: &std::path::Path,
    value: &impl Serialize,
) -> Result<(), ApplicationError> {
    let directory = path
        .parent()
        .ok_or_else(|| ApplicationError::Storage("record path is invalid".into()))?;
    // The crate's helper fsyncs the chain that makes the directory reachable.
    crate::create_durable_directory(directory)
        .map_err(|error| ApplicationError::Storage(error.to_string()))?;
    let bytes =
        serde_json::to_vec(value).map_err(|error| ApplicationError::Storage(error.to_string()))?;
    let temporary = directory.join(format!(
        ".{}-{}.tmp",
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "record".to_owned()),
        crate::application::metadata::now_nanos()
    ));
    {
        use std::io::Write;
        let mut file = std::fs::File::create(&temporary)
            .map_err(|error| ApplicationError::Storage(error.to_string()))?;
        file.write_all(&bytes)
            .map_err(|error| ApplicationError::Storage(error.to_string()))?;
        file.sync_all()
            .map_err(|error| ApplicationError::Storage(error.to_string()))?;
    }
    std::fs::rename(&temporary, path)
        .map_err(|error| ApplicationError::Storage(error.to_string()))?;
    std::fs::File::open(directory)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| ApplicationError::Storage(error.to_string()))?;
    Ok(())
}

impl RunApplication {
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
    let directory = data_root.join("environment-requirements");
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(EnvironmentRequirements {
                requirements: Vec::new(),
                unreadable: 0,
            });
        }
        Err(error) => return Err(ApplicationError::Storage(error.to_string())),
    };
    let mut requirements = Vec::new();
    let mut unreadable = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        match read_environment_requirement(&path) {
            Ok(Some(requirement)) => requirements.push(requirement),
            Ok(None) => {}
            Err(_) => unreadable += 1,
        }
    }
    requirements.sort_by(|left, right| left.name.cmp(&right.name));
    requirements.truncate(200);
    Ok(EnvironmentRequirements {
        requirements,
        unreadable,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EnvironmentReferences {
    pub(crate) name: String,
    /// Graphs whose node declares this environment.
    pub(crate) nodes: Vec<EnvironmentNodeReference>,
    /// Plugin servers whose manifest declares it, per node that binds them.
    pub(crate) servers: Vec<EnvironmentServerReference>,
    /// How much of the catalog this report actually saw. A report may only be
    /// read as "nobody uses this" when coverage is `Complete`.
    pub(crate) coverage: EnvironmentCoverage,
}

/// How complete one derived impact report is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum EnvironmentCoverage {
    Complete,
    /// The scan stopped early or could not read some definitions.
    Partial {
        scanned: usize,
        total: usize,
        missing: Vec<String>,
    },
    /// The report could not be produced at all.
    Unknown {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EnvironmentNodeReference {
    pub(crate) graph: String,
    pub(crate) node: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EnvironmentServerReference {
    pub(crate) graph: String,
    pub(crate) node: String,
    pub(crate) plugin: String,
    pub(crate) server: String,
}

/// The scan bound for a derived impact report.
const ENVIRONMENT_REFERENCE_LIMIT: usize = 200;

impl RunApplication {
    /// Who references one environment name right now.
    ///
    /// This reads the *editable* Graph definitions, not the compiled snapshots:
    /// an impact report may only be a derived view, and the operator's edits are
    /// what a name switch affects next. No reverse index is stored anywhere.
    pub(crate) fn environment_references(
        &self,
        library_root: &std::path::Path,
        name: &str,
    ) -> Result<EnvironmentReferences, ApplicationError> {
        // The deployment's configured bundle may live outside the editable
        // catalog; both are places a run can come from, so both are scanned.
        let mut roots = vec![self.catalog_root.clone()];
        if let Some(bundle) = std::env::var_os("ANCHOR_RUNNER_BUNDLE_ROOT").map(PathBuf::from)
            && bundle.is_dir()
            && !roots.contains(&bundle)
        {
            roots.push(bundle);
        }
        let mut graphs = Vec::new();
        let mut missing = Vec::new();
        for root in &roots {
            let entries = match std::fs::read_dir(root) {
                Ok(entries) => entries,
                Err(error) => {
                    // An unreadable root is not "nobody uses this".
                    return Ok(EnvironmentReferences {
                        name: name.to_owned(),
                        nodes: Vec::new(),
                        servers: Vec::new(),
                        coverage: EnvironmentCoverage::Unknown {
                            reason: format!("{} could not be read: {error}", root.display()),
                        },
                    });
                }
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let name_in_root = entry.file_name().to_string_lossy().into_owned();
                if !path.join("graph.json").is_file() {
                    continue;
                }
                // A symlinked Graph is a Graph too, but resolving it is the
                // loader's business; count it rather than dropping it.
                if entry.file_type().is_ok_and(|kind| kind.is_symlink()) {
                    missing.push(name_in_root);
                    continue;
                }
                graphs.push((root.clone(), name_in_root));
            }
        }
        graphs.sort();
        let total = graphs.len();
        let scanned = total.min(ENVIRONMENT_REFERENCE_LIMIT);
        graphs.truncate(ENVIRONMENT_REFERENCE_LIMIT);

        let mut nodes = Vec::new();
        let mut servers = Vec::new();
        for (root, graph) in &graphs {
            let path = root.join(graph).join("graph.json");
            let Some(definition) = std::fs::read(&path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            else {
                missing.push(graph.clone());
                continue;
            };
            // Which Plugins this Graph uses, and what they declare, comes from the
            // Plugin manifests themselves.
            let mut declared: Vec<(String, String, String)> = Vec::new();
            let plugin_ids = self.graph_plugin_ids(root, graph);
            for plugin in plugin_ids {
                let manifest = library_root
                    .join("plugins")
                    .join(&plugin)
                    .join("plugin.json");
                let Ok(bytes) = std::fs::read(&manifest) else {
                    continue;
                };
                let Ok(manifest) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
                    continue;
                };
                let Some(entries) = manifest
                    .get("mcpServers")
                    .and_then(|value| value.as_object())
                else {
                    continue;
                };
                for (server, config) in entries {
                    let environment = config
                        .get("anchor")
                        .and_then(|anchor| anchor.get("environment"))
                        .and_then(serde_json::Value::as_str);
                    if environment == Some(name) {
                        declared.push((plugin.clone(), server.clone(), name.to_owned()));
                    }
                }
            }
            // Nodes live at the top level and inside inline module graphs.
            let mut bodies = vec![&definition];
            if let Some(modules) = definition.get("graphs").and_then(|value| value.as_object()) {
                bodies.extend(modules.values());
            }
            for body in bodies {
                let Some(node_list) = body.get("nodes").and_then(|value| value.as_array()) else {
                    continue;
                };
                // Admission only lets an agent node or an `Op.run` node declare an
                // environment; a `call`/`fanout`/`join`/`host` declaration is
                // refused, so the report must not count it either.
                let ops = body.get("ops").and_then(|value| value.as_object());
                for node in node_list {
                    let node_id = node
                        .get("id")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    let declares_ok = if node.get("agent").is_some() {
                        true
                    } else {
                        node.get("op")
                            .and_then(serde_json::Value::as_str)
                            .and_then(|op| ops.and_then(|ops| ops.get(op)))
                            .is_some_and(|spec| spec.get("run").is_some())
                    };
                    if declares_ok
                        && node.get("environment").and_then(serde_json::Value::as_str) == Some(name)
                    {
                        nodes.push(EnvironmentNodeReference {
                            graph: graph.clone(),
                            node: node_id.clone(),
                        });
                    }
                    let bound = node
                        .get("plugins")
                        .and_then(|value| value.as_array())
                        .map(|plugins| {
                            plugins
                                .iter()
                                .filter_map(serde_json::Value::as_str)
                                .map(str::to_owned)
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    for (plugin, server, _) in &declared {
                        if bound.iter().any(|bound| bound == plugin) {
                            servers.push(EnvironmentServerReference {
                                graph: graph.clone(),
                                node: node_id.clone(),
                                plugin: plugin.clone(),
                                server: server.clone(),
                            });
                        }
                    }
                }
            }
        }
        let coverage = if missing.is_empty() && total <= ENVIRONMENT_REFERENCE_LIMIT {
            EnvironmentCoverage::Complete
        } else {
            EnvironmentCoverage::Partial {
                scanned,
                total,
                missing,
            }
        };
        Ok(EnvironmentReferences {
            name: name.to_owned(),
            nodes,
            servers,
            coverage,
        })
    }

    /// The Plugin ids one Graph's manifest declares.
    fn graph_plugin_ids(&self, root: &std::path::Path, graph: &str) -> Vec<String> {
        let manifest = root.join(graph).join("manifest.json");
        let Ok(bytes) = std::fs::read(&manifest) else {
            return Vec::new();
        };
        let Ok(manifest) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            return Vec::new();
        };
        manifest
            .get("plugins")
            .and_then(|value| value.as_array())
            .map(|plugins| {
                plugins
                    .iter()
                    .filter_map(|plugin| {
                        plugin
                            .get("id")
                            .and_then(serde_json::Value::as_str)
                            .or_else(|| plugin.as_str())
                            .map(str::to_owned)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Every durable plan, ordered by name and content identity: what was requested
/// and how it ended.
///
/// This is the read model the approval record needs; without it a published
/// environment and an unanswered request are indistinguishable to an operator.
pub(crate) fn list_environment_plans(
    data_root: &std::path::Path,
) -> Result<Vec<EnvironmentPlan>, ApplicationError> {
    let directory = data_root.join("environment-plans");
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(ApplicationError::Storage(error.to_string())),
    };
    let mut plans = Vec::new();
    let mut unreadable = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            let bytes = std::fs::read(&path)
                .map_err(|error| ApplicationError::Storage(error.to_string()))?;
            match serde_json::from_slice::<EnvironmentPlan>(&bytes) {
                Ok(plan) => plans.push(plan),
                // A record this version cannot read is skipped; the projection
                // reports how many were skipped so it cannot hide a plan.
                Err(_) => unreadable += 1,
            }
        }
    }
    // Deterministic order without needing a timestamp in the record.
    plans.sort_by(|left, right| {
        (left.name.as_str(), left.digest.as_str())
            .cmp(&(right.name.as_str(), right.digest.as_str()))
    });
    if unreadable > 0 {
        return Err(ApplicationError::Storage(format!(
            "{unreadable} environment plan record(s) could not be read"
        )));
    }
    plans.truncate(200);
    Ok(plans)
}

/// The durable plan a caller may re-read (used by tests and diagnostics).
#[cfg(test)]
pub(crate) fn read_environment_plan(
    data_root: &std::path::Path,
    digest: &str,
    name: &str,
) -> Result<Option<EnvironmentPlan>, ApplicationError> {
    let path = data_root
        .join("environment-plans")
        .join(format!("{digest}-{name}.json"));
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
    use super::*;
    use std::fs;

    fn application(root: &std::path::Path) -> RunApplication {
        RunApplication::new(root.join("state"), root.to_path_buf())
    }

    fn candidate(root: &std::path::Path, marker: &str) -> PathBuf {
        let directory = root.join(format!("candidate-{marker}"));
        fs::create_dir_all(directory.join("bin")).unwrap();
        fs::write(directory.join("marker.txt"), marker).unwrap();
        fs::write(directory.join("bin/python"), "#!/bin/sh\n").unwrap();
        directory
    }

    fn requester() -> EnvironmentPlanRequester {
        EnvironmentPlanRequester {
            owner: "local".into(),
            session: "pilot".into(),
            turn: "turn-1".into(),
        }
    }

    #[test]
    fn the_impact_scan_finds_node_and_plugin_server_references() {
        let root = tempfile::tempdir().unwrap();
        let application = application(root.path());
        // The bundle loader needs the run store the application owns.
        std::fs::create_dir_all(root.path().join("state/runs")).unwrap();
        std::fs::create_dir_all(root.path().join("library/plugins/demo")).unwrap();
        std::fs::write(
            root.path().join("library/plugins/demo/plugin.json"),
            r#"{"name":"Demo","mcpServers":{"probe":{"command":"bin/run","anchor":{"environment":"research"}}}}"#,
        )
        .unwrap();
        let graph = root.path().join("used");
        std::fs::create_dir_all(&graph).unwrap();
        std::fs::write(
            graph.join("graph.json"),
            serde_json::json!({
                "objective": "impact",
                "entry": "worker",
                "agents": {"worker": {"model": "test"}},
                "ops": {},
                "nodes": [
                    {"id": "worker", "agent": "worker", "plugins": ["demo"]},
                    {"id": "other", "agent": "worker", "environment": "research"}
                ],
                "edges": []
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            graph.join("manifest.json"),
            serde_json::json!({
                "format": 1,
                "graph": "graph.json",
                "plugins": [{
                    "id": "demo",
                    "digest": "a".repeat(64),
                    "resources": [],
                    "mcp_servers": []
                }]
            })
            .to_string(),
        )
        .unwrap();

        let library = root.path().join("library");
        let references = application
            .environment_references(&library, "research")
            .unwrap();
        assert!(
            references
                .nodes
                .iter()
                .any(|reference| reference.graph == "used" && reference.node == "other"),
            "{references:?}"
        );
        assert!(
            references.servers.iter().any(|reference| {
                reference.graph == "used"
                    && reference.node == "worker"
                    && reference.plugin == "demo"
                    && reference.server == "probe"
            }),
            "{references:?}"
        );
        let unused = application
            .environment_references(&library, "unused")
            .unwrap();
        assert!(unused.nodes.is_empty() && unused.servers.is_empty());
    }

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

        let plans = list_environment_plans(&data_root).unwrap();
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
    fn force_writable(root: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut stack = vec![root.to_path_buf()];
        while let Some(directory) = stack.pop() {
            let mut permissions = fs::metadata(&directory).unwrap().permissions();
            permissions.set_mode(permissions.mode() | 0o700);
            fs::set_permissions(&directory, permissions).unwrap();
            for entry in fs::read_dir(&directory).unwrap() {
                let entry = entry.unwrap();
                let metadata = entry.metadata().unwrap();
                if metadata.is_dir() {
                    stack.push(entry.path());
                } else {
                    let mut permissions = metadata.permissions();
                    permissions.set_mode(permissions.mode() | 0o600);
                    fs::set_permissions(entry.path(), permissions).unwrap();
                }
            }
        }
    }

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
        let plan = read_environment_plan(
            &root.path().join("state"),
            &precondition.plan.digest,
            "research",
        )
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
