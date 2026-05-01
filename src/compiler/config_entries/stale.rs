//! Stale config-entry cleanup extension point.
//!
//! Future stale-entry reconciliation belongs here; current behavior does not
//! delete target config entries from this compiler lane.

/// Marker for the future stale config-entry cleanup lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StaleCleanupLane;

const _: StaleCleanupLane = StaleCleanupLane;
