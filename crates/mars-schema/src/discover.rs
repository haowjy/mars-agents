//! Definition selection over a host-independent package tree.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::diagnostic::{SchemaDiagnostic, Severity};
use crate::plugin_metadata::{SENTINEL_FILE, is_valid_sentinel};
use crate::tree::TreeSource;
use crate::upgrade::DefinitionKind;

const RECURSIVE_SKIP_DIRS: &[&str] = &["node_modules", ".git", "dist", "build", "__pycache__"];
const MAX_DISCOVERY_WALK_DEPTH: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelectedDefinition {
    pub kind: DefinitionKind,
    pub slug: String,
    pub source_path: String,
    pub skill_root: Option<String>,
}

#[derive(Debug, Clone)]
struct LayeredDefinition {
    definition: SelectedDefinition,
    layer: usize,
}

pub(crate) struct Discovery {
    pub definitions: Vec<SelectedDefinition>,
    pub skipped_generated_dirs: BTreeSet<String>,
    pub diagnostics: BTreeMap<String, Vec<SchemaDiagnostic>>,
}

pub(crate) fn discover(source: &impl TreeSource) -> Discovery {
    let mut diagnostics = BTreeMap::new();
    let skipped_generated_dirs = generated_plugin_dirs(source, &mut diagnostics);
    if skipped_generated_dirs.contains("") {
        return Discovery {
            definitions: Vec::new(),
            skipped_generated_dirs,
            diagnostics,
        };
    }

    let mut found = Vec::new();
    let mut queue = VecDeque::from([(String::new(), 0usize)]);
    while let Some((dir, depth)) = queue.pop_front() {
        let name = basename(&dir);
        if name == "agents" {
            for path in source.children(&dir) {
                if source.is_file(&path) && path.ends_with(".md") {
                    found.push(LayeredDefinition {
                        definition: SelectedDefinition {
                            kind: DefinitionKind::Agent,
                            slug: file_stem(&path).to_owned(),
                            source_path: path,
                            skill_root: None,
                        },
                        layer: component_count(&dir),
                    });
                }
            }
        } else if name == "skills" {
            for path in source.children(&dir) {
                if source.is_dir(&path)
                    && !is_hidden(&path)
                    && !is_skipped(&path, &skipped_generated_dirs)
                {
                    let definition = join(&path, "SKILL.md");
                    if source.is_file(&definition) {
                        found.push(LayeredDefinition {
                            definition: SelectedDefinition {
                                kind: DefinitionKind::Skill,
                                slug: basename(&path).to_owned(),
                                source_path: definition,
                                skill_root: Some(path),
                            },
                            layer: component_count(&dir),
                        });
                    }
                }
            }
        }

        if depth == MAX_DISCOVERY_WALK_DEPTH {
            continue;
        }
        for path in source.children(&dir) {
            if !source.is_dir(&path)
                || is_hidden(&path)
                || RECURSIVE_SKIP_DIRS.contains(&basename(&path))
                || is_skipped(&path, &skipped_generated_dirs)
            {
                continue;
            }
            queue.push_back((path, depth + 1));
        }
    }

    let had_convention_definition = !found.is_empty();
    if let Some(shallowest) = found.iter().map(|definition| definition.layer).min() {
        found.retain(|definition| definition.layer == shallowest);
    }
    if !had_convention_definition && source.is_file("SKILL.md") {
        found.push(LayeredDefinition {
            definition: SelectedDefinition {
                kind: DefinitionKind::Skill,
                slug: "skill".to_string(),
                source_path: "SKILL.md".to_string(),
                skill_root: Some(String::new()),
            },
            layer: 0,
        });
    }

    let mut definitions = found
        .into_iter()
        .map(|definition| definition.definition)
        .collect::<Vec<_>>();
    definitions.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.slug.cmp(&right.slug))
            .then_with(|| left.source_path.cmp(&right.source_path))
    });
    let mut names = BTreeMap::new();
    for definition in &definitions {
        let key = (definition.kind, definition.slug.clone());
        if let Some(first) = names.insert(key, definition.source_path.clone()) {
            diagnostics
                .entry(definition.source_path.clone())
                .or_default()
                .push(SchemaDiagnostic::new(
                    "definition.slug-collision",
                    Severity::Error,
                    Vec::new(),
                    format!(
                        "{} slug `{}` is selected by both `{first}` and `{}`",
                        match definition.kind {
                            DefinitionKind::Agent => "agent",
                            DefinitionKind::Skill => "skill",
                        },
                        definition.slug,
                        definition.source_path
                    ),
                ));
        }
    }

    Discovery {
        definitions,
        skipped_generated_dirs,
        diagnostics,
    }
}

fn generated_plugin_dirs(
    source: &impl TreeSource,
    diagnostics: &mut BTreeMap<String, Vec<SchemaDiagnostic>>,
) -> BTreeSet<String> {
    let mut generated = BTreeSet::new();
    let mut queue = VecDeque::from([String::new()]);
    while let Some(dir) = queue.pop_front() {
        let marker = join(&dir, SENTINEL_FILE);
        if source.read(&marker).is_some_and(is_valid_sentinel) {
            diagnostics
                .entry(marker)
                .or_default()
                .push(SchemaDiagnostic::new(
                    "generated-plugin-tree-skipped",
                    Severity::Info,
                    Vec::new(),
                    format!(
                        "skipping generated plugin tree at `{}` while reading authored package input",
                        if dir.is_empty() { "." } else { &dir }
                    ),
                ));
            generated.insert(dir);
            continue;
        }
        for child in source.children(&dir) {
            if source.is_dir(&child) {
                queue.push_back(child);
            }
        }
    }
    generated
}

pub(crate) fn payload_paths(
    source: &impl TreeSource,
    definitions: &[SelectedDefinition],
    skipped_generated_dirs: &BTreeSet<String>,
) -> Vec<String> {
    let definition_paths = definitions
        .iter()
        .map(|definition| definition.source_path.as_str())
        .collect::<BTreeSet<_>>();
    let skill_roots = definitions
        .iter()
        .filter_map(|definition| definition.skill_root.as_deref())
        .collect::<BTreeSet<_>>();
    let mut payload = BTreeSet::new();
    for definition in definitions {
        let Some(root) = definition.skill_root.as_deref() else {
            continue;
        };
        let mut queue = VecDeque::from([root.to_owned()]);
        while let Some(dir) = queue.pop_front() {
            for child in source.children(&dir) {
                if is_skipped(&child, skipped_generated_dirs) {
                    continue;
                }
                if source.is_dir(&child) {
                    if child != root && skill_roots.contains(child.as_str()) {
                        continue;
                    }
                    queue.push_back(child);
                } else if !definition_paths.contains(child.as_str()) && child != "mars.toml" {
                    payload.insert(child);
                }
            }
        }
    }
    payload.into_iter().collect()
}

fn join(parent: &str, child: &str) -> String {
    if parent.is_empty() {
        child.to_owned()
    } else {
        format!("{parent}/{child}")
    }
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn file_stem(path: &str) -> &str {
    basename(path).strip_suffix(".md").unwrap_or(basename(path))
}

fn component_count(path: &str) -> usize {
    if path.is_empty() {
        0
    } else {
        path.split('/').count()
    }
}

fn is_hidden(path: &str) -> bool {
    basename(path).starts_with('.')
}

fn is_skipped(path: &str, skipped: &BTreeSet<String>) -> bool {
    skipped.iter().any(|dir| {
        dir.is_empty()
            || path == dir
            || path
                .strip_prefix(dir)
                .is_some_and(|rest| rest.starts_with('/'))
    })
}
