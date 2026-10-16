//! The six primitives and the four kernel duties.
//!
//! Constitution (docs/anchor-constitution.md):
//! 1. Every change is a submission, adjudicated by code that is not the proposer.
//! 2. History is append-only; what was not observed is unknown.
//! 3. Everything ends, within finite resources, in one of a few outcomes.
//! 4. Every submission is measured; only selected changes persist.
//! 5. Purpose and the highest rules come from people; no rule can be changed
//!    by what it constrains.
//!
//! The kernel knows no business, protocol, model or workflow. It adjudicates,
//! records, settles and measures. Everything else is a [`Validator`].

use std::collections::{BTreeMap, BTreeSet};

use crate::measure::Aggregator;

pub type ResourceId = String;
pub type Fields = BTreeMap<String, String>;

pub const CONSTITUTION: &str = "validator/constitution";
pub const HUMAN_DECISION: &str = "validator/human-decision";
pub const OBSERVATION: &str = "validator/observation";

// ---------------------------------------------------------------------------
// Primitive 2: Submission (what a proposer may say)
// ---------------------------------------------------------------------------

/// The four things a proposer can do to the world.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Action {
    /// Change a resource.
    Write,
    /// Ask a higher level for authority or information.
    Request,
    /// Produce an effect on the outside world.
    Execute,
    /// End a task.
    Settle,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Evidence {
    /// Index into the kernel's observation log.
    Observation(usize),
    /// A resource at an exact version.
    Artifact(ResourceId, u64),
}

/// Everything a proposer may fill in. There is deliberately no field for
/// identity, cost, authority, approval or time: those are host facts, and a
/// field that does not exist cannot be forged.
#[derive(Clone, Debug, PartialEq)]
pub struct Submission {
    pub target: ResourceId,
    /// The version the proposer saw (optimistic concurrency).
    pub seen: u64,
    pub action: Action,
    pub fields: Fields,
    /// Free natural language. The kernel never interprets it.
    pub text: String,
    pub evidence: Vec<Evidence>,
}

impl Submission {
    pub fn new(target: &str, seen: u64, action: Action) -> Self {
        Self {
            target: target.into(),
            seen,
            action,
            fields: Fields::new(),
            text: String::new(),
            evidence: Vec::new(),
        }
    }

    pub fn field(mut self, key: &str, value: &str) -> Self {
        self.fields.insert(key.into(), value.into());
        self
    }

    pub fn text(mut self, text: &str) -> Self {
        self.text = text.into();
        self
    }

