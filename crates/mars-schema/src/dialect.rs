//! Inbound dialect vocabulary for foreign definitions.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Dialect {
    Claude,
    /// The Agent Skills standard on its own: translation is the identity plus relocation.
    #[serde(rename = "agent-skills", alias = "agentskills", alias = "agent_skills")]
    AgentSkills,
    Codex,
    /// `open-code` is the spelling v0.19.0 accepted in `mars.toml`.
    #[serde(rename = "opencode", alias = "open-code")]
    OpenCode,
    Cursor,
    #[serde(rename = "mars-native", alias = "marsnative", alias = "mars_native")]
    MarsNative,
}

impl Dialect {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::AgentSkills => "agent-skills",
            Self::Codex => "codex",
            Self::OpenCode => "opencode",
            Self::Cursor => "cursor",
            Self::MarsNative => "mars-native",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "claude" => Some(Self::Claude),
            "agent-skills" | "agentskills" | "agent_skills" => Some(Self::AgentSkills),
            "codex" => Some(Self::Codex),
            "opencode" => Some(Self::OpenCode),
            "cursor" => Some(Self::Cursor),
            "mars-native" | "marsnative" | "mars_native" => Some(Self::MarsNative),
            _ => None,
        }
    }
}

/// A foreign dialect a root is read as, and why. This one value is what `prepare` classifies a
/// foreign root with and what `inspect` reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DialectChoice {
    pub dialect: Dialect,
    pub evidence: DialectEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DialectEvidence {
    pub source: DialectEvidenceSource,
    /// The files that decided a predicted dialect; empty for a declared one.
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DialectEvidenceSource {
    Declared,
    Predicted,
}

impl DialectChoice {
    pub(crate) fn declared(dialect: Dialect) -> Self {
        Self {
            dialect,
            evidence: DialectEvidence {
                source: DialectEvidenceSource::Declared,
                paths: Vec::new(),
            },
        }
    }

    pub(crate) fn predicted(dialect: Dialect, paths: Vec<String>) -> Self {
        Self {
            dialect,
            evidence: DialectEvidence {
                source: DialectEvidenceSource::Predicted,
                paths,
            },
        }
    }
}

impl fmt::Display for Dialect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Dialect {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value).ok_or(())
    }
}
