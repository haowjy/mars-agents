//! Definition selection over a host-independent package tree.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::diagnostic::{SchemaDiagnostic, Severity};
use crate::plugin_metadata::{SENTINEL_FILE, is_valid_sentinel};
use crate::tree::TreeSource;
use crate::upgrade::DefinitionKind;

pub(crate) const RECURSIVE_SKIP_DIRS: &[&str] =
    &["node_modules", ".git", "dist", "build", "__pycache__"];
const MAX_DISCOVERY_WALK_DEPTH: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelectedDefinition {
    pub kind: DefinitionKind,
    pub slug: String,
    pub source_path: String,
    /// Where the file lands in the schema v1 layout; equal to `source_path` unless the
    /// dialect relocates.
    pub output_path: String,
    pub skill_root: Option<String>,
}

impl SelectedDefinition {
    pub(crate) fn new(
        kind: DefinitionKind,
        slug: String,
        source_path: String,
        skill_root: Option<String>,
    ) -> Self {
        Self {
            kind,
            slug,
            output_path: source_path.clone(),
            source_path,
            skill_root,
        }
    }

    /// Relocate to `skills/<slug>/` or `agents/<slug>.md`.
    pub(crate) fn relocated(mut self) -> Self {
        self.output_path = match self.kind {
            DefinitionKind::Agent => format!("agents/{}.md", self.slug),
            DefinitionKind::Skill => format!("skills/{}/SKILL.md", self.slug),
        };
        self
    }

    /// The directory a skill's payload lands in; empty for a skill kept at the package root.
    pub(crate) fn output_root(&self) -> &str {
        self.output_path
            .strip_suffix("SKILL.md")
            .map_or("", |root| root.trim_end_matches('/'))
    }
}

/// A file the dialect recognizes and does not import (Claude commands).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DroppedItem {
    pub slug: String,
    pub source_path: String,
}

#[derive(Debug, Clone)]
struct LayeredDefinition {
    definition: SelectedDefinition,
    layer: usize,
}

pub(crate) struct Discovery {
    pub definitions: Vec<SelectedDefinition>,
    pub dropped: Vec<DroppedItem>,
    pub skipped_generated_dirs: BTreeSet<String>,
    pub diagnostics: BTreeMap<String, Vec<SchemaDiagnostic>>,
}

impl Discovery {
    pub(crate) fn empty() -> Self {
        Self {
            definitions: Vec::new(),
            dropped: Vec::new(),
            skipped_generated_dirs: BTreeSet::new(),
            diagnostics: BTreeMap::new(),
        }
    }
}

/// Mars's convention walk: `agents/` and `skills/` containers at the shallowest layer.
/// Output paths equal source paths.
pub(crate) fn discover(source: &impl TreeSource, fallback_name: Option<&str>) -> Discovery {
    let mut diagnostics = BTreeMap::new();
    let skipped_generated_dirs = generated_plugin_dirs(source, &mut diagnostics);
    if skipped_generated_dirs.contains("") {
        return Discovery {
            skipped_generated_dirs,
            diagnostics,
            ..Discovery::empty()
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
                        definition: SelectedDefinition::new(
                            DefinitionKind::Agent,
                            file_stem(&path).to_owned(),
                            path,
                            None,
                        ),
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
                            definition: SelectedDefinition::new(
                                DefinitionKind::Skill,
                                basename(&path).to_owned(),
                                definition,
                                Some(path),
                            ),
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
            definition: SelectedDefinition::new(
                DefinitionKind::Skill,
                root_skill_slug(source, fallback_name),
                "SKILL.md".to_string(),
                Some(String::new()),
            ),
            layer: 0,
        });
    }

    finish(
        found
            .into_iter()
            .map(|definition| definition.definition)
            .collect(),
        Vec::new(),
        skipped_generated_dirs,
        diagnostics,
    )
}

/// Sort, drop duplicate files, and refuse two definitions of one kind sharing a slug.
pub(crate) fn finish(
    mut definitions: Vec<SelectedDefinition>,
    dropped: Vec<DroppedItem>,
    skipped_generated_dirs: BTreeSet<String>,
    mut diagnostics: BTreeMap<String, Vec<SchemaDiagnostic>>,
) -> Discovery {
    definitions.sort_by(|left, right| left.source_path.cmp(&right.source_path));
    definitions.dedup_by(|left, right| left.source_path == right.source_path);
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
                    "import.duplicate-slug",
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
        dropped,
        skipped_generated_dirs,
        diagnostics,
    }
}

