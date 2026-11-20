use crate::models::probes::{AgyProbeResult, OpenCodeProbeResult, PiProbeResult, ProbeDriver};
use mars_schema::HarnessId;

/// How a harness proves runtime support and authentication.
#[derive(Debug, Clone, Copy)]
pub enum HarnessClass {
    /// Serves one provider directly; support comes from the catalog and auth from a
    /// native status command.
    Native {
        provider: &'static str,
        /// The arguments that print the harness's login state; their exit status is the answer.
        auth_status: &'static [&'static str],
    },
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
}

/// How a probe-backed harness spells the models it lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlugShape {
    /// `provider/model`: the listing names the providers it serves.
    Qualified,
    /// The harness's own flat names, with no provider. Antigravity lists one slug per effort level
    /// of a model, spelled `<model>-<level>` (`gemini-3.8-flash-low`); Mars launches none of them.
    Bare,
}

/// Whether a probe-backed harness lists models only when credentials are configured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListingAuth {
    /// The listing requires configured credentials (Pi lists only providers with auth
    /// configured). A successful listing is auth
    /// evidence: credentials are configured, not proven valid.
    Gated,
    /// The listing enumerates the provider catalog without checking credentials
    /// (OpenCode). It proves support only.
    Ungated,
}

#[derive(Debug, Clone, Copy)]
/// The one row per harness. Facts derived from the id are not stored: the executable is
/// `id.as_str()`, the target directory `.{id}`, and the skill variant key `id.as_str()`.
/// Lowering policy is a table in the compiler, so `src/harness` never depends on it.
pub struct HarnessDescriptor {
    pub class: HarnessClass,
    /// Whether Meridian launches this harness. Under Meridian a launched harness reads its agents
    /// from Meridian, so Mars leaves its native agent files out (`agent_emission`); one it does
    /// not launch is run by hand, and Mars writes its agents.
    pub launched_by_meridian: bool,
}

/// Default launch-bundle harness try order when `settings.harness_order` is unset. A harness
/// joins it only when Meridian can launch it. Routing never picks a harness whose row has
/// `launched_by_meridian: false`, whoever names it (`routing::permission_denial`).
pub const DEFAULT_HARNESS_ORDER: &[HarnessId] = &[
    HarnessId::Claude,
    HarnessId::Codex,
    HarnessId::Pi,
    HarnessId::OpenCode,
];

pub fn default_harness_order_names() -> Vec<String> {
    DEFAULT_HARNESS_ORDER
        .iter()
        .map(|harness| harness.as_str().to_string())
        .collect()
}

const CLAUDE: HarnessDescriptor = HarnessDescriptor {
    class: HarnessClass::Native {
        provider: "anthropic",
        auth_status: &["auth", "status"],
    },
    launched_by_meridian: true,
};

const CODEX: HarnessDescriptor = HarnessDescriptor {
    class: HarnessClass::Native {
        provider: "openai",
        auth_status: &["login", "status"],
    },
    launched_by_meridian: true,
};

const PI: HarnessDescriptor = HarnessDescriptor {
    class: HarnessClass::ProbeBacked(ProbeSpec {
        probe: ProbeDriver::of::<PiProbeResult>(),
        listing: ListingAuth::Gated,
        slugs: SlugShape::Qualified,
    }),
    launched_by_meridian: true,
};

const OPENCODE: HarnessDescriptor = HarnessDescriptor {
    class: HarnessClass::ProbeBacked(ProbeSpec {
        probe: ProbeDriver::of::<OpenCodeProbeResult>(),
        listing: ListingAuth::Ungated,
        slugs: SlugShape::Qualified,
    }),
    launched_by_meridian: true,
};

const AGY: HarnessDescriptor = HarnessDescriptor {
    class: HarnessClass::ProbeBacked(ProbeSpec {
        probe: ProbeDriver::of::<AgyProbeResult>(),
        listing: ListingAuth::Gated,
        slugs: SlugShape::Bare,
    }),
    launched_by_meridian: false,
};

pub fn launched_by_meridian(id: HarnessId) -> bool {
    descriptor(id).launched_by_meridian
}

pub fn descriptor(id: HarnessId) -> &'static HarnessDescriptor {
    match id {
        HarnessId::Claude => &CLAUDE,
        HarnessId::Codex => &CODEX,
        HarnessId::Pi => &PI,
        HarnessId::OpenCode => &OPENCODE,
        HarnessId::Agy => &AGY,
    }
}

/// The probe-backed harness's spec, or `None` for a native harness.
pub fn probe_spec(id: HarnessId) -> Option<ProbeSpec> {
    match descriptor(id).class {
        HarnessClass::ProbeBacked(spec) => Some(spec),
        HarnessClass::Native { .. } => None,
    }
}

/// Whether the harness spells its models `provider/model`, so a provider constraint joins the
/// model it names.
pub fn qualifies_slugs(id: HarnessId) -> bool {
    probe_spec(id).is_some_and(|spec| spec.slugs == SlugShape::Qualified)
}

pub fn native_provider(id: HarnessId) -> Option<&'static str> {
    match descriptor(id).class {
        HarnessClass::Native { provider, .. } => Some(provider),
        HarnessClass::ProbeBacked(_) => None,
    }
}

/// The harness whose native directory `target` names (`.claude`); `None` for `.agents` and any
/// other name.
pub fn harness_for_target(target: &str) -> Option<HarnessId> {
    let name = target.strip_prefix('.')?;
    mars_schema::harness::ALL
        .iter()
        .copied()
        .find(|id| id.as_str() == name)
}

/// Whether `target` takes the content a host provides (`mars provide`): a native directory of a
/// harness Meridian does not launch, so nothing else installs a host's skills, hooks and MCP
/// servers there. `.agents` and a custom directory take none.
pub fn takes_provided(target: &str) -> bool {
    harness_for_target(target).is_some_and(|id| !launched_by_meridian(id))
}

pub fn native_harness_for_provider(provider: &str) -> Option<HarnessId> {
    let normalized = provider.trim().to_ascii_lowercase();
    mars_schema::harness::ALL.iter().copied().find(|id| {
        matches!(descriptor(*id).class, HarnessClass::Native { provider, .. } if provider == normalized)
    })
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
            (OpenCode, Some(ListingAuth::Ungated)),
            (Agy, Some(ListingAuth::Gated)),
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
            ("openai", vec![Codex, Claude, Pi, OpenCode]),
            ("anthropic", vec![Claude, Codex, Pi, OpenCode]),
            ("unknown", vec![Claude, Codex, Pi, OpenCode]),
            ("google", vec![Claude, Codex, Pi, OpenCode]),
        ] {
            assert_eq!(provider_candidate_order(provider), expected, "{provider}");
        }
    }
}
