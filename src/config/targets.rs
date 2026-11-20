use std::collections::BTreeSet;
use std::collections::HashSet;
use std::path::PathBuf;

use mars_schema::HarnessId;

use super::Settings;

/// Permission to route through configured harness targets. An empty set is not autodiscovery.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum HarnessScope {
    #[default]
    Unrestricted,
    Only(BTreeSet<HarnessId>),
}

impl HarnessScope {
    pub fn permits(&self, harness: &str) -> bool {
        mars_schema::harness::parse(harness).is_some_and(|id| match self {
            Self::Unrestricted => true,
            Self::Only(enabled) => enabled.contains(&id),
        })
    }

    pub fn harness_names(&self) -> Option<Vec<String>> {
        match self {
            Self::Unrestricted => None,
            Self::Only(enabled) => Some(enabled.iter().map(ToString::to_string).collect()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedLink {
    pub raw: String,
    pub target: String,
    pub harness: Option<HarnessId>,
    pub kind: LinkKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    KnownHarness,
    GenericTarget,
    PathLike,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LinkSource {
    Targets,
    ManagedRoot,
    #[default]
    None,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TargetOrigin {
    Project,
    Local,
    #[default]
    Unset,
}

/// Winning target field and file, recorded during configuration merging.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TargetSource {
    pub field: LinkSource,
    pub origin: TargetOrigin,
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveLinks {
    pub links: Vec<NormalizedLink>,
    pub source: LinkSource,
}

impl EffectiveLinks {
    pub fn harness_scope(&self) -> HarnessScope {
        match self.source {
            LinkSource::None => HarnessScope::Unrestricted,
            LinkSource::Targets | LinkSource::ManagedRoot => {
                HarnessScope::Only(self.linked_harnesses_set())
            }
        }
    }

    pub fn managed_targets(&self) -> Vec<String> {
        let mut seen = BTreeSet::new();
        let mut targets = Vec::new();
        for link in &self.links {
            if seen.insert(link.target.clone()) {
                targets.push(link.target.clone());
            }
        }
        targets
    }

    pub fn linked_harnesses(&self) -> Vec<HarnessId> {
        let mut seen = HashSet::new();
        let mut harnesses = Vec::new();
        for harness in self.links.iter().filter_map(|link| link.harness) {
            if seen.insert(harness) {
                harnesses.push(harness);
            }
        }
        harnesses
    }

    pub fn linked_harnesses_set(&self) -> BTreeSet<HarnessId> {
        self.linked_harnesses().into_iter().collect()
    }
}

pub fn normalize_link(raw: &str) -> NormalizedLink {
    let trimmed = raw.trim().trim_end_matches('/').trim_end_matches('\\');

    if trimmed.contains('/') || trimmed.contains('\\') {
        return NormalizedLink {
            raw: raw.to_string(),
            target: trimmed.to_string(),
            harness: None,
            kind: LinkKind::PathLike,
        };
    }

    let bare = trimmed.strip_prefix('.').unwrap_or(trimmed);
    // A link names a harness by its name or by its target directory.
    let harness = mars_schema::harness::parse(bare)
        .or_else(|| crate::harness::registry::harness_for_target(&format!(".{bare}")));
    if let Some(harness) = harness {
        return NormalizedLink {
            raw: raw.to_string(),
            target: crate::harness::registry::descriptor(harness)
                .default_target
                .to_string(),
            harness: Some(harness),
            kind: LinkKind::KnownHarness,
        };
    }

    if bare.is_empty() {
        return NormalizedLink {
            raw: raw.to_string(),
            target: trimmed.to_string(),
            harness: None,
            kind: LinkKind::GenericTarget,
        };
    }

    NormalizedLink {
        raw: raw.to_string(),
        target: format!(".{bare}"),
        harness: None,
        kind: LinkKind::GenericTarget,
    }
}
pub fn effective_links(settings: &Settings) -> EffectiveLinks {
    if let Some(targets) = settings.targets.as_deref() {
        return EffectiveLinks {
            links: targets
                .iter()
                .map(|target| normalize_link(target))
                .collect(),
            source: LinkSource::Targets,
        };
    }

    if let Some(managed_root) = settings.managed_root.as_ref() {
        return EffectiveLinks {
            links: vec![normalize_link(managed_root)],
            source: LinkSource::ManagedRoot,
        };
    }

    EffectiveLinks {
        links: Vec::new(),
        source: LinkSource::None,
    }
}

/// The managed target directories: `targets` as normalized, else the normalized `managed_root`
/// (the older spelling), else none (`.mars/` stays the canonical compiled store).
pub fn managed_targets(settings: &Settings) -> Vec<String> {
    effective_links(settings).managed_targets()
}

/// Harnesses that would receive native agent artifacts during sync for these settings.
pub(crate) fn configured_emit_harnesses(settings: &Settings) -> Vec<HarnessId> {
    managed_targets(settings)
        .iter()
        .filter_map(|target| crate::harness::registry::harness_for_target(target))
        .collect()
}

/// The known harnesses the configured links name; generic targets are ignored.
pub fn linked_harnesses(settings: &Settings) -> Vec<String> {
    effective_links(settings)
        .linked_harnesses()
        .into_iter()
        .map(|harness| harness.to_string())
        .collect()
}
