//! The skill body-link rule: which relative links leave a skill's payload and stop resolving
//! once the skills are installed.
//!
//! The rule judges an explicit installed view, so one rule serves both callers: `prepare` builds
//! the view from the package's own selection, and the binary builds it from what a sync actually
//! installs (after the consumer's filters and renames).
//!
//! Links are read from the canonical `SKILL.md` body; a per-target `variants/<harness>/SKILL.md`
//! body is not checked.

use std::collections::BTreeSet;

use crate::diagnostic::{SchemaDiagnostic, Severity};
use crate::discover::join;

/// The codes `skill_link_diagnostics` reports.
pub const LINK_CODES: [&str; 2] = ["skill.reference-to-skill", "skill.outside-reference"];

/// One skill of a package, as the link rule needs it.
pub struct SkillView<'a> {
    pub slug: &'a str,
    /// The skill's folder in the package tree, as its links are written; empty for a root skill.
    pub source_root: &'a str,
    /// Where the skill's folder is stored, or would be stored when it is not installed.
    pub stored_root: &'a str,
    pub installed: bool,
    /// The `SKILL.md` body; `None` when it could not be read, which leaves the skill unchecked.
    pub body: Option<&'a str>,
}

/// Body links that leave a skill's payload and will not resolve after install, by index into
/// `skills` (the same length and order).
pub fn skill_link_diagnostics(skills: &[SkillView<'_>]) -> Vec<Vec<SchemaDiagnostic>> {
    skills
        .iter()
        .map(|skill| match skill.body {
            Some(body) => reference_diagnostics(skill, body, skills),
            None => Vec::new(),
        })
        .collect()
}

fn reference_diagnostics(
    definition: &SkillView<'_>,
    body: &str,
    all: &[SkillView<'_>],
) -> Vec<SchemaDiagnostic> {
    let root = definition.source_root;
    let mut seen = BTreeSet::new();
    let mut diagnostics = Vec::new();
    for target in link_targets(body) {
        if !seen.insert(target.clone()) {
            continue;
        }
        let Some(resolved) = resolve(root, &target) else {
            diagnostics.push(outside(&target));
            continue;
        };
        // The linking skill is a candidate too, so a link into its own payload goes through the
        // same stored-path comparison. Payload enumeration carves child skill roots out of the
        // parent, so the longest root decides who owns a file.
        let owner = all
            .iter()
            .map(|other| (other, other.source_root))
            .filter(|(_, other_root)| {
                other_root.is_empty()
                    || resolved == *other_root
                    || resolved.starts_with(&format!("{other_root}/"))
            })
            .max_by_key(|(_, other_root)| other_root.len());
        let Some((other, other_root)) = owner else {
            diagnostics.push(outside(&target));
            continue;
        };
        let inside = resolved
            .strip_prefix(other_root)
            .unwrap_or("")
            .trim_start_matches('/');
        let stored = join(other.stored_root, inside);
        let imported = other.installed;
        // Every skill is stored under its own folder, so a link between imported skills keeps
        // working when it lands on the target's stored path from the linking skill's stored root.
        if imported && resolve(definition.stored_root, &target).as_deref() == Some(&stored) {
            continue;
        }
        diagnostics.push(SchemaDiagnostic::new(
            "skill.reference-to-skill",
            Severity::Warning,
            Vec::new(),
            if imported {
                format!(
                    "links to `{target}`, a file of skill `{}`, which does not resolve after install; link `{}` instead",
                    other.slug,
                    relative(definition.stored_root, &stored)
                )
            } else {
                format!(
                    "links to `{target}`, a file of skill `{}`, which is not imported; stored it would be `{stored}`",
                    other.slug
                )
            },
        ));
    }
    diagnostics
}

fn outside(target: &str) -> SchemaDiagnostic {
    SchemaDiagnostic::new(
        "skill.outside-reference",
        Severity::Warning,
        Vec::new(),
        format!("links to `{target}`, which is outside the skill and is not imported"),
    )
}

/// The target as a package-relative path, or `None` when it climbs out of the package.
fn resolve(root: &str, target: &str) -> Option<String> {
    let mut parts = root
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

/// The link from directory `from` to the file `to`, both package-relative.
fn relative(from: &str, to: &str) -> String {
    let from = from
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let to = to.split('/').collect::<Vec<_>>();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let ups = std::iter::repeat_n("..", from.len() - common);
    ups.chain(to[common..].iter().copied())
        .collect::<Vec<_>>()
        .join("/")
}

/// Relative file links in Markdown: inline `](target)` and `[id]: target` definitions, outside
/// code fences.
fn link_targets(body: &str) -> Vec<String> {
    let mut targets = Vec::new();
    let mut fenced = false;
    for line in body.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        let mut rest = line;
        while let Some(index) = rest.find("](") {
            rest = &rest[index + 2..];
            if let Some(end) = rest.find(')') {
                targets.push(rest[..end].to_owned());
                rest = &rest[end..];
            }
        }
        if trimmed.starts_with('[')
            && let Some((_, target)) = trimmed.split_once("]:")
        {
            targets.push(target.trim().to_owned());
        }
    }
    targets
        .into_iter()
        .filter_map(|target| clean(&target))
        .collect()
}

fn clean(raw: &str) -> Option<String> {
    let target = raw.trim().trim_start_matches('<');
    let target = target.split([' ', '\t', '>']).next()?;
    let target = target.split(['#', '?']).next()?;
    let external = target.is_empty()
        || target.starts_with(['/', '$', '~'])
        || target.contains("://")
        || target.starts_with("mailto:");
    (!external).then(|| target.to_owned())
}
