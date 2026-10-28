//! The skill body-link rule: which relative links leave a skill's payload and stop resolving
//! once the skills are installed.
//!
//! A link resolves when, from the linking skill's installed folder, it lands at or under anything
//! installed, whichever source put it there. The caller says where each definition is installed
//! and what is installed in all; the definitions are also used to name the skill the author meant
//! and to build the replacement link. [`check`] is the one rule behind every caller, and the
//! caller picks the view: [`package`] judges a package as if only its own layout were installed
//! (`mars check` and `mars migrate`, the WASM `prepare` and `normalize` exports), and the
//! binary's `sync` and plugin build judge every installed skill against the whole installed tree
//! after the consumer's filters and renames.
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
    let skills = prepared
        .definitions
        .iter()
        .map(|definition| (definition, Some(output_root(definition))))
        .collect::<Vec<_>>();
    let roots = skills
        .iter()
        .filter_map(|(_, dest)| *dest)
        .collect::<Vec<_>>();
    let found = check(&skills, &roots)
        .into_iter()
        .map(|(definition, diagnostic)| (definition.source_path.clone(), diagnostic))
        .collect::<Vec<_>>();
    for (source_path, diagnostic) in found {
        prepared
            .diagnostics
            .entry(source_path)
            .or_default()
            .push(diagnostic);
    }
}

/// The folder a skill is output to, or the file an agent is.
fn output_root(definition: &PreparedDefinition) -> &str {
    definition
        .output_path
        .strip_suffix("SKILL.md")
        .map_or(&definition.output_path, |root| root.trim_end_matches('/'))
}

/// A definition of one package and where it is installed (`None` when a filter dropped it).
pub type Installed<'a> = (&'a PreparedDefinition, Option<&'a str>);

/// Body links that leave a skill's payload and will not resolve after install, each with the
/// definition it is in. `skills` is every definition of one package, because a link's target is
/// named by the package's own layout; `installed` lists everything that is installed, from all
/// packages.
///
/// Every skill is checked, whatever its dialect and wherever it was authored; a skill that is
/// not installed has no links checked, though it can still be a link target.
pub fn check<'a>(
    skills: &[Installed<'a>],
    installed: &[&str],
) -> Vec<(&'a PreparedDefinition, SchemaDiagnostic)> {
    let mut found = Vec::new();
    for &(definition, dest) in skills {
        if let (
            Reading::Skill(DefinitionRead {
                body: Some(body), ..
            }),
            Some(dest),
            Some(root),
        ) = (&definition.reading, dest, definition.skill_root.as_deref())
        {
            found.extend(
                reference_diagnostics(root, dest, body, skills, installed)
                    .into_iter()
                    .map(|diagnostic| (definition, diagnostic)),
            );
        }
    }
    found
}

fn at_or_under(path: &str, root: &str) -> bool {
    path == root
        || path
            .strip_prefix(root)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn reference_diagnostics(
    root: &str,
    dest: &str,
    body: &str,
    all: &[Installed<'_>],
    installed: &[&str],
) -> Vec<SchemaDiagnostic> {
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
                .filter_map(|&(other, other_dest)| {
                    Some((other, other_dest, other.skill_root.as_deref()?))
                })
                .filter(|(_, _, other_root)| {
                    other_root.is_empty() || at_or_under(&resolved, other_root)
                })
                .max_by_key(|(_, _, other_root)| other_root.len())
                .map(|(other, other_dest, other_root)| (other, other_dest, other_root, resolved))
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
/// code fences, indented code blocks and inline code spans.
fn link_targets(body: &str) -> Vec<String> {
    let mut targets = Vec::new();
    // The opening fence's character and length: only a fence of the same kind, at least as
    // long, closes it.
    let mut fence: Option<(char, usize)> = None;
    let mut previous_blank = true;
    let mut previous_code = false;
    let mut in_list = false;
    for line in body.lines() {
        let trimmed = line.trim_start();
        let blank = trimmed.is_empty();
        let marker = trimmed.chars().next().filter(|c| matches!(c, '`' | '~'));
        let run = marker.map_or(0, |m| trimmed.chars().take_while(|&c| c == m).count());
        if let Some((open, length)) = fence {
            if marker == Some(open) && run >= length && trimmed[run..].trim().is_empty() {
                fence = None;
            }
            continue;
        }
        if let Some(marker) = marker
            && run >= 3
        {
            fence = Some((marker, run));
            previous_blank = false;
            previous_code = false;
            continue;
        }
        // An indented line after a blank line is code unless it continues a list item.
        let indented = line.starts_with("    ") || line.starts_with('\t');
        let code = indented && !in_list && (previous_blank || previous_code);
        previous_blank = blank;
        previous_code = code;
        if code {
            continue;
        }
        if !blank && !indented {
            in_list = is_list_item(trimmed);
        }
        let line = without_code_spans(line);
        let mut rest = line.as_str();
        while let Some(index) = rest.find("](") {
            rest = &rest[index + 2..];
            if let Some(end) = rest.find(')') {
                targets.push(rest[..end].to_owned());
                rest = &rest[end..];
            }
        }
        let trimmed = line.trim_start();
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

fn is_list_item(trimmed: &str) -> bool {
    let after_digits = trimmed.trim_start_matches(|c: char| c.is_ascii_digit());
    let after_marker = if after_digits.len() < trimmed.len() {
        after_digits.strip_prefix(['.', ')'])
    } else {
        trimmed.strip_prefix(['-', '*', '+'])
    };
    after_marker.is_some_and(|rest| rest.starts_with(' ') || rest.is_empty())
}

/// `line` with each inline code span replaced by a space. A backtick run closes at the next run
/// of the same length; one that never closes is literal text.
fn without_code_spans(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(start) = rest.find('`') {
        let length = rest[start..].bytes().take_while(|&b| b == b'`').count();
        let after = &rest[start + length..];
        out.push_str(&rest[..start]);
        match closing_run(after, length) {
            Some(end) => {
                out.push(' ');
                rest = &after[end + length..];
            }
            None => {
                out.push_str(&rest[start..start + length]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Where the next backtick run of exactly `length` starts in `text`.
fn closing_run(text: &str, length: usize) -> Option<usize> {
    let mut from = 0;
    while let Some(offset) = text[from..].find('`') {
        let start = from + offset;
        let run = text[start..].bytes().take_while(|&b| b == b'`').count();
        if run == length {
            return Some(start);
        }
        from = start + run;
    }
    None
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
