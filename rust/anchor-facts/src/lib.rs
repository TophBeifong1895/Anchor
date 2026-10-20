//! The shared vocabulary of harness facts.
//!
//! An execution port records what it observed as ordinary kernel submissions
//! to resources it owns, in its own state root; the data hub reads them back
//! through [`anchor_kernel::HistoryReader`]. This crate is the only contract
//! between the two: resource names, validators and content schemas. It holds
//! no storage and no analysis.
//!
//! Facts carry no business content. A tool call is recorded by its name, the
//! shape of its arguments and how it ended, never by argument or result values.

use anchor_kernel::{
    Action, Authority, JudgeCtx, Judgement, Kernel, KernelError, Principal, PrincipalKind, Reason,
    Store, Submission, Validator, Verification,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::Arc;

/// Judges observation records: only host code may record, and only facts
/// about the invocation the resource is named after.
pub const OBSERVATION: &str = "validator/observation";
/// Configuration manifests are content-addressed and never change.
pub const CONFIGURATION: &str = "validator/configuration";
/// The host principal that records observations.
pub const RECORDER_ID: &str = "anchor/recorder";
/// The content schema version written by this crate.
pub const VERSION: u32 = 1;

/// The recorder, as the configuration it observed. Authority is judged by id
/// and kind only; the digest is recorded identity.
pub fn recorder(configuration: &str) -> Principal {
    Principal::new(RECORDER_ID, PrincipalKind::Code).with_config(configuration)
}

/// Install the validators of this vocabulary into a kernel.
pub fn install<S: Store>(kernel: &mut Kernel<S>) -> Result<(), KernelError> {
    kernel.install(OBSERVATION, Arc::new(Observations))?;
    kernel.install(CONFIGURATION, Arc::new(Immutable))
}

/// The authority of an observation root: the recorder writes traces.
pub struct RecorderOnly;

impl Authority for RecorderOnly {
    fn permits(&self, principal: &Principal, action: Action, target: &str) -> bool {
        principal.id == RECORDER_ID
            && principal.kind == PrincipalKind::Code
            && action == Action::Write
            && target.starts_with("trace/")
    }
}

// ---------------------------------------------------------------------------
// Names
// ---------------------------------------------------------------------------

/// One execution of one node in one Run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invocation {
    pub run_id: String,
    pub graph_digest: String,
    pub node_id: String,
    pub invocation: u64,
}

impl Invocation {
    /// The resource holding this invocation's observations.
    pub fn trace(&self) -> String {
        format!("trace/{}/{}/{}", self.run_id, self.node_id, self.invocation)
    }
}

