//! The dependency name a provider's hooks, MCP servers and skill records carry in a lock.
//!
//! Config reserves the prefix, so no package or project dependency can share it.

const PREFIX: &str = "_provided-";

/// `_provided-<provider>`.
pub(crate) fn source_name(provider: &str) -> String {
    format!("{PREFIX}{provider}")
}

pub(crate) fn is_provided_source(name: &str) -> bool {
    name.starts_with(PREFIX)
}
