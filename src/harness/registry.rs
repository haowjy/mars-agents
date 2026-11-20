use crate::models::probes::{
    AntigravityProbeResult, CursorProbeResult, OpenCodeProbeResult, PiProbeResult, ProbeDriver,
    cursor,
};
use mars_schema::HarnessId;
use mars_schema::agent::EffortLevel;

/// How a harness proves runtime support and authentication.
#[derive(Debug, Clone, Copy)]
pub enum HarnessClass {
    /// Serves one provider directly; support comes from the catalog and auth from a
    /// native status command.
    Native { provider: &'static str },
    /// Support comes from the harness's own model listing.
    ProbeBacked(ProbeSpec),
}

/// What a probe-backed harness is: how its listing is fetched and cached, what a good listing
/// proves about credentials, and how it spells models.
#[derive(Debug, Clone, Copy)]
pub struct ProbeSpec {
    pub probe: ProbeDriver,
    pub listing: ListingAuth,
    pub slugs: SlugShape,
    /// Set when the harness folds an effort level into its slugs.
    pub tiers: Option<SlugTiers>,
}

/// How a harness folds an effort level into a model's slugs (`<model>-<level>`); the module
/// `models/slug_tiers.rs` splits and joins them.
#[derive(Debug, Clone, Copy)]
pub struct SlugTiers {
    /// The level a slug with no suffix stands for, if it stands for one.
    pub bare_level: Option<EffortLevel>,
    /// The harness's own spelling of a model models.dev spells differently, tried when the
    /// models.dev spelling lists no slug.
    pub respell: fn(&str) -> Option<String>,
}

/// How a probe-backed harness spells the models it lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlugShape {
    /// `provider/model`: the listing names the providers it serves.
    Qualified,
    /// The harness's own flat names (Cursor's `gpt-5.5-high`), with no provider.
    Bare,
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

#[derive(Debug, Clone, Copy)]
/// The one row per harness: identity-derived facts (`as_str`, the `.claude`-style variant key)
/// are not stored. Lowering policy is a match on `HarnessId` in the compiler.
pub struct HarnessDescriptor {
    pub id: HarnessId,
    pub display_name: &'static str,
    pub binary: &'static str,
    pub default_target: &'static str,
    pub class: HarnessClass,
}

/// Default launch-bundle harness try order when `settings.harness_order` is unset. A harness
/// joins it only when Meridian can launch it; Antigravity is reached through `harness_order` or
/// an alias that names it.
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
        display_name: "Claude",
        binary: "claude",
        default_target: ".claude",
        class: HarnessClass::Native {
            provider: "anthropic",
        },
    },
    HarnessDescriptor {
        id: HarnessId::Codex,
        display_name: "Codex",
        binary: "codex",
        default_target: ".codex",
        class: HarnessClass::Native { provider: "openai" },
    },
    HarnessDescriptor {
        id: HarnessId::Pi,
        display_name: "Pi",
        binary: "pi",
        default_target: ".pi",
        class: HarnessClass::ProbeBacked(ProbeSpec {
            probe: ProbeDriver::of::<PiProbeResult>(),
            listing: ListingAuth::Gated,
            slugs: SlugShape::Qualified,
            tiers: None,
        }),
    },
    HarnessDescriptor {
        id: HarnessId::OpenCode,
        display_name: "OpenCode",
        binary: "opencode",
        default_target: ".opencode",
        class: HarnessClass::ProbeBacked(ProbeSpec {
            probe: ProbeDriver::of::<OpenCodeProbeResult>(),
            listing: ListingAuth::Ungated,
            slugs: SlugShape::Qualified,
            tiers: None,
        }),
    },
    HarnessDescriptor {
        id: HarnessId::Cursor,
        display_name: "Cursor",
        binary: "cursor",
        default_target: ".cursor",
        class: HarnessClass::ProbeBacked(ProbeSpec {
            probe: ProbeDriver::of::<CursorProbeResult>(),
            listing: ListingAuth::Gated,
            slugs: SlugShape::Bare,
            tiers: Some(SlugTiers {
                bare_level: Some(EffortLevel::Medium),
                respell: cursor::model_id_shim,
            }),
        }),
    },
    HarnessDescriptor {
        id: HarnessId::Antigravity,
        display_name: "Antigravity",
        binary: "agy",
        default_target: ".agy",
        class: HarnessClass::ProbeBacked(ProbeSpec {
            probe: ProbeDriver::of::<AntigravityProbeResult>(),
            listing: ListingAuth::Gated,
            slugs: SlugShape::Bare,
            tiers: Some(SlugTiers {
                bare_level: None,
                respell: |_| None,
            }),
        }),
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

/// The probe-backed harness's spec, or `None` for a native harness.
pub fn probe_spec(id: HarnessId) -> Option<ProbeSpec> {
    match descriptor(id).class {
        HarnessClass::ProbeBacked(spec) => Some(spec),
        HarnessClass::Native { .. } => None,
    }
}

/// How the harness folds an effort level into its slugs, or `None` when it does not.
pub fn slug_tiers(id: HarnessId) -> Option<SlugTiers> {
    probe_spec(id)?.tiers
}

pub fn native_provider(id: HarnessId) -> Option<&'static str> {
    match descriptor(id).class {
        HarnessClass::Native { provider } => Some(provider),
        HarnessClass::ProbeBacked(_) => None,
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
            HarnessClass::ProbeBacked(_) => false,
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
            (Antigravity, Some(ListingAuth::Gated)),
        ] {
            let listing = match descriptor(harness).class {
                HarnessClass::Native { .. } => None,
                HarnessClass::ProbeBacked(spec) => Some(spec.listing),
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
