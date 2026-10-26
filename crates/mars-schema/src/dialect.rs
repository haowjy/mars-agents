//! Inbound dialect vocabulary for foreign definitions.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

use crate::HarnessId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Dialect {
    Claude,
    /// The Agent Skills standard on its own: translation is the identity plus relocation.
    #[serde(rename = "agent-skills")]
    AgentSkills,
    Codex,
    #[serde(rename = "opencode")]
    OpenCode,
    Cursor,
    #[serde(rename = "mars-native")]
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

    pub fn from_harness_id(id: HarnessId) -> Option<Self> {
        match id {
            HarnessId::Claude => Some(Self::Claude),
            HarnessId::Codex => Some(Self::Codex),
            HarnessId::OpenCode => Some(Self::OpenCode),
            HarnessId::Cursor => Some(Self::Cursor),
            HarnessId::Pi => None,
        }
    }

    pub fn to_harness_id(self) -> Option<HarnessId> {
        match self {
            Self::Claude => Some(HarnessId::Claude),
            Self::Codex => Some(HarnessId::Codex),
            Self::OpenCode => Some(HarnessId::OpenCode),
            Self::Cursor => Some(HarnessId::Cursor),
            Self::AgentSkills | Self::MarsNative => None,
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
