//! Frozen legacy observations, independent of the Kernel's comparison rules.
//!
//! Each seed retains both original run-id/input namespaces. In particular,
//! changing the run id changes durable invocation and GraphCall identities.
use super::*;
use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
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
pub(crate) enum Population {
    Serial,
    ParallelFailures,
    ParallelNoFailures,
    ParallelModules,
    Calls,
}

impl Population {
    pub(crate) const ALL: [Self; 5] = [
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

    pub(crate) fn seeds(self) -> u64 {
        if self == Self::Serial { 10_000 } else { 5_000 }
    }

    pub(crate) fn inject_failures(self) -> bool {
        matches!(self, Self::Serial | Self::ParallelFailures)
    }

    fn graph(self, seed: u64) -> GraphSnapshot {
        match self {
            Self::Serial => random_graph(seed),
            Self::ParallelFailures | Self::ParallelNoFailures => random_parallel_graph(seed),
            Self::ParallelModules => random_parallel_module_graph(seed),
            Self::Calls => random_call_graph(seed),
        }
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

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "admission", rename_all = "snake_case")]
pub(crate) enum FrozenOutcome {
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
    pub(crate) fn accepted(&self) -> (&Trace, GraphRunRecord) {
        match self {
            Self::Accepted { trace, record } => (
                trace,
                serde_json::from_value(record.clone()).expect("legacy snapshot record"),
            ),
            Self::Rejected { reason } => panic!("unexpected frozen rejection: {reason}"),
        }
    }

    pub(crate) fn assert_rejection(&self, error: &GraphError) {
        match self {
            Self::Rejected { reason } => assert_eq!(reason, &error.to_string()),
            Self::Accepted { .. } => panic!("previously accepted seed rejected: {error}"),
        }
    }

