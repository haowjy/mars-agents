//! The Claude dialect: plugin and marketplace manifests, discovery, relocation and
//! translation of Claude-shaped repositories into schema v1 items.

pub(crate) mod checks;
pub(crate) mod discover;
pub mod manifest;
pub(crate) mod mcp;
pub(crate) mod translate;
