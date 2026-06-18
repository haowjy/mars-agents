use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessId {
    Claude,
    Codex,
    Pi,
    OpenCode,
    Cursor,
}

impl HarnessId {
    pub fn as_str(self) -> &'static str {
        descriptor(self).name
    }

    pub fn default_target(self) -> &'static str {
        descriptor(self).default_target
    }
}

impl fmt::Display for HarnessId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarnessClass {
    Native { provider: &'static str },
    ProbeBacked,
    UniversalPassthrough,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HarnessDescriptor {
    pub id: HarnessId,
    pub name: &'static str,
    pub binary: &'static str,
    pub default_target: &'static str,
    pub class: HarnessClass,
}

/// Default launch-bundle harness try order when `settings.harness_order` is unset.
pub const DEFAULT_HARNESS_ORDER: &[HarnessId] = &[
    HarnessId::Claude,
    HarnessId::Codex,
    HarnessId::Pi,
    HarnessId::Cursor,
    HarnessId::OpenCode,
];

pub fn default_harness_order_names() -> Vec<String> {
    DEFAULT_HARNESS_ORDER
        .iter()
        .map(|harness| harness.as_str().to_string())
        .collect()
}

const DESCRIPTORS: &[HarnessDescriptor] = &[
    HarnessDescriptor {
        id: HarnessId::Claude,
        name: "claude",
        binary: "claude",
        default_target: ".claude",
        class: HarnessClass::Native {
            provider: "anthropic",
        },
    },
    HarnessDescriptor {
        id: HarnessId::Codex,
        name: "codex",
        binary: "codex",
        default_target: ".codex",
        class: HarnessClass::Native { provider: "openai" },
    },
    HarnessDescriptor {
        id: HarnessId::Pi,
        name: "pi",
        binary: "pi",
        default_target: ".pi",
        class: HarnessClass::ProbeBacked,
    },
    HarnessDescriptor {
        id: HarnessId::OpenCode,
        name: "opencode",
        binary: "opencode",
        default_target: ".opencode",
        class: HarnessClass::ProbeBacked,
    },
    HarnessDescriptor {
        id: HarnessId::Cursor,
        name: "cursor",
        binary: "cursor",
        default_target: ".cursor",
        class: HarnessClass::ProbeBacked,
    },
];

pub fn descriptors() -> &'static [HarnessDescriptor] {
    DESCRIPTORS
}

pub fn all() -> &'static [HarnessId] {
    &[
        HarnessId::Claude,
        HarnessId::Codex,
        HarnessId::Pi,
        HarnessId::OpenCode,
        HarnessId::Cursor,
    ]
}

pub fn names() -> &'static [&'static str] {
    &["claude", "codex", "pi", "cursor", "opencode"]
}

pub fn descriptor(id: HarnessId) -> &'static HarnessDescriptor {
    DESCRIPTORS
        .iter()
        .find(|descriptor| descriptor.id == id)
        .expect("harness descriptor exists")
}

pub fn parse(name: &str) -> Option<HarnessId> {
    let normalized = name.trim().to_ascii_lowercase();
    DESCRIPTORS
        .iter()
        .find(|descriptor| descriptor.name == normalized)
        .map(|descriptor| descriptor.id)
}

pub fn is_known(name: &str) -> bool {
    parse(name).is_some()
}

pub fn normalize_name(name: &str) -> Option<String> {
    parse(name).map(|id| id.as_str().to_string())
}

pub fn default_target_for_name(name: &str) -> Option<&'static str> {
    parse(name).map(|id| id.default_target())
}

pub fn native_provider_for(id: HarnessId) -> Option<&'static str> {
    match descriptor(id).class {
        HarnessClass::Native { provider } => Some(provider),
        HarnessClass::ProbeBacked | HarnessClass::UniversalPassthrough => None,
    }
}

pub fn native_harness_for_provider(provider: &str) -> Option<HarnessId> {
    let normalized = provider.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "anthropic" => Some(HarnessId::Claude),
        "openai" => Some(HarnessId::Codex),
        _ => None,
    }
}

pub fn provider_candidate_order(provider: &str) -> Vec<HarnessId> {
    derive_provider_candidate_order(native_harness_for_provider(provider))
}

fn derive_provider_candidate_order(native_harness: Option<HarnessId>) -> Vec<HarnessId> {
    match native_harness {
        Some(native) => {
            let mut order = vec![native];
            order.extend(
                DEFAULT_HARNESS_ORDER
                    .iter()
                    .copied()
                    .filter(|harness| *harness != native),
            );
            order
        }
        None => DEFAULT_HARNESS_ORDER.to_vec(),
    }
}

pub fn is_known_provider(provider: &str) -> bool {
    matches!(
        provider.trim().to_ascii_lowercase().as_str(),
        "anthropic" | "openai" | "google" | "meta" | "mistral" | "deepseek" | "cohere"
    )
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
    fn provider_candidate_order_derives_from_default_harness_order() {
        assert_eq!(
            provider_candidate_order("openai"),
            derive_provider_candidate_order(Some(HarnessId::Codex))
        );
        assert_eq!(
            provider_candidate_order("anthropic"),
            derive_provider_candidate_order(Some(HarnessId::Claude))
        );
        assert_eq!(
            provider_candidate_order("unknown"),
            derive_provider_candidate_order(None)
        );
        assert_eq!(
            provider_candidate_order("google"),
            DEFAULT_HARNESS_ORDER.to_vec()
        );
    }

    #[test]
    fn descriptor_cursor_is_first_class() {
        let cursor = descriptor(HarnessId::Cursor);
        assert_eq!(cursor.default_target, ".cursor");
    }
}
