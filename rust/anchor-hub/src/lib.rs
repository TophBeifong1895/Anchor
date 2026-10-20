//! The harness data hub: the read side of recorded harness facts.
//!
//! Layers depend only downwards, and only L0 and L4 do IO:
//!
//! - L0 [`source`]: read-only kernel histories;
//! - L1 [`table`]: records projected into fact rows, kept up to date
//!   incrementally;
//! - L2 [`query`]: trajectory algebra, pure functions;
//! - L3 [`analyzers`]: plugins over L2 that return findings with evidence;
//! - L4 [`adapters`]: the command line.
//!
//! The hub depends on `anchor-kernel` and `anchor-facts` only, writes
//! nothing to any data root, and nothing depends on it.

pub mod adapters;
pub mod analyzers;
pub mod query;
pub mod source;
pub mod table;
