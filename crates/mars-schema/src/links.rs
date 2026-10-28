//! The skill body-link rule: which relative links leave a skill's payload and stop resolving
//! once the skills are installed.
//!
//! A link resolves when, from the linking skill's stored folder, it lands at or under anything the
//! caller says is installed. The caller supplies that test, so one rule serves both callers:
//! `prepare` answers from the package's own selection, and the binary answers from the whole
//! installed tree of a sync (every source, after the consumer's filters and renames). The skills
//! are only used to name the skill the author meant and to build the replacement link.
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
/// `skills` (the same length and order). `installed` says whether a stored path (relative to the
/// installed root) is at or under an installed item.
pub fn skill_link_diagnostics(
    skills: &[SkillView<'_>],
    installed: &dyn Fn(&str) -> bool,
) -> Vec<Vec<SchemaDiagnostic>> {
    skills
        .iter()
        .map(|skill| match skill.body {
            Some(body) => reference_diagnostics(skill, body, skills, installed),
            None => Vec::new(),
        })
        .collect()
}

fn reference_diagnostics(
    definition: &SkillView<'_>,
    body: &str,
    all: &[SkillView<'_>],
    installed: &dyn Fn(&str) -> bool,
) -> Vec<SchemaDiagnostic> {
    let root = definition.source_root;
    let mut seen = BTreeSet::new();
    let mut diagnostics = Vec::new();
    for target in link_targets(body) {
        if !seen.insert(target.clone()) {
            continue;
        }
        // What the agent reads is the installed tree: a link that lands on any installed item
        // works, whichever source put it there.
        if resolve(definition.stored_root, &target).is_some_and(|landing| installed(&landing)) {
            continue;
        }
        // Payload enumeration carves child skill roots out of the parent, so the longest root
        // decides who owns a file.
        let owner = resolve(root, &target).and_then(|resolved| {
            all.iter()
                .filter(|other| {
                    other.source_root.is_empty()
                        || resolved == other.source_root
                        || resolved.starts_with(&format!("{}/", other.source_root))
                })
                .max_by_key(|other| other.source_root.len())
                .map(|other| (other, resolved))
        });
        let Some((other, resolved)) = owner else {
            diagnostics.push(outside(&target));
            continue;
        };
        let inside = resolved
            .strip_prefix(other.source_root)
            .unwrap_or("")
            .trim_start_matches('/');
        let stored = join(other.stored_root, inside);
        let what = if inside.is_empty() {
            "the folder"
        } else {
            "a file"
        };
        diagnostics.push(SchemaDiagnostic::new(
            "skill.reference-to-skill",
            Severity::Warning,
            Vec::new(),
            if other.installed {
                format!(
                    "links to `{target}`, {what} of skill `{}`, which does not resolve after install; link `{}` instead",
                    other.slug,
                    relative(definition.stored_root, &stored)
                )
            } else {
                format!(
                    "links to `{target}`, {what} of skill `{}`, which is not imported; stored it would be `{stored}`",
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
