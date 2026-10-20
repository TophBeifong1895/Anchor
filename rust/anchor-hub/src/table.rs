//! L1: projection. Records become fact rows, one per submission, keyed by
//! data root, resource and place. Only harness facts are kept: who submitted,
//! when, under which configuration, how it was judged, and for observation
//! records the observed facts. Submission content of any other kind, outcome
//! details and evidence are never projected, so a table holds no business
//! content. A table can be dropped and rebuilt from its sources at any time.

use std::collections::BTreeMap;

use anchor_facts::{AgentConfiguration, Observation};
use anchor_kernel::{Action, Outcome, Phase, PrincipalKind, Record, Resource, Verdict, Waiter};
use serde::Serialize;

use crate::source::{HistorySource, SourceError};

/// Where a record is: its data root, its resource, and its 1-based place in
/// that resource's history. Records need no other number.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct Key {
    pub root: String,
    pub resource: String,
    pub place: usize,
}

/// A task's phase without its free-text details.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Executing,
    WaitingHuman,
    WaitingChild,
    WaitingEffect,
    Done,
    Failed(anchor_kernel::Reason),
    Escalated,
    Unknown,
    Aborted,
}

impl Status {
    pub fn of(phase: &Phase) -> Self {
        match phase {
            Phase::Executing => Self::Executing,
            Phase::Waiting { waiter } => match waiter {
                Waiter::Human { .. } => Self::WaitingHuman,
                Waiter::Child { .. } => Self::WaitingChild,
                Waiter::Effect { .. } => Self::WaitingEffect,
            },
            Phase::Settled { result } => match result {
                Outcome::Done => Self::Done,
                Outcome::Failed { reason, .. } => Self::Failed(*reason),
                Outcome::Escalated { .. } => Self::Escalated,
                Outcome::Unknown { .. } => Self::Unknown,
                Outcome::Aborted { .. } => Self::Aborted,
            },
        }
    }

    pub fn is_settled(self) -> bool {
        !matches!(
            self,
            Self::Executing | Self::WaitingHuman | Self::WaitingChild | Self::WaitingEffect
        )
    }
}

/// What a record says beyond its kernel fields.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "body", rename_all = "snake_case")]
pub enum Body {
    /// Kernel fields only.
    Kernel,
    /// An execution port's observations of one invocation.
    Observation(Observation),
    /// An observation record this hub's vocabulary cannot read.
    Unreadable,
}

/// One submission.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Row {
    pub key: Key,
    pub at: Option<u64>,
    pub principal: String,
    pub principal_kind: PrincipalKind,
    pub config: Option<String>,
    pub cost: u64,
    pub action: Action,
    pub slot: Option<String>,
    pub verdict: Verdict,
    /// The task's status after this record, for task resources.
    pub status: Option<Status>,
    #[serde(flatten)]
    pub body: Body,
}

/// A resource as it stands now.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Entry {
    pub root: String,
    pub id: String,
    pub validator: String,
    pub version: u64,
    pub created_at: Option<u64>,
    pub created_config: Option<String>,
    pub status: Option<Status>,
}

/// Everything projected so far, as the trajectory algebra reads it.
#[derive(Clone, Copy)]
pub struct Facts<'a> {
    /// In projection order: by root, then resource, then place.
    pub rows: &'a [Row],
    pub resources: &'a BTreeMap<(String, String), Entry>,
    /// Configuration manifests by digest.
    pub configurations: &'a BTreeMap<String, AgentConfiguration>,
}

/// Storage of projected facts. The in-memory table is the first backend.
pub trait FactTable {
    fn insert(&mut self, row: Row);
    fn resource(&mut self, entry: Entry);
    fn configuration(&mut self, digest: String, manifest: AgentConfiguration);
    fn facts(&self) -> Facts<'_>;
}

#[derive(Default)]
pub struct MemoryTable {
    rows: Vec<Row>,
    resources: BTreeMap<(String, String), Entry>,
    configurations: BTreeMap<String, AgentConfiguration>,
}

impl FactTable for MemoryTable {
    fn insert(&mut self, row: Row) {
        self.rows.push(row);
    }
    fn resource(&mut self, entry: Entry) {
        self.resources
            .insert((entry.root.clone(), entry.id.clone()), entry);
    }
    fn configuration(&mut self, digest: String, manifest: AgentConfiguration) {
        self.configurations.insert(digest, manifest);
    }
    fn facts(&self) -> Facts<'_> {
        Facts {
            rows: &self.rows,
            resources: &self.resources,
            configurations: &self.configurations,
        }
    }
}

/// Projects sources into a table, remembering per resource how far it has
/// read so that a later pull adds only what was appended since.
#[derive(Default)]
pub struct Projector {
    read: BTreeMap<(String, String), usize>,
}

impl Projector {
    /// Returns how many rows were added.
    pub fn pull(
        &mut self,
        source: &dyn HistorySource,
        table: &mut dyn FactTable,
    ) -> Result<usize, SourceError> {
        let root = source.root().to_owned();
        let mut added = 0;
        for id in source.list()? {
            // Removed since it was listed.
            let Some(resource) = source.load(&id)? else {
                continue;
            };
            if resource.validator == anchor_facts::CONFIGURATION
                && let Some(digest) = id.strip_prefix("config/")
                && let Ok(manifest) =
                    serde_json::from_value::<AgentConfiguration>(resource.state.clone())
                // Content-addressed: a manifest that does not hash to its
                // name is not that configuration.
                && manifest.digest() == digest
            {
                table.configuration(digest.to_owned(), manifest);
            }
            let read = self.read.entry((root.clone(), id.clone())).or_default();
            for record in source.history_after(&id, *read)? {
                *read += 1;
                added += 1;
                table.insert(row(&root, &id, *read, &resource.validator, record));
            }
            table.resource(entry(&root, &resource));
        }
        Ok(added)
    }
}

fn row(root: &str, id: &str, place: usize, validator: &str, record: Record) -> Row {
    let body = if validator == anchor_facts::OBSERVATION {
        serde_json::from_value::<Observation>(record.submission.content)
            .map(Body::Observation)
            .unwrap_or(Body::Unreadable)
    } else {
        Body::Kernel
    };
    let principal = record.stamp.principal();
    Row {
        key: Key {
            root: root.to_owned(),
            resource: id.to_owned(),
            place,
        },
        at: record.stamp.at(),
        principal: principal.id.clone(),
        principal_kind: principal.kind,
        config: principal.config.clone(),
        cost: record.stamp.cost(),
        action: record.submission.action,
        slot: record.submission.slot,
        verdict: record.verdict,
        status: record
            .after
            .as_ref()
            .and_then(Resource::phase)
            .map(Status::of),
        body,
    }
}

fn entry(root: &str, resource: &Resource) -> Entry {
    Entry {
        root: root.to_owned(),
        id: resource.id.clone(),
        validator: resource.validator.clone(),
        version: resource.version,
        created_at: resource.created.as_ref().and_then(|c| c.at),
        created_config: resource.created.as_ref().and_then(|c| c.config.clone()),
        status: resource.phase().map(Status::of),
    }
}
