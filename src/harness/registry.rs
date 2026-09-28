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

    pub fn class(self) -> HarnessClass {
        descriptor(self).class
    }

    /// Provider a native harness serves directly; `None` for probe-backed harnesses.
    pub fn native_provider(self) -> Option<&'static str> {
        match self.class() {
            HarnessClass::Native { provider } => Some(provider),
            HarnessClass::ProbeBacked { .. } => None,
        }
    }
}

impl fmt::Display for HarnessId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How a harness proves runtime support and authentication.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarnessClass {
    /// Serves one provider directly; support comes from the catalog and auth from a
    /// native status command.
    Native { provider: &'static str },
    /// Support comes from the harness's own model listing.
    ProbeBacked { listing: ListingAuth },
}

/// Whether a probe-backed harness lists models only when credentials are configured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListingAuth {
    /// The listing requires configured credentials (Pi lists only providers with auth
    /// configured; Cursor refuses to list when logged out). A successful listing is auth
    /// evidence: credentials are configured, not proven valid.
    Gated,
    /// The listing enumerates the provider catalog without checking credentials
    /// (OpenCode). It proves support only.
    Ungated,
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
        class: HarnessClass::ProbeBacked {
            listing: ListingAuth::Gated,
        },
    },
    HarnessDescriptor {
        id: HarnessId::OpenCode,
        name: "opencode",
        binary: "opencode",
        default_target: ".opencode",
        class: HarnessClass::ProbeBacked {
            listing: ListingAuth::Ungated,
        },
    },
    HarnessDescriptor {
        id: HarnessId::Cursor,
        name: "cursor",
        binary: "cursor",
        default_target: ".cursor",
        class: HarnessClass::ProbeBacked {
            listing: ListingAuth::Gated,
        },
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

pub fn native_harness_for_provider(provider: &str) -> Option<HarnessId> {
    let normalized = provider.trim().to_ascii_lowercase();
    DESCRIPTORS
        .iter()
        .find(|descriptor| descriptor.id.native_provider() == Some(normalized.as_str()))
        .map(|descriptor| descriptor.id)
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
    fn listing_auth_is_gated_only_for_credential_filtered_listings() {
        use HarnessId::*;
        for (harness, expected) in [
            (Claude, None),
            (Codex, None),
            (Pi, Some(ListingAuth::Gated)),
            (Cursor, Some(ListingAuth::Gated)),
            (OpenCode, Some(ListingAuth::Ungated)),
        ] {
            let listing = match harness.class() {
                HarnessClass::Native { .. } => None,
                HarnessClass::ProbeBacked { listing } => Some(listing),
            };
            assert_eq!(listing, expected, "{harness}");
        }
    }

    #[test]
    fn provider_candidates_prefer_native_then_follow_default_order() {
        use HarnessId::*;
        for (provider, expected) in [
            ("openai", vec![Codex, Claude, Pi, Cursor, OpenCode]),
            ("anthropic", vec![Claude, Codex, Pi, Cursor, OpenCode]),
            ("unknown", vec![Claude, Codex, Pi, Cursor, OpenCode]),
            ("google", vec![Claude, Codex, Pi, Cursor, OpenCode]),
        ] {
            assert_eq!(provider_candidate_order(provider), expected, "{provider}");
        }
    }
}