    pub fn evidence(mut self, evidence: Evidence) -> Self {
        self.evidence.push(evidence);
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PrincipalKind {
    Agent,
    Human,
    Code,
}

/// Host-observed facts about one submission. Fields are private and the only
/// constructor is inside the kernel, so no proposer can build one.
#[derive(Clone, Debug)]
pub struct Stamp {
    proposer: String,
    kind: PrincipalKind,
    task: Option<ResourceId>,
    cost: u64,
    knob: String,
    seq: u64,
}

impl Stamp {
    pub fn proposer(&self) -> &str {
        &self.proposer
    }
    pub fn kind(&self) -> PrincipalKind {
        self.kind
    }
    pub fn task(&self) -> Option<&str> {
        self.task.as_deref()
    }
    pub fn cost(&self) -> u64 {
        self.cost
    }
    pub fn knob(&self) -> &str {
        &self.knob
    }
    pub fn seq(&self) -> u64 {
        self.seq
    }
}

// ---------------------------------------------------------------------------
// Primitive 3: Verdict (two independent dimensions + a reason code)
// ---------------------------------------------------------------------------

/// May the change take effect?
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Effect {
    Accepted,
    /// Fixable: go back to the proposer.
    Returned,
    /// Not fixable: authority, a law, or a settled fact.
    Refused,
    /// Waiting for a person or another validator.
    Pending,
}

/// What verification did the content receive? Independent of [`Effect`]:
/// "accepted" never means "correct".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Verification {
    Mechanical,
    Evidenced,
    Asserted,
    Reviewed,
    NotApplicable,
}

/// A closed set chosen by code, never summarised by a model.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Reason {
    Ok,
    UnknownResource,
    StaleVersion,
    Unauthorized,
    SelfJudgement,
    TaskSettled,
    BudgetExhausted,
    EffectUnknown,
    AwaitingHuman,
    HumanOnly,
    PendingTargetChanged,
    CompletionInvalid,
    RouteInvalid,
    StateRefused,
    EvidenceMissing,
    Unclassified,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Verdict {
    pub effect: Effect,
    pub verification: Verification,
    pub reason: Reason,
}

impl Verdict {
    fn refused(reason: Reason) -> Self {
        Self {
            effect: Effect::Refused,
            verification: Verification::NotApplicable,
            reason,
        }
    }
    fn returned(reason: Reason) -> Self {
        Self {
            effect: Effect::Returned,
            verification: Verification::NotApplicable,
            reason,
        }
    }
}

/// What a validator decides. `next` is the resource state after acceptance.
#[derive(Clone, Debug)]
pub struct Judgement {
    pub verdict: Verdict,
    pub next: Option<Fields>,
}

impl Judgement {
    pub fn accept(next: Fields, verification: Verification) -> Self {
        Self {
            verdict: Verdict {
                effect: Effect::Accepted,
                verification,
                reason: Reason::Ok,
            },
            next: Some(next),
        }
    }
    pub fn returned(reason: Reason) -> Self {
        Self {
            verdict: Verdict::returned(reason),
            next: None,
        }
    }
    pub fn refused(reason: Reason) -> Self {
        Self {
            verdict: Verdict::refused(reason),
            next: None,
        }
    }
    /// Pending with the state that would apply once a person approves.
    pub fn pending(next: Fields) -> Self {
        Self {
            verdict: Verdict {
                effect: Effect::Pending,
                verification: Verification::NotApplicable,
                reason: Reason::AwaitingHuman,
            },
            next: Some(next),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Observation {
    pub fields: Fields,
}

pub struct JudgeCtx<'a> {
    pub state: &'a Fields,
    pub proposer_kind: PrincipalKind,
    pub observations: &'a [Observation],
}

impl JudgeCtx<'_> {
    /// The strongest verification the referenced evidence supports. A
    /// mechanical check is an observation the host recorded with `check=pass`.
    pub fn verification_of(&self, submission: &Submission) -> Verification {
        let mechanical = submission.evidence.iter().any(|evidence| match evidence {
            Evidence::Observation(index) => self
                .observations
                .get(*index)
                .is_some_and(|o| o.fields.get("check").map(String::as_str) == Some("pass")),
            Evidence::Artifact(..) => false,
        });
        if mechanical {
            Verification::Mechanical
        } else if submission.evidence.is_empty() {
            Verification::Asserted
        } else {
            Verification::Evidenced
        }
    }
}

/// A validator is a resource too; changing it is a submission to the level
/// above it.
pub trait Validator {
    fn judge(&self, ctx: &JudgeCtx, submission: &Submission) -> Judgement;
}

// ---------------------------------------------------------------------------
// Primitives 1, 4, 5: Resource, Effect, Task
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Record {
    pub stamp: Stamp,
    pub submission: Submission,
    pub verdict: Verdict,
}

struct Resource {
    version: u64,
    validator: ResourceId,
    state: Fields,
    history: Vec<Record>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Settlement {
    Done,
    Failed(Reason),
    Escalated,
    Unknown,
}

struct Task {
    budget: u64,
    spent: u64,
    settled: Option<Settlement>,
}

/// What the outside world answered. `NoResponse` becomes "unknown", never
/// "failed": the effect may have happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorldOutcome {
    Ok,
    Fail,
    NoResponse,
}

pub trait World {
    fn perform(&mut self, endpoint: &str, fields: &Fields) -> WorldOutcome;
}

struct Principal {
    kind: PrincipalKind,
    grants: BTreeSet<(String, Action)>,
    /// Every validator that has judged this principal. A principal can never
    /// change a validator in this set (law 5). Grown automatically.
    subject_to: BTreeSet<ResourceId>,
}

/// One measurement point, produced at the moment of adjudication.
#[derive(Clone, Debug)]
pub struct Point {
    pub component: String,
    pub action: Action,
    pub effect: Effect,
    pub reason: Reason,
    pub cost: u64,
    pub knob: String,
}

struct PendingChange {
    original: ResourceId,
    seen: u64,
    next: Fields,
}

// ---------------------------------------------------------------------------
// The kernel: adjudicate, record, settle, measure
// ---------------------------------------------------------------------------

pub struct Kernel {
    resources: BTreeMap<ResourceId, Resource>,
    validators: BTreeMap<ResourceId, Box<dyn Validator>>,
    principals: BTreeMap<String, Principal>,
    tasks: BTreeMap<ResourceId, Task>,
    observations: Vec<Observation>,
    pending: BTreeMap<ResourceId, PendingChange>,
    seq: u64,
    knob: String,
    keep_history: bool,
    pub probe: Aggregator,
}

impl Default for Kernel {
    fn default() -> Self {
        Self::new()
    }
}

impl Kernel {
    pub fn new() -> Self {
        let mut kernel = Self {
            resources: BTreeMap::new(),
            validators: BTreeMap::new(),
            principals: BTreeMap::new(),
            tasks: BTreeMap::new(),
            observations: Vec::new(),
            pending: BTreeMap::new(),
            seq: 0,
            knob: "v0".into(),
            keep_history: true,
            probe: Aggregator::default(),
        };
        // The top of the recursion: only people may change the highest rules.
        kernel.install_validator(CONSTITUTION, Box::new(HumanOnly), CONSTITUTION);
        kernel.install_validator(HUMAN_DECISION, Box::new(HumanDecision), CONSTITUTION);
        kernel.install_validator(OBSERVATION, Box::new(ObservationRequired), CONSTITUTION);
        kernel
    }

