//! Package item vocabulary.

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ItemKind {
    Agent,
    Skill,
    Hook,
    // The aliases are the spellings before kebab-case, which locks written by older builds hold.
    #[serde(alias = "mcpserver")]
    McpServer,
    #[serde(alias = "bootstrapdoc")]
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

/// The kinds of file a package's definitions are: what `mars-schema` selects, prepares and
/// reads, as opposed to the binary's wider [`ItemKind`].
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DefinitionKind {
    Agent,
    Skill,
}

impl From<DefinitionKind> for ItemKind {
    fn from(kind: DefinitionKind) -> Self {
        match kind {
            DefinitionKind::Agent => Self::Agent,
            DefinitionKind::Skill => Self::Skill,
        }
    }
}

impl fmt::Display for DefinitionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        ItemKind::from(*self).fmt(f)
    }
}
