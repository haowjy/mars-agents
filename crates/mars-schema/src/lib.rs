//! Pure schema vocabulary, parsing, validation, and editing for Mars packages.

/// The schema crate version, kept in lockstep with the Mars CLI.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod agent;
pub mod harness;
pub mod manifest;
pub mod read;
pub mod skill;
pub mod tree;

pub use harness::HarnessId;

pub mod frontmatter;
pub mod invocability;
pub mod item;
pub mod markdown;

pub mod mcp;
pub mod tool_names;
pub mod tool_natives;

pub mod tool_policy;

pub mod model_policy;

pub mod diagnostic;
pub mod package;
pub mod upgrade;
