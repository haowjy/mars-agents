//! Warnings about a translated skill that depend on where it landed: its name, and body links
//! that leave the skill's payload.

use std::collections::BTreeSet;

use crate::diagnostic::{SchemaDiagnostic, Severity, key_path};
use crate::discover::{SelectedDefinition, join};
use crate::frontmatter::Frontmatter;
use crate::item::DefinitionKind;

/// `selected` are the source paths of the definitions being imported.
pub(crate) fn skill_checks(
    definition: &SelectedDefinition,
    frontmatter: &Frontmatter,
    all: &[SelectedDefinition],
    selected: &BTreeSet<&str>,
) -> Vec<SchemaDiagnostic> {
    let mut diagnostics = Vec::new();
    let warn = |code: &str, path, message: String| {
        SchemaDiagnostic::new(code, Severity::Warning, path, message)
    };
    if let Some(name) = frontmatter.name() {
        if !name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            diagnostics.push(warn(
                "skill.name-invalid",
                key_path("name"),
                format!("name `{name}` has characters outside `a-z`, `0-9` and `-`"),
            ));
        } else if name != definition.slug {
            diagnostics.push(warn(
                "skill.name-mismatch",
                key_path("name"),
                format!(
                    "name `{name}` differs from the skill folder `{}`; the folder names it",
                    definition.slug
                ),
            ));
        }
    }
    if frontmatter
        .get("description")
        .and_then(serde_yaml::Value::as_str)
        .is_some_and(|description| description.chars().count() > 1024)
    {
        diagnostics.push(warn(
            "skill.description-too-long",
            key_path("description"),
            "description is over 1024 characters".into(),
        ));
    }
    diagnostics.extend(reference_diagnostics(
        definition,
        frontmatter.body(),
        all,
        selected,
    ));
    diagnostics
}

fn reference_diagnostics(
    definition: &SelectedDefinition,
    body: &str,
    all: &[SelectedDefinition],
    selected: &BTreeSet<&str>,
) -> Vec<SchemaDiagnostic> {
    let Some(root) = definition.skill_root.as_deref() else {
        return Vec::new();
    };
    let mut seen = BTreeSet::new();
    let mut diagnostics = Vec::new();
    for target in link_targets(body) {
        let Some(resolved) = resolve(root, &target) else {
            if seen.insert(target.clone()) {
                diagnostics.push(outside(&target));
            }
            continue;
        };
        if !seen.insert(target.clone()) {
            continue;
        }
        // The linking skill is a candidate too, so a link into its own payload goes through the
        // same stored-path comparison. Payload enumeration carves child skill roots out of the
        // parent, so the longest root decides who owns a file.
        let owner = all
            .iter()
            .filter(|other| other.kind == DefinitionKind::Skill)
            .filter_map(|other| Some((other, other.skill_root.as_deref()?)))
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
        let stored = join(other.output_root(), inside);
        let imported = selected.contains(other.source_path.as_str());
        // Every skill is stored under its own folder, so a link between imported skills keeps
        // working when it lands on the target's stored path from the linking skill's stored root.
        if imported && resolve(definition.output_root(), &target).as_deref() == Some(&stored) {
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
                    relative(definition.output_root(), &stored)
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