    /// Benchmarks may drop in-memory history; the semantics are unchanged.
    pub fn without_history(mut self) -> Self {
        self.keep_history = false;
        self
    }

    pub fn set_knob(&mut self, knob: &str) {
        self.knob = knob.into();
    }

    pub fn install_validator(&mut self, id: &str, validator: Box<dyn Validator>, judged_by: &str) {
        self.validators.insert(id.into(), validator);
        self.resources.insert(
            id.into(),
            Resource {
                version: 0,
                validator: judged_by.into(),
                state: Fields::new(),
                history: Vec::new(),
            },
        );
    }

    pub fn create(&mut self, id: &str, validator: &str, state: Fields) {
        assert!(
            self.validators.contains_key(validator),
            "unknown validator {validator}"
        );
        self.resources.insert(
            id.into(),
            Resource {
                version: 0,
                validator: validator.into(),
                state,
                history: Vec::new(),
            },
        );
    }

    pub fn add_principal(&mut self, id: &str, kind: PrincipalKind) {
        self.principals.insert(
            id.into(),
            Principal {
                kind,
                grants: BTreeSet::new(),
                subject_to: BTreeSet::new(),
            },
        );
    }

    /// Bootstrap grants come from the deployment (a person).
    pub fn grant_root(&mut self, to: &str, pattern: &str, action: Action) {
        self.principals
            .get_mut(to)
            .expect("principal")
            .grants
            .insert((pattern.into(), action));
    }

    /// Authority only descends: a principal can grant only what it holds.
    pub fn delegate(
        &mut self,
        from: &str,
        to: &str,
        pattern: &str,
        action: Action,
    ) -> Result<(), Reason> {
        let holds = self.principals.get(from).is_some_and(|p| {
            p.grants
                .iter()
                .any(|(g, a)| *a == action && covers(g, pattern))
        });
        if !holds {
            return Err(Reason::Unauthorized);
        }
        self.principals
            .get_mut(to)
            .ok_or(Reason::UnknownResource)?
            .grants
            .insert((pattern.into(), action));
        Ok(())
    }

    pub fn register_task(&mut self, resource: &str, budget: u64) {
        self.tasks.insert(
            resource.into(),
            Task {
                budget,
                spent: 0,
                settled: None,
            },
        );
    }

    /// A host observation (a tool result, a check). Observations are facts the
    /// host saw; proposers can only reference them.
    pub fn observe(&mut self, fields: &[(&str, &str)]) -> usize {
        self.observations.push(Observation {
            fields: fields
                .iter()
                .map(|(k, v)| ((*k).into(), (*v).into()))
                .collect(),
        });
        self.observations.len() - 1
    }

    pub fn version(&self, id: &str) -> u64 {
        self.resources.get(id).map(|r| r.version).unwrap_or(0)
    }

    pub fn state(&self, id: &str) -> Option<&Fields> {
        self.resources.get(id).map(|r| &r.state)
    }

    pub fn history(&self, id: &str) -> &[Record] {
        self.resources
            .get(id)
            .map(|r| r.history.as_slice())
            .unwrap_or(&[])
    }

    pub fn settlement(&self, task: &str) -> Option<Settlement> {
        self.tasks.get(task).and_then(|t| t.settled)
    }

    pub fn resource_ids_with_prefix(&self, prefix: &str) -> Vec<ResourceId> {
        self.resources
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect()
    }

