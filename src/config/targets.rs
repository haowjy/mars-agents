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
            LinkSource::Targets => HarnessScope::Only(self.linked_harnesses_set()),
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
    let harness = mars_schema::harness::parse(bare);
    if let Some(harness) = harness {
        return NormalizedLink {
            raw: raw.to_string(),
            target: harness.target_dir(),
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
    let managed_root = settings.managed_root.as_slice();
    let targets = settings
        .targets
        .as_deref()
        .or(Some(managed_root).filter(|r| !r.is_empty()));
    if let Some(targets) = targets {
        return EffectiveLinks {
            links: targets
                .iter()
                .map(|target| normalize_link(target))
                .collect(),
            source: LinkSource::Targets,
        };
    }

    EffectiveLinks {
        links: Vec::new(),
        source: LinkSource::None,
    }
}

/// Move a file's `managed_root` into its `targets`, or drop it when `targets` is set. Every
/// writer of `targets` calls this on the file's own settings first, so a saved file never
/// carries a `managed_root` that the next read would layer over what the writer meant.
pub fn retire_managed_root(managed_root: &mut Option<String>, targets: &mut Option<Vec<String>>) {
    if let Some(root) = managed_root.take()
        && targets.is_none()
    {
        *targets = Some(vec![root]);
    }
}

/// The managed target directories: `targets` as normalized, else none (`.mars/` stays the
/// canonical compiled store).
pub fn managed_targets(settings: &Settings) -> Vec<String> {
    effective_links(settings).managed_targets()
}

/// The advisories about the targets in `managed`: one line each, whatever the number of items.
pub(crate) fn target_advisories(managed: &[String]) -> Vec<crate::diagnostic::Diagnostic> {
    agents_target_diagnostic(managed)
        .into_iter()
        .chain(cursor_target_diagnostic(managed))
        .collect()
}

/// Cursor support is removed in the minor after 0.37.0. Until then it works, so this is the one
/// notice that tells the user to unlink it.
fn cursor_target_diagnostic(managed: &[String]) -> Option<crate::diagnostic::Diagnostic> {
    use crate::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticLevel};

    let cursor = managed.iter().find(|target| {
        crate::harness::registry::harness_for_target(target) == Some(HarnessId::Cursor)
    })?;
    let unlink = format!("mars unlink {cursor}");
    let unlink = crate::types::managed_cmd(&unlink);
    Some(Diagnostic {
        category: Some(DiagnosticCategory::Advisory),
        ..Diagnostic::new(
            DiagnosticLevel::Warning,
            "deprecated-cursor-target",
            format!(
                "Cursor support is deprecated and goes away in the next minor release, so `{cursor}` will stop being written. Run `{unlink}` to remove it."
            ),
        )
    })
}

/// What linking `.agents` among `managed` means for the user, or `None` when it is not linked.
/// `.agents` is deprecated, which loses nothing; but a harness that also reads `.agents/skills`
/// (`agy`) then sees every skill twice under one name, and only unlinking `.agents` fixes that.
/// Neither says the sync fell short of what it wanted.
fn agents_target_diagnostic(managed: &[String]) -> Option<crate::diagnostic::Diagnostic> {
    use crate::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticLevel};
    use crate::target::AGENTS_TARGET;

    if !managed.iter().any(|target| target == AGENTS_TARGET) {
        return None;
    }
    let unlink = crate::types::managed_cmd("mars unlink .agents");
    let reader = managed
        .iter()
        .find(|target| crate::target::spec(target).is_some_and(|spec| spec.reads_agents_skills));
    let (level, message) = match reader {
        Some(reader) => (
            DiagnosticLevel::Warning,
            format!(
                "`{AGENTS_TARGET}` and `{reader}` are both linked, and the harness for `{reader}` also reads skills from `{AGENTS_TARGET}/skills`, so it sees each skill twice under the same name. Run `{unlink}` to remove it."
            ),
        ),
        None => (
            DiagnosticLevel::Info,
            format!(
                "`{AGENTS_TARGET}` is a deprecated link target. Run `{unlink}` to remove it. Skills are now emitted to native harness dirs automatically."
            ),
        ),
    };
    Some(Diagnostic {
        category: Some(DiagnosticCategory::Advisory),
        ..Diagnostic::new(level, "deprecated-agents-target", message)
    })
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
