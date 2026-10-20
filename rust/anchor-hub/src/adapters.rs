//! L4: the command line. Finds kernel data roots under the given paths,
//! projects them and prints analyzer reports or the fact rows themselves.
//!
//! ```text
//! anchor-hub health [--json] PATH...
//! anchor-hub stall [--json] [--idle-minutes N] [--follow SECONDS] PATH...
//! anchor-hub rows PATH...
//! ```
//!
//! A PATH may be a data root, a Host state directory, or any directory above
//! them; every data root found below it is read.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::analyzers::{Analyzer, Report, health::Health, stall::Stall};
use crate::query::Query;
use crate::source::{DataRoot, HistorySource};
use crate::table::{FactTable, MemoryTable, Projector};

/// How far below a given path to look for data roots.
const DEPTH: usize = 6;

/// A directory is a kernel data root when it holds validator resources, as
/// every kernel installs them.
fn is_root(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|entries| {
        entries.flatten().any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("validator%2F")
                && entry.path().join("resource.json").is_file()
        })
    })
}

/// Kernel data roots at or below `path`, without following links.
pub fn discover(path: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![(path.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        if is_root(&dir) {
            found.push(dir);
            continue;
        }
        if depth == DEPTH {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let hidden = entry.file_name().to_string_lossy().starts_with('.');
            if !hidden && entry.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push((entry.path(), depth + 1));
            }
        }
    }
    found.sort();
    found
}

/// Sources, a table and the projector that keeps one up to date with the
/// other.
#[derive(Default)]
pub struct Hub {
    paths: Vec<PathBuf>,
    sources: Vec<DataRoot>,
    opened: BTreeSet<PathBuf>,
    projector: Projector,
    table: MemoryTable,
}

impl Hub {
    pub fn new(paths: Vec<PathBuf>) -> Self {
        Self {
            paths,
            ..Self::default()
        }
    }

    /// Open roots that appeared and project what was appended since the
    /// last refresh. Returns how many rows were added.
    pub fn refresh(&mut self) -> Result<usize, String> {
        for path in &self.paths {
            for root in discover(path) {
                if self.opened.insert(root.clone()) {
                    let name = root.display().to_string();
                    self.sources
                        .push(DataRoot::open(&root, name).map_err(|e| e.to_string())?);
                }
            }
        }
        let mut added = 0;
        for source in &self.sources {
            added += self
                .projector
                .pull(source as &dyn HistorySource, &mut self.table)
                .map_err(|e| e.to_string())?;
        }
        Ok(added)
    }

    pub fn roots(&self) -> usize {
        self.sources.len()
    }

    pub fn query(&self, now: Option<u64>) -> Query<'_> {
        Query::new(self.table.facts(), now)
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

struct Options {
    command: String,
    json: bool,
    idle_minutes: Option<u64>,
    follow: Option<u64>,
    paths: Vec<PathBuf>,
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut args = args.iter();
    let command = args.next().ok_or(USAGE)?.clone();
    let mut options = Options {
        command,
        json: false,
        idle_minutes: None,
        follow: None,
        paths: Vec::new(),
    };
    while let Some(arg) = args.next() {
        let mut number = |name: &str| -> Result<u64, String> {
            args.next()
                .and_then(|v| v.parse().ok())
                .ok_or(format!("{name} needs a number"))
        };
        match arg.as_str() {
            "--json" => options.json = true,
            "--idle-minutes" => options.idle_minutes = Some(number(arg)?),
            "--follow" => options.follow = Some(number(arg)?),
            flag if flag.starts_with("--") => return Err(format!("unknown flag {flag}\n{USAGE}")),
            path => options.paths.push(path.into()),
        }
    }
    if options.paths.is_empty() {
        return Err(USAGE.into());
    }
    Ok(options)
}

const USAGE: &str = "usage: anchor-hub health [--json] PATH...\n       anchor-hub stall [--json] [--idle-minutes N] [--follow SECONDS] PATH...\n       anchor-hub rows PATH...";

pub fn run(args: &[String]) -> Result<(), String> {
    let options = parse(args)?;
    let mut hub = Hub::new(options.paths.clone());
    hub.refresh()?;
    // A follower may start before the first root exists.
    if hub.roots() == 0 && options.follow.is_none() {
        return Err("no kernel data root found under the given paths".into());
    }
    match options.command.as_str() {
        "rows" => {
            for row in hub.query(None).rows() {
                println!("{}", serde_json::to_string(row).map_err(|e| e.to_string())?);
            }
            Ok(())
        }
        "health" => {
            print(&Health.analyze(&hub.query(Some(now_ms()))), options.json);
            Ok(())
        }
        "stall" => {
            let mut stall = Stall::default();
            if let Some(minutes) = options.idle_minutes {
                stall.idle_ms = minutes * 60_000;
            }
            let Some(seconds) = options.follow else {
                print(&stall.analyze(&hub.query(Some(now_ms()))), options.json);
                return Ok(());
            };
            // Follow: print each finding the first time it appears.
            let mut seen = BTreeSet::new();
            loop {
                let mut report = stall.analyze(&hub.query(Some(now_ms())));
                report
                    .findings
                    .retain(|f| seen.insert((f.code, f.subject.clone(), f.detail.clone())));
                if !report.findings.is_empty() {
                    print(&report, options.json);
                }
                std::thread::sleep(Duration::from_secs(seconds.max(1)));
                hub.refresh()?;
            }
        }
        other => Err(format!("unknown command {other}\n{USAGE}")),
    }
}

fn print(report: &Report, json: bool) {
    if json {
        println!(
            "{}",
            serde_json::to_string(report).expect("report serializes")
        );
        return;
    }
    println!("== {} v{}", report.analyzer, report.version);
    if !report.not_applicable.is_empty() {
        println!(
            "not applicable: {}",
            serde_json::to_string(&report.not_applicable).expect("needs serialize")
        );
    }
    for (name, value) in &report.summary {
        println!(
            "-- {name}\n{}",
            serde_json::to_string_pretty(value).expect("summary serializes")
        );
    }
    println!("-- findings: {}", report.findings.len());
    for finding in &report.findings {
        println!(
            "{:<28} {:<40} {} [{} record(s)]",
            finding.code,
            finding.subject,
            finding.detail,
            finding.evidence.len()
        );
    }
}
