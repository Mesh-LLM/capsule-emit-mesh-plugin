//! The referee: an independent check of two twins that answered the same
//! request differently.
//!
//! - [`half`], [`verdict`]: two halves checked and compared; the ruling a
//!   referee's re-answer gives.
//! - [`hold`]: a node receiving a verdict delivered to it.
//! - `crate::verdict_counts`, `crate::routing_rule`: the verdicts on this
//!   node's own chain, per peer and model, and the opt-in stop-routing rule.
//!
//! Nothing here is a measure of a node. A verdict is a yes/no ruling on one
//! pair of answers, signed by the referee that gave it; the counts are this
//! node's own and are never sent to anyone.

pub mod half;
pub mod hold;
pub mod verdict;

#[cfg(test)]
pub(crate) mod parity;
