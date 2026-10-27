//! Stable harness identifiers and authored-name parsing.

use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessId {
    Claude,
    Codex,
    Pi,
    #[serde(rename = "opencode")]
    OpenCode,
    Cursor,
}

impl HarnessId {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Pi => "pi",
            Self::OpenCode => "opencode",
            Self::Cursor => "cursor",
        }
    }
}

impl fmt::Display for HarnessId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

pub const ALL: &[HarnessId] = &[
    HarnessId::Claude,
    HarnessId::Codex,
    HarnessId::Pi,
    HarnessId::OpenCode,
    HarnessId::Cursor,
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

/// The namespace spellings before the full host names; the reader reports them as removed and
/// `mars migrate` rewrites them to the name beside them.
pub(crate) const RENAMED_CONSUMER_NAMESPACES: &[(&str, &str)] =
    &[("flow", "meridian-flow"), ("meridian", "meridian-cli")];

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
