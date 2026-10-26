use mars_schema::HarnessId;

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

pub fn descriptor(id: HarnessId) -> &'static HarnessDescriptor {
    DESCRIPTORS
        .iter()
        .find(|descriptor| descriptor.id == id)
        .expect("harness descriptor exists")
}

pub fn native_provider(id: HarnessId) -> Option<&'static str> {
    match descriptor(id).class {
        HarnessClass::Native { provider } => Some(provider),
        HarnessClass::ProbeBacked { .. } => None,
    }
}

pub fn harness_for_target(target: &str) -> Option<HarnessId> {
    DESCRIPTORS
        .iter()
        .find(|descriptor| descriptor.default_target == target)
        .map(|descriptor| descriptor.id)
}

pub fn native_harness_for_provider(provider: &str) -> Option<HarnessId> {
    let normalized = provider.trim().to_ascii_lowercase();
    DESCRIPTORS
        .iter()
        .find(|descriptor| match descriptor.class {
            HarnessClass::Native { provider } => provider == normalized,
            HarnessClass::ProbeBacked { .. } => false,
        })
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
    fn listing_auth_is_gated_only_for_credential_filtered_listings() {
        use HarnessId::*;
        for (harness, expected) in [
            (Claude, None),
            (Codex, None),
            (Pi, Some(ListingAuth::Gated)),
            (Cursor, Some(ListingAuth::Gated)),
            (OpenCode, Some(ListingAuth::Ungated)),
        ] {
            let listing = match descriptor(harness).class {
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
