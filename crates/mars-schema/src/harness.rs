//! Stable harness identifiers and authored-name parsing.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::agent::EffortLevel;
use crate::tool_natives;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessId {
    Claude,
    Codex,
    Pi,
    #[serde(rename = "opencode")]
    OpenCode,
    Cursor,
    Agy,
}

impl HarnessId {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Pi => "pi",
            Self::OpenCode => "opencode",
            Self::Cursor => "cursor",
            Self::Agy => "agy",
        }
    }
}

impl HarnessId {
    /// The harness's native directory in a project (`.claude`).
    pub fn target_dir(self) -> String {
        format!(".{}", self.as_str())
    }

    pub fn display_name(self) -> &'static str {
        Host::Harness(self).spec().display_name
    }
}

impl fmt::Display for HarnessId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where a tool policy is lowered to: a harness Mars writes files for, or Meridian Flow, which
/// is a consumer of the result and not a harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Host {
    Harness(HarnessId),
    MeridianFlow,
}

impl From<HarnessId> for Host {
    fn from(harness: HarnessId) -> Self {
        Self::Harness(harness)
    }
}

impl Host {
    /// Every harness in [`ALL`]'s order, then Meridian Flow.
    pub const ALL: &'static [Host; ALL.len() + 1] = &{
        let mut hosts = [Host::MeridianFlow; ALL.len() + 1];
        let mut i = 0;
        while i < ALL.len() {
            hosts[i] = Host::Harness(ALL[i]);
            i += 1;
        }
        hosts
    };

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Harness(harness) => harness.as_str(),
            Self::MeridianFlow => "meridian-flow",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|host| host.as_str() == name)
    }
}

/// How a host spells an MCP ref in a tool list.
#[derive(Clone, Copy)]
pub(crate) enum McpNaming {
    /// Claude's `mcp__server__tool`, which cannot scope one tool across servers.
    Claude,
    /// A name built from the server and tool segments.
    Format(fn(server: &str, tool: &str) -> String),
    /// The host has no name for an MCP tool, for the reason given.
    NoName(&'static str),
    /// `agy` gives every agent every MCP server whatever its tool list says, so no name is
    /// written and a rule that restricts MCP is reported lost.
    Ungated,
    /// Flow reaches MCP through connection tools that take the server as an argument.
    ConnectionTools,
}

/// The facts the schema crate holds about one host. Each is read by one projection, so a new
/// host adds a row here and nothing else in this crate.
pub(crate) struct HostSpec {
    pub display_name: &'static str,
    /// The native name each capability becomes.
    pub tools: tool_natives::Table,
    /// Whether a tool name can be narrowed to a pattern. A harness spells the scope in the
    /// permission name (`Bash(git push *)`); Flow's tools and `agy`'s `tools` names take no
    /// scope.
    pub scopes_patterns: bool,
    pub mcp: McpNaming,
    /// The effort levels the host takes, or `None` for any. The caller passes the levels a
    /// probe-backed harness's slugs list.
    pub effort: Option<&'static [EffortLevel]>,
}

use EffortLevel::{High, Low, Max, Medium, XHigh};

/// Claude Code 2.1.325 `--effort` help: low, medium, high, xhigh, max.
const CLAUDE: HostSpec = HostSpec {
    display_name: "Claude",
    tools: tool_natives::CLAUDE,
    scopes_patterns: true,
    mcp: McpNaming::Claude,
    effort: Some(&[Low, Medium, High, XHigh, Max]),
};

/// Codex 0.167.0 `exec -c model_reasoning_effort=…` sends any value unchecked; the API rejected
/// `minimal` on gpt-5.6-luna and listed none, low, medium, high, xhigh, max. models.dev decides
/// per model: Codex's own presets (`codex debug models`) list low..max, plus `ultra` on six, and
/// never `none`, and models.dev lacks `ultra`, `gpt-reserve` and `codex-auto-review`.
const CODEX: HostSpec = HostSpec {
    display_name: "Codex",
    tools: tool_natives::CODEX,
    scopes_patterns: true,
    mcp: McpNaming::NoName("Codex per-tool MCP gating lives in server config, not the tool list"),
    effort: Some(&EffortLevel::LADDER),
};

/// Pi 1.5.0 `--thinking` help: off, minimal, low, medium, high, xhigh, max (`none` is `off`).
const PI: HostSpec = HostSpec {
    display_name: "Pi",
    tools: tool_natives::PI,
    scopes_patterns: true,
    mcp: McpNaming::NoName("Harness has no MCP tool surface"),
    effort: Some(&EffortLevel::LADDER),
};

/// OpenCode 2.3.0 decodes models.dev `reasoning_options` itself (the string is in its binary).
const OPENCODE: HostSpec = HostSpec {
    display_name: "OpenCode",
    tools: tool_natives::OPENCODE,
    scopes_patterns: true,
    mcp: McpNaming::Format(|server, tool| format!("{server}_{tool}")),
    effort: None,
};

const CURSOR: HostSpec = HostSpec {
    display_name: "Cursor",
    tools: tool_natives::CURSOR,
    scopes_patterns: true,
    mcp: McpNaming::Format(|server, tool| format!("Mcp({server}:{tool})")),
    effort: None,
};

/// `agy models` 1.5.12.
const AGY: HostSpec = HostSpec {
    display_name: "Antigravity",
    tools: tool_natives::AGY,
    scopes_patterns: false,
    mcp: McpNaming::Ungated,
    effort: None,
};

/// meridian-runtime `src/gateway/domain/types.ts:63` (`REASONING_EFFORTS`), per Flow's gateway
/// lead.
const FLOW: HostSpec = HostSpec {
    display_name: "Meridian Flow",
    tools: tool_natives::FLOW,
    scopes_patterns: false,
    mcp: McpNaming::ConnectionTools,
    effort: Some(&[EffortLevel::None, Low, Medium, High, Max]),
};

impl Host {
    pub(crate) fn spec(self) -> &'static HostSpec {
        match self {
            Self::Harness(HarnessId::Claude) => &CLAUDE,
            Self::Harness(HarnessId::Codex) => &CODEX,
            Self::Harness(HarnessId::Pi) => &PI,
            Self::Harness(HarnessId::OpenCode) => &OPENCODE,
            Self::Harness(HarnessId::Cursor) => &CURSOR,
            Self::Harness(HarnessId::Agy) => &AGY,
            Self::MeridianFlow => &FLOW,
        }
    }
}