    pub(crate) fn assert_old(&self, trace: &Trace, record: &GraphRunRecord) {
        let (frozen_trace, frozen_record) = self.accepted();
        assert_eq!(
            trace, frozen_trace,
            "legacy trace differs from frozen baseline"
        );
        // Preserve all original bytes in the artifact. Only wall-clock times
        // are omitted when a fresh legacy execution is checked against it.
        let normalize = |r: &GraphRunRecord| {
            let mut value = serde_json::to_value(r).unwrap();
            value.as_object_mut().unwrap().remove("started");
            value.as_object_mut().unwrap().remove("updated");
            // Workspace feature unification can enable serde_json/preserve_order.
            // Join/Call summaries are generated JSON, so compare their complete
            // structure while leaving executor-authored text byte-exact.
            for node in &r.snapshot.nodes {
                let control = node
                    .op
                    .as_ref()
                    .and_then(|id| r.snapshot.ops.get(id))
                    .is_some_and(|op| op.get("join").is_some() || op.get("call").is_some());
                if control && let Some(results) = value["results"][&node.id].as_array_mut() {
                    for result in results {
                        if let Some(summary) = result["completion"]["submission"].as_str()
                            && let Ok(json) = serde_json::from_str::<Value>(summary)
                        {
                            result["completion"]["submission"] = json;
                        }
                    }
                }
            }
            value
        };
        assert_eq!(
            normalize(record),
            normalize(&frozen_record),
            "complete legacy facts differ from frozen baseline"
        );
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct FrozenCase {
    pub seed: u64,
    pub validator: FrozenOutcome,
    pub scheduler: FrozenOutcome,
}

/// Stream one population so regular regression does not retain tens of
/// thousands of complete legacy records in memory.
pub(crate) struct Snapshots {
    lines: Lines<BufReader<GzDecoder<File>>>,
    population: Population,
    next_seed: u64,
}

impl Snapshots {
    pub(crate) fn open(population: Population) -> Self {
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

    pub(crate) fn next(&mut self, seed: u64) -> FrozenCase {
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

    pub(crate) fn at(population: Population, seed: u64) -> FrozenCase {
        let mut snapshots = Self::open(population);
        for skipped in 0..seed {
            snapshots.next(skipped);
        }
        snapshots.next(seed)
    }
}

async fn freeze_outcome(
    snapshot: GraphSnapshot,
    input: Value,
    run_id: String,
    seed: u64,
    inject_failures: bool,
) -> FrozenOutcome {
    match GraphRunRecord::create_with_id(snapshot, input, run_id) {
        Ok(record) => {
            let (trace, record) = run_old(record, seed, inject_failures).await;
            FrozenOutcome::Accepted {
                trace,
                record: serde_json::to_value(record).unwrap(),
            }
        }
        Err(error) => FrozenOutcome::Rejected {
            reason: error.to_string(),
        },
    }
}

async fn freeze(population: Population) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let sources = [
        "src/graph/test_support.rs",
        "src/graph/runner.rs",
        "src/graph/parallel.rs",
        "src/graph/logic.rs",
        "src/graph/state.rs",
        "src/graph/test_support/snapshots.rs",
    ]
    .into_iter()
    .map(|path| Source {
        path: format!("rust/anchor-runtime/{path}"),
        sha256: format!("{:x}", Sha256::digest(fs::read(root.join(path)).unwrap())),
    })
    .collect();
    let header = Header {
        format: FORMAT,
        generator_version: GENERATOR_VERSION.into(),
        generator_sources: sources,
        legacy_runner_baseline: BASELINE.into(),
        population,
        seed_start: 0,
        seed_end_exclusive: population.seeds(),
        cases: population.seeds(),
        inject_failures: population.inject_failures(),
        namespaces: BTreeMap::from([
            ("validator".into(), "run_id=r{seed}; input={}".into()),
            ("scheduler".into(), if population == Population::Calls {
                "run_id=scheduler-calls-{seed}; input={}"
            } else {
                "run_id=scheduler-diff-{seed}; input=null"
            }.into()),
        ]),
        comparison: "Preserve full legacy records; omit only started/updated wall-clock fields when comparing fresh legacy execution. Kernel comparisons retain the existing differential rules and S18 invariants; no byte-equality requirement between legacy and Kernel.".into(),
    };
    fs::create_dir_all(ROOT).unwrap();
    let temporary = population.path().with_extension("gz.new");
    let mut output = GzEncoder::new(File::create(&temporary).unwrap(), Compression::default());
    serde_json::to_writer(&mut output, &header).unwrap();
    output.write_all(b"\n").unwrap();
    for seed in 0..population.seeds() {
        let snapshot = population.graph(seed);
        let validator = freeze_outcome(
            snapshot.clone(),
            serde_json::json!({}),
            format!("r{seed}"),
            seed,
            population.inject_failures(),
        )
        .await;
        let (run_id, input) = if population == Population::Calls {
            (format!("scheduler-calls-{seed}"), serde_json::json!({}))
        } else {
            (format!("scheduler-diff-{seed}"), Value::Null)
        };
        let scheduler =
            freeze_outcome(snapshot, input, run_id, seed, population.inject_failures()).await;
        serde_json::to_writer(
            &mut output,
            &FrozenCase {
                seed,
                validator,
                scheduler,
            },
        )
        .unwrap();
        output.write_all(b"\n").unwrap();
    }
    output.finish().unwrap().sync_all().unwrap();
    fs::rename(temporary, population.path()).unwrap();
    eprintln!(
        "frozen {:?}: {} seeds, validator + scheduler namespaces",
        population,
        population.seeds()
    );
}

#[tokio::test]
async fn old_runner_snapshots_have_provenance_and_refresh_only_when_explicit() {
    if std::env::var("ANCHOR_UPDATE_LEGACY_SNAPSHOTS").as_deref() == Ok("1") {
        for population in Population::ALL {
            freeze(population).await;
        }
    }
    for population in Population::ALL {
        let _ = Snapshots::open(population);
    }
}
