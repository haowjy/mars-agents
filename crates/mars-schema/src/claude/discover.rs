//! What a Claude plugin or skill tree contains, and where each item lands in schema v1.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use super::manifest::{PLUGIN_MANIFEST, PluginSpec, SkillSources};
use crate::diagnostic::{PathSegment, SchemaDiagnostic, Severity};
use crate::discover::{
    Discovery, DroppedItem, RECURSIVE_SKIP_DIRS, SelectedDefinition, basename, file_stem, finish,
    generated_plugin_dirs, is_hidden, is_skipped, join, root_skill_slug,
};
use crate::tree::TreeSource;
use crate::upgrade::DefinitionKind;

/// A skill directory is searched for within this many levels of the root.
const MAX_SKILL_DEPTH: usize = 5;

type Diagnostics = BTreeMap<String, Vec<SchemaDiagnostic>>;

pub(crate) fn discover_claude(
    source: &impl TreeSource,
    spec: &PluginSpec,
    fallback_name: Option<&str>,
) -> Discovery {
    let mut diagnostics = Diagnostics::new();
    let skipped = generated_plugin_dirs(source, &mut diagnostics);
    if skipped.contains("") {
        return Discovery {
            skipped_generated_dirs: skipped,
            diagnostics,
            ..Discovery::empty()
        };
    }
    let name = spec.name.as_deref().or(fallback_name);

    let mut skills = BTreeSet::new();
    match &spec.skills {
        SkillSources::Walk => walk_skills(source, &skipped, &mut skills),
        SkillSources::Declared { default, listed } => {
            if *default {
                default_skills(source, &skipped, &mut skills);
            }
            for raw in listed {
                listed_skills(source, raw, &skipped, &mut skills, &mut diagnostics);
            }
        }
    }
    let mut definitions = skills
        .into_iter()
        .map(|root| {
            let slug = if root.is_empty() {
                root_skill_slug(source, name)
            } else {
                basename(&root).to_owned()
            };
            SelectedDefinition::new(
                DefinitionKind::Skill,
                slug,
                join(&root, "SKILL.md"),
                Some(root),
            )
            .relocated()
        })
        .collect::<Vec<_>>();

    for path in agent_files(source, spec, &skipped, &mut diagnostics) {
        definitions.push(
            SelectedDefinition::new(
                DefinitionKind::Agent,
                file_stem(&path).to_owned(),
                path,
                None,
            )
            .relocated(),
        );
    }
    let dropped = command_files(source, spec, &skipped, &mut diagnostics);
    finish(definitions, dropped, skipped, diagnostics)
}

fn is_dir_visible(source: &impl TreeSource, path: &str, skipped: &BTreeSet<String>) -> bool {
    source.is_dir(path)
        && !is_hidden(path)
        && !RECURSIVE_SKIP_DIRS.contains(&basename(path))
        && !is_skipped(path, skipped)
}

/// Every `SKILL.md` directory within five levels, not searched further once found. A root
/// `SKILL.md` counts only when nothing else is found.
fn walk_skills(
    source: &impl TreeSource,
    skipped: &BTreeSet<String>,
    skills: &mut BTreeSet<String>,
) {
    let mut queue = VecDeque::from([(String::new(), 0usize)]);
    while let Some((dir, depth)) = queue.pop_front() {
        for child in source.children(&dir) {
            if !is_dir_visible(source, &child, skipped) {
                continue;
            }
            if source.is_file(&join(&child, "SKILL.md")) {
                skills.insert(child);
            } else if depth + 1 < MAX_SKILL_DEPTH {
                queue.push_back((child, depth + 1));
            }
        }
    }
    if skills.is_empty() && source.is_file("SKILL.md") {
        skills.insert(String::new());
    }
}

/// `skills/<name>/SKILL.md`, one level deep.
fn default_skills(
    source: &impl TreeSource,
    skipped: &BTreeSet<String>,
    skills: &mut BTreeSet<String>,
) {
    for child in source.children("skills") {
        if is_dir_visible(source, &child, skipped) && source.is_file(&join(&child, "SKILL.md")) {
            skills.insert(child);
        }
    }
}

