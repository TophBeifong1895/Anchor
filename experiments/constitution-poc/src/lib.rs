//! Proof of concept for the Anchor constitution.
//!
//! - [`kernel`]: the six primitives and the four kernel duties (adjudicate,
//!   record, settle, measure). Experiment E1.
//! - [`lifecycle`]: exhaustive comparison of the documented Run lifecycle
//!   rules against the same behaviour derived from the constitution. E2.
//! - [`sim`] and [`measure`]: probes, statistics, a deterministic selection
//!   gate and knob analysis over a simulated proposer. E3.
//!
//! Nothing here depends on, or changes, the Anchor workspace.

pub mod kernel;
pub mod lifecycle;
pub mod measure;
pub mod rng;
pub mod sim;
