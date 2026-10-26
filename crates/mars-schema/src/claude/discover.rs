//! What a Claude plugin or skill tree contains, and where each item lands in schema v1.

use std::collections::{BTreeMap, BTreeSet};

use super::manifest::{PLUGIN_MANIFEST, PluginSpec, SkillSources};
use crate::diagnostic::{PathSegment, SchemaDiagnostic, Severity};
use crate::discover::{
    Boundary, RECURSIVE_SKIP_DIRS, Discovery, DroppedItem, SelectedDefinition, basename, file_stem, finish, is_hidden,
    join, root_skill_slug, walk,
};
use crate::tree::TreeSource;
use crate::upgrade::DefinitionKind;

type Diagnostics = BTreeMap<String, Vec<SchemaDiagnostic>>;

/// The skills, Agents and commands a plugin's rules select. The root is not a generated tree:
/// `dispatch` checks first.
pub(crate) fn discover_claude(
    source: &impl TreeSource,
    spec: &PluginSpec,
    fallback_name: Option<&str>,
) -> Discovery {
    let mut diagnostics = Diagnostics::new();
    let boundary = Boundary::of(source);
    let mut generated = Vec::new();
    let name = spec.name.as_deref().or(fallback_name);

    let mut skills = BTreeSet::new();
    match &spec.skills {
        SkillSources::Walk => generated.extend(walk_skills(source, &boundary, &mut skills)),
        SkillSources::Declared { default, listed } => {
            if *default {
                default_skills(source, &boundary, &mut generated, &mut skills);
            }
            for raw in listed {
                listed_skills(
                    source,
                    &boundary,
                    raw,
                    &mut generated,
                    &mut skills,
                    &mut diagnostics,
                );
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

    for path in markdown_files(
        source,
        &boundary,
        spec.agents.as_deref(),
        "agents",
        &mut generated,
        &mut diagnostics,
    ) {
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
    // Commands are only reported, never imported.
    let dropped = markdown_files(
        source,
        &boundary,
        spec.commands.as_deref(),
        "commands",
        &mut generated,
        &mut diagnostics,
    )
    .into_iter()
    .map(|source_path| DroppedItem {
        slug: file_stem(&source_path).to_owned(),
        source_path,
    })
    .collect();
    finish(definitions, dropped, boundary, generated, diagnostics)
}

/// Every `SKILL.md` directory within five levels, not searched further once found. A root
/// `SKILL.md` counts only when nothing else is found.
fn walk_skills(
    source: &impl TreeSource,
    boundary: &Boundary,
    skills: &mut BTreeSet<String>,
) -> Vec<String> {
    let generated = walk(source, boundary, |dir, _| {
        if dir.is_empty() {
            return true;
        }
        if source.is_file(&join(dir, "SKILL.md")) {
            skills.insert(dir.to_owned());
            return false;
        }
        true
    });
    if skills.is_empty() && source.is_file("SKILL.md") {
        skills.insert(String::new());
    }
    generated
}

/// `skills/<name>/SKILL.md`, one level deep.
fn default_skills(
    source: &impl TreeSource,
    boundary: &Boundary,
    generated: &mut Vec<String>,
    skills: &mut BTreeSet<String>,
) {
    if boundary.excludes(source, "skills", generated) {
        return;
    }
    for child in source.children("skills") {
        if !is_hidden(&child)
            && !RECURSIVE_SKIP_DIRS.contains(&basename(&child))
            && source.is_dir(&child)
            && boundary.admits(source, &child, generated)
            && source.is_file(&join(&child, "SKILL.md"))
        {
            skills.insert(child);
        }
    }
}

/// A listed path is a folder holding `SKILL.md`, or a folder of `<name>/SKILL.md` folders.
fn listed_skills(
    source: &impl TreeSource,
    boundary: &Boundary,
    raw: &str,
    generated: &mut Vec<String>,
    skills: &mut BTreeSet<String>,
    diagnostics: &mut Diagnostics,
) {
    let Some(path) = declared_path(raw, "skills", diagnostics) else {
        return;
    };
    if !path.is_empty() && boundary.excludes(source, &path, generated) {
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

/// `folder/*.md`, replaced by a manifest list of `.md` files.
fn markdown_files(
    source: &impl TreeSource,
    boundary: &Boundary,
    listed: Option<&[String]>,
    folder: &str,
    generated: &mut Vec<String>,
    diagnostics: &mut Diagnostics,
) -> Vec<String> {
    let mut files = Vec::new();
    match listed {
        None => {
            if !boundary.excludes(source, folder, generated) {
                for child in source.children(folder) {
                    if source.is_file(&child) && child.ends_with(".md") {
                        files.push(child);
                    }
                }
            }
        }
        Some(listed) => {
            for raw in listed {
                let Some(path) = declared_path(raw, folder, diagnostics) else {
                    continue;
                };
                if boundary.excludes(source, &path, generated) {
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
            // Claude loads the rest of the plugin.
            Severity::Warning,
            vec![PathSegment::Key(key.to_owned())],
            format!("declared path `{raw}` does not exist"),
        ));
}