/// A listed path is a folder holding `SKILL.md`, or a folder of `<name>/SKILL.md` folders.
fn listed_skills(
    source: &impl TreeSource,
    raw: &str,
    skipped: &BTreeSet<String>,
    skills: &mut BTreeSet<String>,
    diagnostics: &mut Diagnostics,
) {
    let Some(path) = declared_path(raw, "skills", diagnostics) else {
        return;
    };
    if is_skipped(&path, skipped) && !path.is_empty() {
        return;
    }
    // A listed file would make the probes below look inside it.
    if !source.is_dir(&path) {
        missing_path(diagnostics, "skills", raw);
    } else if source.is_file(&join(&path, "SKILL.md")) {
        skills.insert(path);
    } else {
        for child in source.children(&path) {
            if source.is_dir(&child) && source.is_file(&join(&child, "SKILL.md")) {
                skills.insert(child);
            }
        }
    }
}

/// Default `agents/*.md`, replaced by a manifest list of `.md` files.
fn agent_files(
    source: &impl TreeSource,
    spec: &PluginSpec,
    skipped: &BTreeSet<String>,
    diagnostics: &mut Diagnostics,
) -> Vec<String> {
    markdown_files(
        source,
        spec.agents.as_deref(),
        "agents",
        skipped,
        diagnostics,
    )
}

/// Commands are only reported, never imported.
fn command_files(
    source: &impl TreeSource,
    spec: &PluginSpec,
    skipped: &BTreeSet<String>,
    diagnostics: &mut Diagnostics,
) -> Vec<DroppedItem> {
    markdown_files(
        source,
        spec.commands.as_deref(),
        "commands",
        skipped,
        diagnostics,
    )
    .into_iter()
    .map(|source_path| DroppedItem {
        slug: file_stem(&source_path).to_owned(),
        source_path,
    })
    .collect()
}

fn markdown_files(
    source: &impl TreeSource,
    listed: Option<&[String]>,
    folder: &str,
    skipped: &BTreeSet<String>,
    diagnostics: &mut Diagnostics,
) -> Vec<String> {
    let mut files = Vec::new();
    match listed {
        None => {
            for child in source.children(folder) {
                if source.is_file(&child) && child.ends_with(".md") && !is_skipped(&child, skipped)
                {
                    files.push(child);
                }
            }
        }
        Some(listed) => {
            for raw in listed {
                let Some(path) = declared_path(raw, folder, diagnostics) else {
                    continue;
                };
                if is_skipped(&path, skipped) {
                    continue;
                }
                if source.is_file(&path) && path.ends_with(".md") {
                    files.push(path);
                } else if source.is_dir(&path) {
                    diagnostics
                        .entry(PLUGIN_MANIFEST.to_owned())
                        .or_default()
                        .push(SchemaDiagnostic::new(
                            "source-manifest.path-not-file",
                            Severity::Warning,
                            vec![PathSegment::Key(folder.to_owned())],
                            format!(
                                "`{raw}` is a directory; `{folder}` entries must be `.md` files"
                            ),
                        ));
                } else {
                    missing_path(diagnostics, folder, raw);
                }
            }
        }
    }
    files
}

/// A `./`-relative manifest path as a package-relative one; `.` is the root (`""`).
fn declared_path(raw: &str, key: &str, diagnostics: &mut Diagnostics) -> Option<String> {
    let relative = raw.strip_prefix("./").unwrap_or(raw).trim_end_matches('/');
    let relative = if relative == "." { "" } else { relative };
    if !relative.is_empty()
        && relative
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
    {
        diagnostics
            .entry(PLUGIN_MANIFEST.to_owned())
            .or_default()
            .push(SchemaDiagnostic::new(
                "source-manifest.path-escape",
                Severity::Error,
                vec![PathSegment::Key(key.to_owned())],
                format!("declared path `{raw}` escapes the package root"),
            ));
        return None;
    }
    Some(relative.to_owned())
}

fn missing_path(diagnostics: &mut Diagnostics, key: &str, raw: &str) {
    diagnostics
        .entry(PLUGIN_MANIFEST.to_owned())
        .or_default()
        .push(SchemaDiagnostic::new(
            "source-manifest.path-missing",
            Severity::Error,
            vec![PathSegment::Key(key.to_owned())],
            format!("declared path `{raw}` does not exist"),
        ));
}