/// A skill kept at the package root is named by its frontmatter `name` when that is a valid
/// slug, else by the package, else `skill`.
pub(crate) fn root_skill_slug(source: &impl TreeSource, fallback_name: Option<&str>) -> String {
    source
        .read("SKILL.md")
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .and_then(|text| crate::frontmatter::Frontmatter::parse(&text).ok())
        .and_then(|frontmatter| frontmatter.name().map(str::to_owned))
        .filter(|name| is_slug(name))
        .or_else(|| fallback_name.map(slugify).filter(|name| is_slug(name)))
        .unwrap_or_else(|| "skill".to_owned())
}

fn is_slug(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// A package name as a slug: `Foo Bar/baz` → `foo-bar-baz`.
fn slugify(name: &str) -> String {
    let mut slug = String::new();
    for character in name.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            slug.push(character);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.trim_matches('-').to_owned()
}

pub(crate) fn generated_plugin_dirs(
    source: &impl TreeSource,
    diagnostics: &mut BTreeMap<String, Vec<SchemaDiagnostic>>,
) -> BTreeSet<String> {
    let mut generated = BTreeSet::new();
    let mut queue = VecDeque::from([String::new()]);
    while let Some(dir) = queue.pop_front() {
        let marker = join(&dir, SENTINEL_FILE);
        if source
            .read(&marker)
            .is_some_and(|bytes| is_valid_sentinel(&bytes))
        {
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
            // Hidden surfaces are not searched for generated plugin trees.
            // They remain visible to flat-skill payload enumeration, where a
            // dot-prefixed directory can be intentional authored content.
            if !is_hidden(&child) && source.is_dir(&child) {
                queue.push_back(child);
            }
        }
    }
    generated
}

/// The files a skill carries, source path to output path. `all` is every discovered
/// definition, so a nested skill is never its parent's payload whether or not it is selected;
/// `selected` are the skills whose payload is wanted.
pub(crate) fn payload_paths<'a>(
    source: &impl TreeSource,
    all: &[SelectedDefinition],
    selected: impl IntoIterator<Item = &'a SelectedDefinition>,
    skipped_generated_dirs: &BTreeSet<String>,
    diagnostics: &mut BTreeMap<String, Vec<SchemaDiagnostic>>,
) -> Vec<(String, String)> {
    let definition_paths = all
        .iter()
        .map(|definition| definition.source_path.as_str())
        .collect::<BTreeSet<_>>();
    let skill_roots = all
        .iter()
        .filter_map(|definition| definition.skill_root.as_deref())
        .collect::<BTreeSet<_>>();
    let mut payload = BTreeMap::new();
    for definition in selected {
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
                    if (child != root && skill_roots.contains(child.as_str()))
                        || matches!(basename(&child), ".git" | "node_modules")
                    {
                        continue;
                    }
                    queue.push_back(child);
                } else if !definition_paths.contains(child.as_str()) && child != "mars.toml" {
                    if basename(&child) == "SKILL.md" {
                        diagnostics
                            .entry(child.clone())
                            .or_default()
                            .push(SchemaDiagnostic::new(
                                "skill.nested-skill-md",
                                Severity::Warning,
                                Vec::new(),
                                format!(
                                    "`{child}` is inside skill `{}` and is kept as its payload, not imported as a skill",
                                    definition.slug
                                ),
                            ));
                    }
                    let relative = if root.is_empty() {
                        child.as_str()
                    } else {
                        &child[root.len() + 1..]
                    };
                    let output_root = definition.output_root();
                    payload.insert(child.clone(), join(output_root, relative));
                }
            }
        }
    }
    payload.into_iter().collect()
}

pub(crate) fn join(parent: &str, child: &str) -> String {
    if parent.is_empty() {
        child.to_owned()
    } else {
        format!("{parent}/{child}")
    }
}

pub(crate) fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

pub(crate) fn file_stem(path: &str) -> &str {
    basename(path).strip_suffix(".md").unwrap_or(basename(path))
}

fn component_count(path: &str) -> usize {
    if path.is_empty() {
        0
    } else {
        path.split('/').count()
    }
}

pub(crate) fn is_hidden(path: &str) -> bool {
    basename(path).starts_with('.')
}

pub(crate) fn is_skipped(path: &str, skipped: &BTreeSet<String>) -> bool {
    skipped.iter().any(|dir| {
        dir.is_empty()
            || path == dir
            || path
                .strip_prefix(dir)
                .is_some_and(|rest| rest.starts_with('/'))
    })
}