    /// The single entry point for change. `task` and `cost` are host facts.
    pub fn submit(
        &mut self,
        proposer: &str,
        task: Option<&str>,
        cost: u64,
        submission: Submission,
        world: &mut dyn World,
    ) -> Verdict {
        self.seq += 1;
        let kind = self
            .principals
            .get(proposer)
            .map(|p| p.kind)
            .unwrap_or(PrincipalKind::Agent);
        let stamp = Stamp {
            proposer: proposer.into(),
            kind,
            task: task.map(Into::into),
            cost,
            knob: self.knob.clone(),
            seq: self.seq,
        };
        let verdict = self.adjudicate(&stamp, &submission, world);
        self.record(stamp, submission, verdict);
        verdict
    }

    fn adjudicate(
        &mut self,
        stamp: &Stamp,
        submission: &Submission,
        world: &mut dyn World,
    ) -> Verdict {
        // Law 3: every task is bounded and settles exactly once.
        if let Some(task_id) = stamp.task.as_deref() {
            let Some(task) = self.tasks.get_mut(task_id) else {
                return Verdict::refused(Reason::UnknownResource);
            };
            if task.settled.is_some() {
                return Verdict::refused(Reason::TaskSettled);
            }
            task.spent += stamp.cost;
            if task.spent > task.budget {
                task.settled = Some(Settlement::Failed(Reason::BudgetExhausted));
                return Verdict::refused(Reason::BudgetExhausted);
            }
        }
        let Some(resource) = self.resources.get(&submission.target) else {
            return Verdict::refused(Reason::UnknownResource);
        };
        let validator_id = resource.validator.clone();
        let Some(principal) = self.principals.get(&stamp.proposer) else {
            return Verdict::refused(Reason::Unauthorized);
        };
        // Authority only descends; nothing in a submission can create it.
        if !principal
            .grants
            .iter()
            .any(|(g, a)| *a == submission.action && covers(g, &submission.target))
        {
            return Verdict::refused(Reason::Unauthorized);
        }
        // Law 5: no one changes a validator that judges them.
        if self.validators.contains_key(&submission.target)
            && principal.subject_to.contains(&submission.target)
        {
            return Verdict::refused(Reason::SelfJudgement);
        }
        if submission.seen != resource.version {
            return Verdict::returned(Reason::StaleVersion);
        }
        // Law 2: an unknown effect must be observed before the next one.
        if submission.action == Action::Execute && self.has_unknown_effect(&submission.target) {
            return Verdict::refused(Reason::EffectUnknown);
        }
        let judgement = {
            let validator = self
                .validators
                .get(&validator_id)
                .expect("validator installed");
            validator.judge(
                &JudgeCtx {
                    state: &resource.state,
                    proposer_kind: stamp.kind,
                    observations: &self.observations,
                },
                submission,
            )
        };
        self.principals
            .get_mut(&stamp.proposer)
            .expect("principal")
            .subject_to
            .insert(validator_id);
        self.apply(submission, judgement, world)
    }

    fn apply(
        &mut self,
        submission: &Submission,
        judgement: Judgement,
        world: &mut dyn World,
    ) -> Verdict {
        let verdict = judgement.verdict;
        match verdict.effect {
            Effect::Accepted => {
                let next = judgement.next.unwrap_or_default();
                if submission.target.starts_with("pending/") {
                    return self.resolve_pending(&submission.target, &next, verdict);
                }
                let resource = self
                    .resources
                    .get_mut(&submission.target)
                    .expect("resource");
                resource.version += 1;
                resource.state = next.clone();
                match submission.action {
                    Action::Execute => self.perform(&submission.target, &submission.fields, world),
                    Action::Settle => self.settle_from(&submission.target, &next),
                    Action::Write | Action::Request => {}
                }
                verdict
            }
            Effect::Pending => {
                let id = format!("pending/{}", self.pending.len() + 1);
                self.pending.insert(
                    id.clone(),
                    PendingChange {
                        original: submission.target.clone(),
                        seen: submission.seen,
                        next: judgement.next.unwrap_or_default(),
                    },
                );
                self.create(&id, HUMAN_DECISION, Fields::new());
                verdict
            }
            Effect::Returned | Effect::Refused => verdict,
        }
    }

