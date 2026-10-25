//! Package item vocabulary.

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemKind {
    Agent,
    Skill,
    Hook,
    McpServer,
    BootstrapDoc,
}

impl fmt::Display for ItemKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Agent => "agent",
            Self::Skill => "skill",
            Self::Hook => "hook",
            Self::McpServer => "mcp-server",
            Self::BootstrapDoc => "bootstrap-doc",
        })
    }
}
