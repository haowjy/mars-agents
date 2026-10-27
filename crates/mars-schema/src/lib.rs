//! Pure schema vocabulary, parsing, validation, and editing for Mars packages.

/// The schema crate version, kept in lockstep with the Mars CLI.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod agent;
pub mod dependency;
pub mod dialect;
pub mod discover;
mod dispatch;
pub mod filter;
pub mod git_url;
pub mod harness;
pub mod harness_overrides;
pub mod inspect;
pub mod manifest;
pub mod normalize;
pub mod read;
pub mod skill;
pub mod tree;

pub use harness::HarnessId;

pub mod claude;
pub mod frontmatter;
mod invocability;
pub mod item;
mod lift;
pub mod markdown;

pub mod mcp;
pub mod provider;
pub mod tool_names;
pub mod tool_natives;

pub mod tool_policy;

pub mod model_policy;
pub mod names;
pub mod network;
pub mod ordered;
pub mod permission_set;

pub mod diagnostic;
pub mod edit;
pub mod package;
pub mod plugin_metadata;
pub mod prepare;
mod set_aside;
pub mod upgrade;
pub mod version;