    /// A person approved or rejected a pending change. The approval binds to
    /// the exact version the proposal saw; a changed target refuses it.
    fn resolve_pending(
        &mut self,
        pending_id: &str,
        decision: &Fields,
        verdict: Verdict,
    ) -> Verdict {
        let change = self.pending.remove(pending_id).expect("pending change");
        let pending = self
            .resources
            .get_mut(pending_id)
            .expect("pending resource");
        pending.version += 1;
        pending.state = decision.clone();
        if decision.get("decision").map(String::as_str) != Some("approve") {
            return verdict;
        }
        let original = self.resources.get_mut(&change.original).expect("original");
        if original.version != change.seen {
            return Verdict::refused(Reason::PendingTargetChanged);
        }
        original.version += 1;
        original.state = change.next;
        verdict
    }

    fn perform(&mut self, endpoint: &str, fields: &Fields, world: &mut dyn World) {
        let id = format!("effect/{}", self.seq);
        let mut state = Fields::new();
        state.insert("endpoint".into(), endpoint.into());
        state.insert("key".into(), self.seq.to_string());
        state.insert("status".into(), "started".into());
        self.create(&id, OBSERVATION, state);
        let status = match world.perform(endpoint, fields) {
            WorldOutcome::Ok => "succeeded",
            WorldOutcome::Fail => "failed",
            WorldOutcome::NoResponse => "unknown",
        };
        let effect = self.resources.get_mut(&id).expect("effect");
        effect.state.insert("status".into(), status.into());
    }

    fn has_unknown_effect(&self, endpoint: &str) -> bool {
        self.resources
            .range("effect/".to_string()..)
            .take_while(|(k, _)| k.starts_with("effect/"))
            .any(|(_, r)| {
                r.state.get("endpoint").map(String::as_str) == Some(endpoint)
                    && r.state.get("status").map(String::as_str) == Some("unknown")
            })
    }

    fn settle_from(&mut self, resource: &str, state: &Fields) {
        if let Some(task) = self.tasks.get_mut(resource)
            && task.settled.is_none()
        {
            task.settled = Some(match state.get("outcome").map(String::as_str) {
                Some("escalate") => Settlement::Escalated,
                Some("unknown") => Settlement::Unknown,
                Some("failed") => Settlement::Failed(Reason::Unclassified),
                _ => Settlement::Done,
            });
        }
    }

