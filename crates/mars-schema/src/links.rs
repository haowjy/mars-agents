//! The skill body-link rule: which relative links leave a skill's payload and stop resolving
//! once the skills are installed.
//!
//! A link resolves when, from the linking skill's installed folder, it lands at or under anything
//! installed, whichever source put it there. The caller says where each definition is installed
//! and what is installed in all; the definitions are also used to name the skill the author meant
//! and to build the replacement link. [`package`] answers for a package's own layout and
//! [`check`] is the one rule behind every caller: `mars check`, `doctor` and `migrate` through
//! `package_prepare`, the WASM `prepare` and `normalize` exports, and the binary's `sync` and
//! plugin build, which answer from the whole installed tree after the consumer's filters and
//! renames.
//!
//! Links are read from the canonical `SKILL.md` body; a per-target `variants/<harness>/SKILL.md`
//! body is not checked.

use std::collections::BTreeSet;

use crate::diagnostic::{SchemaDiagnostic, Severity};
use crate::discover::join;
use crate::prepare::{Prepared, PreparedDefinition};
use crate::read::{DefinitionRead, Reading};

/// Add the link verdict of `prepared`'s own layout to its diagnostics, as if every definition
/// were installed where it is output.
pub fn package(prepared: &mut Prepared) {
    let dests = prepared
        .definitions
        .iter()
        .map(|definition| Some(output_root(definition)))
        .collect::<Vec<_>>();
    let roots = dests.iter().flatten().copied().collect::<Vec<_>>();
    let found = check(&prepared.definitions, &dests, &roots)
        .into_iter()
        .zip(&prepared.definitions)
        .filter(|(found, _)| !found.is_empty())
        .map(|(found, definition)| (definition.source_path.clone(), found))
        .collect::<Vec<_>>();
    for (source_path, found) in found {
        prepared
            .diagnostics
            .entry(source_path)
            .or_default()
            .extend(found);
    }
}

/// The folder a skill is output to, or the file an agent is.
fn output_root(definition: &PreparedDefinition) -> &str {
    definition
        .output_path
        .strip_suffix("SKILL.md")
        .map_or(&definition.output_path, |root| root.trim_end_matches('/'))
}

/// Body links that leave a skill's payload and will not resolve after install, by index into
/// `definitions`. `dests[i]` is where definition `i` is installed (`None` when a filter or a
/// local item dropped it), and `installed` lists everything that is installed.
///
/// Every skill is checked, whatever its dialect and wherever it was authored; a skill that is
/// not installed has no links checked, though it can still be a link target.
pub fn check(
    definitions: &[PreparedDefinition],
    dests: &[Option<&str>],
    installed: &[&str],
) -> Vec<Vec<SchemaDiagnostic>> {
    definitions
        .iter()
        .zip(dests)
        .map(|(definition, dest)| match (&definition.reading, dest) {
            (
                Reading::Skill(DefinitionRead {
                    body: Some(body), ..
                }),
                Some(dest),
            ) if definition.skill_root.is_some() => {
                reference_diagnostics(definition, dest, body, definitions, dests, installed)
            }
            _ => Vec::new(),
        })
        .collect()
}

fn at_or_under(path: &str, root: &str) -> bool {
    path == root
        || path
            .strip_prefix(root)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn reference_diagnostics(
    definition: &PreparedDefinition,
    dest: &str,
    body: &str,
    all: &[PreparedDefinition],
    dests: &[Option<&str>],
    installed: &[&str],
) -> Vec<SchemaDiagnostic> {
    let root = definition.skill_root.as_deref().unwrap_or_default();
    let mut seen = BTreeSet::new();
    let mut diagnostics = Vec::new();
    for target in link_targets(body) {
        if !seen.insert(target.clone()) {
            continue;
        }
        if resolve(dest, &target)
            .is_some_and(|landing| installed.iter().any(|root| at_or_under(&landing, root)))
        {
            continue;
        }
        // Payload enumeration carves child skill roots out of the parent, so the longest root
        // decides who owns a file.
        let owner = resolve(root, &target).and_then(|resolved| {
            all.iter()
                .zip(dests)
                .filter_map(|(other, other_dest)| {
                    Some((other, other_dest, other.skill_root.as_deref()?))
                })
                .filter(|(_, _, other_root)| {
                    other_root.is_empty() || at_or_under(&resolved, other_root)
                })
                .max_by_key(|(_, _, other_root)| other_root.len())
                .map(|(other, other_dest, other_root)| (other, *other_dest, other_root, resolved))
        });
        let Some((other, other_dest, other_root, resolved)) = owner else {
            diagnostics.push(outside(&target));
            continue;
        };
        let inside = resolved
            .strip_prefix(other_root)
            .unwrap_or("")
            .trim_start_matches('/');
        let what = if inside.is_empty() {
            "the folder"
        } else {
            "a file"
        };
        let advice = match other_dest {
            Some(other_dest) => format!(
                "which does not resolve after install; link `{}` instead",
                relative(dest, &join(other_dest, inside))
            ),
            None => "which is not installed; import it or drop the link".to_owned(),
        };
        diagnostics.push(SchemaDiagnostic::new(
            "skill.reference-to-skill",
            Severity::Warning,
            Vec::new(),
            format!(
                "links to `{target}`, {what} of skill `{}`, {advice}",
                other.slug
            ),
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
