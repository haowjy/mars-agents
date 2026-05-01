//! Collision-resolution extension point for config entries.
//!
//! Future collision resolution policy belongs here; current behavior remains
//! hard-error collision detection in the MCP compiler lane.

/// Marker for the future config-entry collision resolution lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CollisionResolutionLane;

const _: CollisionResolutionLane = CollisionResolutionLane;