    fn record(&mut self, stamp: Stamp, submission: Submission, verdict: Verdict) {
        let component = submission.target.split('/').next().unwrap_or("").to_owned();
        self.probe.observe(&Point {
            component,
            action: submission.action,
            effect: verdict.effect,
            reason: verdict.reason,
            cost: stamp.cost,
            knob: stamp.knob.clone(),
        });
        if !self.keep_history {
            return;
        }
        if let Some(resource) = self.resources.get_mut(&submission.target) {
            resource.history.push(Record {
                stamp,
                submission,
                verdict,
            });
        }
    }
}

/// `run/*` covers `run/a`; anything else must match exactly.
fn covers(grant: &str, target: &str) -> bool {
    match grant.strip_suffix('*') {
        Some(prefix) => target.starts_with(prefix),
        None => grant == target,
    }
}

// ---------------------------------------------------------------------------
// Built-in validators
// ---------------------------------------------------------------------------

/// The constitution's own validator: only people may change the top rules.
pub struct HumanOnly;

impl Validator for HumanOnly {
    fn judge(&self, ctx: &JudgeCtx, submission: &Submission) -> Judgement {
        if ctx.proposer_kind != PrincipalKind::Human {
            return Judgement::refused(Reason::HumanOnly);
        }
        Judgement::accept(submission.fields.clone(), Verification::Reviewed)
    }
}

/// A person completes a pending verdict.
pub struct HumanDecision;

impl Validator for HumanDecision {
    fn judge(&self, ctx: &JudgeCtx, submission: &Submission) -> Judgement {
        if ctx.proposer_kind != PrincipalKind::Human {
            return Judgement::refused(Reason::HumanOnly);
        }
        match submission.fields.get("decision").map(String::as_str) {
            Some("approve" | "reject") => {
                Judgement::accept(submission.fields.clone(), Verification::Reviewed)
            }
            _ => Judgement::returned(Reason::CompletionInvalid),
        }
    }
}

/// An effect's status changes only with an observation as evidence: what was
/// not observed stays unknown.
pub struct ObservationRequired;

impl Validator for ObservationRequired {
    fn judge(&self, ctx: &JudgeCtx, submission: &Submission) -> Judgement {
        let observed = submission.evidence.iter().any(|e| match e {
            Evidence::Observation(i) => ctx.observations.get(*i).is_some(),
            Evidence::Artifact(..) => false,
        });
        if !observed {
            return Judgement::returned(Reason::EvidenceMissing);
        }
        match submission.fields.get("status").map(String::as_str) {
            Some("succeeded" | "failed") => {
                let mut next = ctx.state.clone();
                next.insert("status".into(), submission.fields["status"].clone());
                Judgement::accept(next, Verification::Evidenced)
            }
            _ => Judgement::returned(Reason::CompletionInvalid),
        }
    }
}

// ---------------------------------------------------------------------------
// Example validators: a Graph is the validator of a Run
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Strictness {
    /// Every transition is specified.
    Strict,
    /// Only the final settlement contract is specified.
    Loose,
}

/// A write → review → publish Graph with a feedback edge, expressed as the
/// rules for which submissions a Run accepts in each state. It does not tell
/// anyone what to do next.
pub struct ReviewGraph {
    pub strictness: Strictness,
}

impl Validator for ReviewGraph {
    fn judge(&self, ctx: &JudgeCtx, submission: &Submission) -> Judgement {
        let phase = ctx
            .state
            .get("phase")
            .map(String::as_str)
            .unwrap_or("draft");
        let verification = ctx.verification_of(submission);
        match (self.strictness, submission.action) {
            (Strictness::Strict, Action::Write) => {
                let (node, routes): (&str, &[&str]) = match phase {
                    "draft" => ("writer", &["review"]),
                    "review" => ("reviewer", &["revise", "publish"]),
                    _ => return Judgement::refused(Reason::StateRefused),
                };
                let (Some(summary), Some(route)) = (
                    submission.fields.get("summary"),
                    submission.fields.get("route"),
                ) else {
                    return Judgement::returned(Reason::CompletionInvalid);
                };
                if submission.fields.get("node").map(String::as_str) != Some(node) {
                    return Judgement::refused(Reason::StateRefused);
                }
                if !routes.contains(&route.as_str()) {
                    return Judgement::returned(Reason::RouteInvalid);
                }
                let mut next = ctx.state.clone();
                next.insert(
                    "phase".into(),
                    match route.as_str() {
                        "review" => "review",
                        "revise" => "draft",
                        _ => "publish",
                    }
                    .into(),
                );
                next.insert("last_summary".into(), summary.clone());
                Judgement::accept(next, verification)
            }
            (Strictness::Loose, Action::Write) => {
                let mut next = ctx.state.clone();
                if let Some(summary) = submission.fields.get("summary") {
                    next.insert("last_summary".into(), summary.clone());
                }
                Judgement::accept(next, verification)
            }
            (strictness, Action::Settle) => {
                if strictness == Strictness::Strict && phase != "publish" {
                    return Judgement::returned(Reason::StateRefused);
                }
                if !submission.fields.contains_key("deliverable") {
                    return Judgement::returned(Reason::CompletionInvalid);
                }
                let mut next = ctx.state.clone();
                next.insert("phase".into(), "settled".into());
                next.insert(
                    "outcome".into(),
                    submission
                        .fields
                        .get("outcome")
                        .cloned()
                        .unwrap_or_else(|| "done".into()),
                );
                Judgement::accept(next, verification)
            }
            _ => Judgement::refused(Reason::StateRefused),
        }
    }
}

/// A Library whose installs require a person, bound to the proposed digest.
pub struct Library;

impl Validator for Library {
    fn judge(&self, ctx: &JudgeCtx, submission: &Submission) -> Judgement {
        let (Some(plugin), Some(digest)) = (
            submission.fields.get("plugin"),
            submission.fields.get("digest"),
        ) else {
            return Judgement::returned(Reason::CompletionInvalid);
        };
        let mut next = ctx.state.clone();
        next.insert(plugin.clone(), digest.clone());
        Judgement::pending(next)
    }
}

/// An outbound message endpoint.
pub struct Endpoint;

impl Validator for Endpoint {
    fn judge(&self, ctx: &JudgeCtx, submission: &Submission) -> Judgement {
        if !(submission.fields.contains_key("to") && submission.fields.contains_key("body")) {
            return Judgement::returned(Reason::CompletionInvalid);
        }
        Judgement::accept(ctx.state.clone(), Verification::NotApplicable)
    }
}

/// A Graph resource whose content is compiled (here: required keys) before
/// it is accepted.
pub struct GraphCompiler;

impl Validator for GraphCompiler {
    fn judge(&self, _ctx: &JudgeCtx, submission: &Submission) -> Judgement {
        if !submission.fields.contains_key("definition") {
            return Judgement::returned(Reason::CompletionInvalid);
        }
        Judgement::accept(submission.fields.clone(), Verification::Mechanical)
    }
}
