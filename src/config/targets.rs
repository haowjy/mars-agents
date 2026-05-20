use std::collections::BTreeSet;
use std::collections::HashSet;

use crate::harness::registry::HarnessId;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkSource {
    Targets,
    ManagedRoot,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveLinks {
    pub links: Vec<NormalizedLink>,
    pub source: LinkSource,
}

impl EffectiveLinks {
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
    if let Some(harness) = crate::harness::registry::parse(bare) {
        return NormalizedLink {
            raw: raw.to_string(),
            target: harness.default_target().to_string(),
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

pub fn normalized_targets<'a>(links: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut targets = Vec::new();
    for link in links {
        let target = normalize_link(link).target;
        if seen.insert(target.clone()) {
            targets.push(target);
        }
    }
    targets
}

pub fn linked_harnesses<'a>(links: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut harnesses = Vec::new();

    for link in links {
        if let Some(harness) = normalize_link(link).harness {
            let name = harness.as_str().to_string();
            if seen.insert(name.clone()) {
                harnesses.push(name);
            }
        }
    }

    harnesses
}

pub fn effective_links(
    targets: Option<&[String]>,
    managed_root: Option<&String>,
) -> EffectiveLinks {
    if let Some(targets) = targets {
        return EffectiveLinks {
            links: targets
                .iter()
                .map(|target| normalize_link(target))
                .collect(),
            source: LinkSource::Targets,
        };
    }

    if let Some(managed_root) = managed_root {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_harness_name_and_legacy_path_form() {
        assert_eq!(
            normalize_link("codex"),
            NormalizedLink {
                raw: "codex".to_string(),
                target: ".codex".to_string(),
                harness: Some(HarnessId::Codex),
                kind: LinkKind::KnownHarness,
            }
        );
        assert_eq!(
            normalize_link(".codex"),
            NormalizedLink {
                raw: ".codex".to_string(),
                target: ".codex".to_string(),
                harness: Some(HarnessId::Codex),
                kind: LinkKind::KnownHarness,
            }
        );
    }

    #[test]
    fn normalizes_agents_as_generic_target() {
        assert_eq!(
            normalize_link("agents"),
            NormalizedLink {
                raw: "agents".to_string(),
                target: ".agents".to_string(),
                harness: None,
                kind: LinkKind::GenericTarget,
            }
        );
    }

    #[test]
    fn extracts_known_harnesses_only() {
        let links = [".codex", ".claude", ".agents", "foo/bar"];
        assert_eq!(
            linked_harnesses(links.iter().copied()),
            vec!["codex".to_string(), "claude".to_string()]
        );
    }
}