impl Serialize for Host {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// Every harness, in the order error messages list them. A new variant is added here and to
/// [`HarnessId::as_str`] and [`Host::spec`], which the compiler enforces.
pub const ALL: &[HarnessId] = &[
    HarnessId::Claude,
    HarnessId::Codex,
    HarnessId::Pi,
    HarnessId::OpenCode,
    HarnessId::Cursor,
    HarnessId::Agy,
];

/// The harness names in `ALL`'s order, as error messages list them.
pub fn names() -> String {
    ALL.iter()
        .map(|harness| harness.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Consumer namespaces under `harness-overrides` that are not harnesses: the consumer
/// owns the block's contents, and Mars only keeps it. Each is the host's full name: Meridian
/// Flow's is `meridian-flow`, the Meridian CLI's is `meridian-cli`.
pub(crate) const RESERVED_CONSUMER_NAMESPACES: &[&str] = &["meridian-flow", "meridian-cli"];

pub fn parse(name: &str) -> Option<HarnessId> {
    let normalized = name.trim().to_ascii_lowercase();
    ALL.iter()
        .copied()
        .find(|harness| harness.as_str() == normalized)
}

pub fn normalize_name(name: &str) -> Option<String> {
    parse(name).map(|id| id.as_str().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_normalize_are_case_insensitive() {
        assert_eq!(parse("OpenCode"), Some(HarnessId::OpenCode));
        assert_eq!(normalize_name(" OpenCode "), Some("opencode".to_string()));
        assert_eq!(parse("gemini"), None);
    }

    #[test]
    fn serde_names_round_trip_registry_names() {
        for &harness in ALL {
            let json = serde_json::to_string(&harness).unwrap();
            assert_eq!(json, format!("\"{}\"", harness.as_str()));
            assert_eq!(serde_json::from_str::<HarnessId>(&json).unwrap(), harness);
        }
        assert!(serde_json::from_str::<HarnessId>("\"open_code\"").is_err());
    }
}