/// The resource holding the manifest with this digest.
pub fn configuration_id(digest: &str) -> String {
    format!("config/{digest}")
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// What the host froze for one AgentNode invocation. Its digest is the
/// configuration identity stamped on every observation of the invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentConfiguration {
    pub version: u32,
    pub graph_digest: String,
    pub node_id: String,
    pub model: String,
    pub model_binding: String,
    pub runtime_sha256: String,
    /// Digest of the node's instructions and task text.
    pub instructions_sha256: String,
    /// Digest of the Plugin instructions disclosed to the model.
    pub plugin_instructions_sha256: String,
    pub plugins: Vec<PluginIdentity>,
    pub routes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PluginIdentity {
    pub id: String,
    pub digest: String,
    pub authorization: String,
}

impl AgentConfiguration {
    /// Order-insensitive collections are sorted so equal configurations have
    /// equal digests.
    pub fn normalized(mut self) -> Self {
        self.plugins.sort();
        self.routes.sort();
        self
    }

    pub fn digest(&self) -> String {
        let bytes = serde_json::to_vec(&self.clone().normalized()).expect("manifest serializes");
        format!("{:x}", Sha256::digest(bytes))
    }
}

/// The digest of a text, for manifests that must not carry the text itself.
pub fn text_digest(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

// ---------------------------------------------------------------------------
// Observations
// ---------------------------------------------------------------------------

/// What the host observed of one invocation, recorded as the content of one
/// submission when the invocation ends. The facts happened at the times they
/// carry; one record holds them all because they are reported together, and a
/// record per fact would only multiply durable writes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Observation {
    pub version: u32,
    pub invocation: Invocation,
    pub facts: Vec<Fact>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Fact {
    /// When the invocation ran and how it ended.
    Lifecycle {
        started_at_ms: u64,
        ended_at_ms: u64,
        outcome: Outcome,
        model_turns: u64,
        tool_calls: u64,
        tool_calls_dropped: u64,
        resumed: bool,
    },
    /// One model response, as the host observed its usage report.
    ModelTurn {
        /// 1-based order within the invocation.
        index: u64,
        message_id: Option<String>,
        /// When the host saw the usage report. Goose reports a turn's usage
        /// once the turn is processed, which is after the tool calls it asked
        /// for have run: this is not when the response ended. A turn ends no
        /// later than its first tool call starts.
        observed_at_ms: Option<u64>,
        elapsed_ms: Option<u64>,
        time_to_first_token_ms: Option<u64>,
        input_tokens: u64,
        output_tokens: u64,
        total_tokens: u64,
        cache_read_tokens: Option<u64>,
        cache_write_tokens: Option<u64>,
        cost_usd: Option<f64>,
        compaction: bool,
    },
    /// One tool call through the host bridge.
    ToolCall {
        tool: String,
        /// Goose's own id for the call, when it sent one.
        tool_call_id: Option<String>,
        /// How many model requests had been made when the call arrived: the
        /// call answers model request number `model_request`.
        model_request: u64,
        arguments_shape: String,
        started_at_ms: Option<u64>,
        ended_at_ms: Option<u64>,
        ok: bool,
        failure: Option<Failure>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Completed,
    Failed,
    Cancelled,
    /// Ended without a known result; resumable after inspection.
    Interrupted,
}

/// Why a tool call did not succeed, as far as the host can tell without
/// reading its business content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Failure {
    /// The host's sandbox or grants did not allow it.
    NotAuthorized,
    /// The arguments did not fit the tool's contract.
    InvalidArguments,
    /// The completion tool refused the attempt.
    CompletionRejected,
    Cancelled,
    /// The host refused to execute it for another reason.
    NotExecuted,
    /// The tool ran and reported an error.
    ToolError,
}

/// Classify a tool error by the host's own refusal wording. Only the class is
/// recorded, never the text.
pub fn classify_failure(error: &str) -> Failure {
    let error = error.to_ascii_lowercase();
    if error.contains("not authorized") || error.contains("must belong to an authorized") {
        Failure::NotAuthorized
    } else if error.contains("invalid arguments") || error.contains("missing field") {
        Failure::InvalidArguments
    } else if error.contains("final_result") || error.contains("observed_receipt") {
        Failure::CompletionRejected
    } else if error.contains("cancelled") {
        Failure::Cancelled
    } else if error.contains("not_executed") || error.contains("refused") {
        Failure::NotExecuted
    } else {
        Failure::ToolError
    }
}

/// A digest of the structure of tool arguments: keys and value types, never
/// values. Equal shapes mean the model called the tool the same way.
pub fn arguments_shape(arguments: &Value) -> String {
    fn shape(value: &Value) -> Value {
        match value {
            // Sort explicitly: with serde_json's `preserve_order`, which other
            // crates in a build may enable, maps keep insertion order.
            Value::Object(map) => {
                let mut entries: Vec<_> = map.iter().collect();
                entries.sort_by(|a, b| a.0.cmp(b.0));
                Value::Object(
                    entries
                        .into_iter()
                        .map(|(key, value)| (key.clone(), shape(value)))
                        .collect(),
                )
            }
            Value::Array(_) => Value::from("array"),
            Value::String(_) => Value::from("string"),
            Value::Number(_) => Value::from("number"),
            Value::Bool(_) => Value::from("bool"),
            Value::Null => Value::from("null"),
        }
    }
    let bytes = serde_json::to_vec(&shape(arguments)).expect("shape serializes");
    format!("{:x}", Sha256::digest(bytes))
}

// ---------------------------------------------------------------------------
// Validators
// ---------------------------------------------------------------------------

struct Observations;

impl Validator for Observations {
    fn judge(&self, ctx: &JudgeCtx, submission: &Submission) -> Judgement {
        // An agent cannot report on itself; only host code records.
        if ctx.principal.kind != PrincipalKind::Code || submission.action != Action::Write {
            return Judgement::refused(Reason::Unauthorized);
        }
        match serde_json::from_value::<Observation>(submission.content.clone()) {
            Ok(observation)
                if observation.version == VERSION
                    && !observation.facts.is_empty()
                    && observation.invocation.trace() == ctx.resource.id =>
            {
                // The trace's own state stays empty: the facts are its history.
                Judgement::accept(ctx.resource.state.clone(), Verification::Asserted)
            }
            _ => Judgement::returned(Reason::ContentInvalid),
        }
    }
}

struct Immutable;

impl Validator for Immutable {
    fn judge(&self, _: &JudgeCtx, _: &Submission) -> Judgement {
        Judgement::refused(Reason::Unauthorized)
    }
}

#[cfg(test)]
mod tests;
