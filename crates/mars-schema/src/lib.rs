//! Pure schema vocabulary, parsing, validation, and editing for Mars packages.

/// The schema crate version, kept in lockstep with the Mars CLI.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod harness;

pub use harness::HarnessId;

pub mod frontmatter;
pub mod invocability;
pub mod item;
pub mod markdown;
