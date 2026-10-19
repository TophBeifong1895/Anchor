//! Frozen legacy observations, independent of the Kernel's comparison rules.
//!
//! Each seed retains both original run-id/input namespaces. In particular,
//! changing the run id changes durable invocation and GraphCall identities.
use super::*;
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{BufRead, BufReader, Lines},
    path::Path,
};

const BASELINE: &str = "4d3d19d4ba120a85526c7d5a8abc00ef0c777eec";
const FORMAT: u32 = 1;
const GENERATOR_VERSION: &str = "anchor-runtime-random-graphs-v1";
const ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../experiments/constitution-host-preflight/m3/snapshots"
);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Population {
    Serial,
    ParallelFailures,
    ParallelNoFailures,
    ParallelModules,
    Calls,
}

impl Population {
    pub const ALL: [Self; 5] = [
        Self::Serial,
        Self::ParallelFailures,
        Self::ParallelNoFailures,
        Self::ParallelModules,
        Self::Calls,
    ];

    fn file(self) -> &'static str {
        match self {
            Self::Serial => "serial.jsonl.gz",
            Self::ParallelFailures => "parallel-failures.jsonl.gz",
            Self::ParallelNoFailures => "parallel-no-failures.jsonl.gz",
            Self::ParallelModules => "parallel-modules.jsonl.gz",
            Self::Calls => "calls.jsonl.gz",
        }
    }

    pub fn seeds(self) -> u64 {
        if self == Self::Serial { 10_000 } else { 5_000 }
    }

    pub fn inject_failures(self) -> bool {
        matches!(self, Self::Serial | Self::ParallelFailures)
    }

    fn path(self) -> PathBuf {
        Path::new(ROOT).join(self.file())
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Source {
    path: String,
    sha256: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct Header {
    format: u32,
    generator_version: String,
    generator_sources: Vec<Source>,
    legacy_runner_baseline: String,
    population: Population,
    seed_start: u64,
    seed_end_exclusive: u64,
    cases: u64,
    inject_failures: bool,
    namespaces: BTreeMap<String, String>,
    comparison: String,
}

// Historical wire types only. These have no executor, store, control or
// compatibility role in the product; they retain evidence needed by S18.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrozenStatus {
    Ready,
    Running,
    Paused,
    BudgetStopped,
    WaitingCall,
    WaitingRecovery,
    Completed,
    Aborted,
    Stopped,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenCursor {
    pub node_id: String,
    pub key: InvocationKey,
    pub input_commits: Vec<CommitRef>,
    pub prepared_input: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenParallelBranch {
    pub branch_id: String,
    pub entry: String,
    pub nodes: Vec<String>,
    pub next_index: usize,
    pub status: ParallelBranchStatus,
    pub cursor: Option<FrozenCursor>,
    /// Commits for the completed prefix of `nodes`, in dependency order.
    pub completed: Vec<CommitRef>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenParallelActivation {
    pub activation_id: String,
    pub fanout_node: String,
    pub join_node: String,
    pub fanout_invocation: u64,
    pub branches: Vec<FrozenParallelBranch>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenOldRecord {
    pub format: u32,
    /// RFC 3339 UTC when the record was created. Absent on records written
    /// before format 8: the real creation time is unknown and is deliberately
    /// not inferred from file mtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started: Option<String>,
    /// Original legacy write time, retained as evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated: Option<String>,
    pub run_id: String,
    pub graph_digest: String,
    pub snapshot: GraphSnapshot,
    pub input: Value,
    pub status: FrozenStatus,
    pub cursor: Option<FrozenCursor>,
    #[serde(default)]
    pub parallel: Option<FrozenParallelActivation>,
    /// Persisted unresolved tool attempts. Each attempt is tied to
    /// the exact serial or parallel invocation whose cursor remains active.
    #[serde(default)]
    pub recovery: Vec<PendingRecovery>,
    #[serde(default)]
    pub recovery_submissions: Vec<RecoverySubmission>,
    #[serde(default)]
    pub graph_calls: BTreeMap<String, GraphCallRecord>,
    #[serde(default)]
    pub plugin_bindings: BTreeMap<String, PluginBinding>,
    #[serde(default)]
    pub plugin_bindings_initialized: bool,
    #[serde(default)]
    pub invocations: BTreeMap<String, u64>,
    pub passes: BTreeMap<String, u64>,
    pub module_activations: BTreeMap<String, u64>,
    pub ceased: BTreeSet<String>,
    pub results: BTreeMap<String, Vec<RunResult>>,
    pub decided: BTreeMap<String, EdgeDecision>,
    pub sequence: u64,
    pub error: Option<String>,
}

impl FrozenParallelActivation {
    fn graph_progress(&self) -> ParallelActivation {
        ParallelActivation {
            activation_id: self.activation_id.clone(),
            fanout_node: self.fanout_node.clone(),
            join_node: self.join_node.clone(),
            fanout_invocation: self.fanout_invocation,
            branches: self
                .branches
                .iter()
                .map(|branch| ParallelBranchRecord {
                    branch_id: branch.branch_id.clone(),
                    entry: branch.entry.clone(),
                    nodes: branch.nodes.clone(),
                    next_index: branch.next_index,
                    status: branch.status,
                    completed: branch.completed.clone(),
                    error: branch.error.clone(),
                })
                .collect(),
        }
    }
    #[cfg(test)]
    fn from_graph(activation: &ParallelActivation) -> Self {
        Self {
            activation_id: activation.activation_id.clone(),
            fanout_node: activation.fanout_node.clone(),
            join_node: activation.join_node.clone(),
            fanout_invocation: activation.fanout_invocation,
            branches: activation
                .branches
                .iter()
                .map(|branch| FrozenParallelBranch {
                    branch_id: branch.branch_id.clone(),
                    entry: branch.entry.clone(),
                    nodes: branch.nodes.clone(),
                    next_index: branch.next_index,
                    status: branch.status,
                    cursor: None,
                    completed: branch.completed.clone(),
                    error: branch.error.clone(),
                })
                .collect(),
        }
    }
}

impl FrozenOldRecord {
    /// Test-only structural projection. Lifecycle and cursors stay in this
    /// historical wire value and never enter live Graph facts or persistence.
    pub fn graph_record(&self) -> GraphRunRecord {
        GraphRunRecord {
            format: RunRecordFormat,
            started: self.started.clone(),
            updated: self.updated.clone(),
            run_id: self.run_id.clone(),
            graph_digest: self.graph_digest.clone(),
            snapshot: self.snapshot.clone(),
            input: self.input.clone(),
            parallel: self
                .parallel
                .as_ref()
                .map(FrozenParallelActivation::graph_progress),
            recovery: self.recovery.clone(),
            recovery_submissions: self.recovery_submissions.clone(),
            graph_calls: self.graph_calls.clone(),
            plugin_bindings: self.plugin_bindings.clone(),
            plugin_bindings_initialized: self.plugin_bindings_initialized,
            invocations: self.invocations.clone(),
            passes: self.passes.clone(),
            module_activations: self.module_activations.clone(),
            ceased: self.ceased.clone(),
            results: self.results.clone(),
            decided: self.decided.clone(),
            sequence: self.sequence,
            error: self.error.clone(),
        }
    }
    #[cfg(test)]
    pub(crate) fn from_graph(record: &GraphRunRecord) -> Self {
        Self {
            status: FrozenStatus::Ready,
            cursor: None,
            format: u32::from(record.format),
            started: record.started.clone(),
            updated: record.updated.clone(),
            run_id: record.run_id.clone(),
            graph_digest: record.graph_digest.clone(),
            snapshot: record.snapshot.clone(),
            input: record.input.clone(),
            parallel: record
                .parallel
                .as_ref()
                .map(FrozenParallelActivation::from_graph),
            recovery: record.recovery.clone(),
            recovery_submissions: record.recovery_submissions.clone(),
            graph_calls: record.graph_calls.clone(),
            plugin_bindings: record.plugin_bindings.clone(),
            plugin_bindings_initialized: record.plugin_bindings_initialized,
            invocations: record.invocations.clone(),
            passes: record.passes.clone(),
            module_activations: record.module_activations.clone(),
            ceased: record.ceased.clone(),
            results: record.results.clone(),
            decided: record.decided.clone(),
            sequence: record.sequence,
            error: record.error.clone(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "admission", rename_all = "snake_case")]
pub enum FrozenOutcome {
    Accepted {
        trace: Trace,
        /// Complete legacy record, including branch cursors, full Work
        /// results and terminal errors needed by the unchanged S18 checker.
        record: Value,
    },
    Rejected {
        reason: String,
    },
}

impl FrozenOutcome {
    pub fn accepted(&self) -> (&Trace, FrozenOldRecord) {
        match self {
            Self::Accepted { trace, record } => (
                trace,
                serde_json::from_value(record.clone()).expect("legacy snapshot record"),
            ),
            Self::Rejected { reason } => panic!("unexpected frozen rejection: {reason}"),
        }
    }

    pub fn assert_rejection(&self, error: &GraphError) {
        match self {
            Self::Rejected { reason } => assert_eq!(reason, &error.to_string()),
            Self::Accepted { .. } => panic!("previously accepted seed rejected: {error}"),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FrozenCase {
    pub seed: u64,
    pub validator: FrozenOutcome,
    pub scheduler: FrozenOutcome,
}

/// Stream one population so regular regression does not retain tens of
/// thousands of complete legacy records in memory.
pub struct Snapshots {
    lines: Lines<BufReader<GzDecoder<File>>>,
    population: Population,
    next_seed: u64,
}

impl Snapshots {
    pub fn open(population: Population) -> Self {
        let file = File::open(population.path()).expect("committed legacy snapshot");
        let mut lines = BufReader::new(GzDecoder::new(file)).lines();
        let header: Header =
            serde_json::from_str(&lines.next().unwrap().unwrap()).expect("snapshot header");
        assert_eq!(header.format, FORMAT);
        assert_eq!(header.generator_version, GENERATOR_VERSION);
        assert_eq!(header.legacy_runner_baseline, BASELINE);
        assert_eq!(header.population, population);
        assert_eq!(
            (header.seed_start, header.seed_end_exclusive),
            (0, population.seeds())
        );
        assert_eq!(header.cases, population.seeds());
        assert_eq!(header.inject_failures, population.inject_failures());
        assert!(!header.generator_sources.is_empty());
        assert!(
            header
                .generator_sources
                .iter()
                .all(|s| s.sha256.len() == 64)
        );
        Self {
            lines,
            population,
            next_seed: 0,
        }
    }

    pub fn next(&mut self, seed: u64) -> FrozenCase {
        assert_eq!(seed, self.next_seed, "snapshot seed order");
        let case: FrozenCase =
            serde_json::from_str(&self.lines.next().expect("snapshot missing seed").unwrap())
                .expect("complete legacy snapshot case");
        assert_eq!(case.seed, seed);
        self.next_seed += 1;
        if self.next_seed == self.population.seeds() {
            assert!(
                self.lines.next().is_none(),
                "unexpected extra snapshot case"
            );
        }
        case
    }

    pub fn at(population: Population, seed: u64) -> FrozenCase {
        let mut snapshots = Self::open(population);
        for skipped in 0..seed {
            snapshots.next(skipped);
        }
        snapshots.next(seed)
    }
}

#[test]
fn frozen_snapshots_have_complete_provenance() {
    for population in Population::ALL {
        let _ = Snapshots::open(population);
    }
}
