//! Config compatibility migrations.
//!
//! Migration modules keep legacy read-time normalization close to config parsing
//! while separate from normal config and resolver code paths.

pub mod link;
