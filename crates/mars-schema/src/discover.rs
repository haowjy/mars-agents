//! Definition selection over a host-independent package tree, and the one bounded directory
//! walk every convention scan shares (the binary's bootstrap walk included).

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::diagnostic::{SchemaDiagnostic, Severity};
use crate::item::DefinitionKind;
use crate::plugin_metadata::{SENTINEL_FILE, is_valid_sentinel};
use crate::tree::TreeSource;

/// Never walked: high-volume output that often holds docs shaped like agents or skills.
pub(crate) const RECURSIVE_SKIP_DIRS: &[&str] =
    &["node_modules", ".git", "dist", "build", "__pycache__"];
/// Real layouts like `vendor/pkg/.claude/skills/foo` and `packages/group/tooling/agents/foo.md`
/// fit; deeper convention directories are skipped silently so a repository tree is not an
/// unbounded discovery surface.
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
    /// What this root's walks never enter; payload enumeration applies it too.
    pub boundary: Boundary,
    pub diagnostics: BTreeMap<String, Vec<SchemaDiagnostic>>,
}

impl Discovery {
    pub(crate) fn empty() -> Self {
        Self {
            definitions: Vec::new(),
            dropped: Vec::new(),
            boundary: Boundary::default(),
            diagnostics: BTreeMap::new(),
        }
    }

    /// A root that is itself a generated plugin tree holds nothing to read.
    pub(crate) fn generated_root() -> Self {
        let mut discovery = Self::empty();
        report_generated(&mut discovery.diagnostics, [""]);
        discovery
    }
}

/// The directories no walk enters: hidden ones, the skip list, `[plugin].dir`, and any
/// directory holding a valid generated-plugin marker. A walk checks each directory it is about
/// to enter, so a marker is found only inside the bounded walk and never by a scan of the tree.
#[derive(Debug, Clone, Default)]
pub(crate) struct Boundary {
    /// `[plugin].dir`, excluded by path even before any marker exists, so the package's own
    /// committed output never adds noise.
    plugin_dir: Option<String>,
}

impl Boundary {
    /// Reads `[plugin].dir` (`plugin` when the table omits it) from the root's `mars.toml`. A
    /// value that is not a package-relative path is ignored: config validation reports it.
    pub(crate) fn of(source: &impl TreeSource) -> Self {
        let plugin_dir = source
            .read("mars.toml")
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .and_then(|text| toml::from_str::<toml::Value>(&text).ok())
            .and_then(|manifest| {
                let plugin = manifest.get("plugin")?.as_table()?;
                match plugin.get("dir") {
                    None => Some("plugin".to_owned()),
                    Some(dir) => dir.as_str().map(str::to_owned),
                }
            })
            .map(|dir| dir.trim_end_matches('/').to_owned())
            .filter(|dir| {
                !dir.is_empty()
                    && !dir.starts_with('/')
                    && dir
                        .split('/')
                        .all(|part| !part.is_empty() && !matches!(part, "." | ".."))
            });
        Self { plugin_dir }
    }

    fn in_plugin_dir(&self, path: &str) -> bool {
        self.plugin_dir
            .as_deref()
            .is_some_and(|dir| within(path, dir))
    }

    /// Whether `dir`, whose parents are already clear, may be entered. A directory holding a
    /// marker is recorded in `generated`. Hidden directories are not searched for markers: a
    /// dot-prefixed directory can be intentional authored content in a skill's payload.
    pub(crate) fn admits(
        &self,
        source: &impl TreeSource,
        dir: &str,
        generated: &mut Vec<String>,
    ) -> bool {
        if self.in_plugin_dir(dir) {
            return false;
        }
        if !is_hidden(dir) && has_marker(source, dir) {
            generated.push(dir.to_owned());
            return false;
        }
        true
    }

    /// Why `path` is not input, if it is not: for a path a manifest names, whose parents
    /// nothing has checked.
    fn exclusion(&self, source: &impl TreeSource, path: &str) -> Option<Excluded> {
        if self.in_plugin_dir(path) {
            return Some(Excluded::PluginDir);
        }
        enclosing_generated_dir(source, path).map(Excluded::Generated)
    }

    /// [`Boundary::exclusion`], recording a generated tree.
    pub(crate) fn excludes(
        &self,
        source: &impl TreeSource,
        path: &str,
        generated: &mut Vec<String>,
    ) -> bool {
        match self.exclusion(source, path) {
            Some(Excluded::Generated(dir)) => {
                generated.push(dir);
                true
            }
            Some(Excluded::PluginDir) => true,
            None => false,
        }
    }
}

/// Why a path is not package input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Excluded {
    /// It lies in the manifest's `[plugin].dir`, where Mars writes its own output.
    PluginDir,
    /// It lies in this generated plugin tree (the directory holding the marker).
    Generated(String),
}

/// Why a package-relative path a manifest names is not input, if it is not.
pub fn excluded(source: &impl TreeSource, path: &str) -> Option<Excluded> {
    Boundary::of(source).exclusion(source, path)
}

fn within(path: &str, dir: &str) -> bool {
    path == dir
        || path
            .strip_prefix(dir)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn has_marker(source: &impl TreeSource, dir: &str) -> bool {
    source
        .read(&join(dir, SENTINEL_FILE))
        .is_some_and(|bytes| is_valid_sentinel(&bytes))
}

/// An info diagnostic per generated tree, once each, at its marker.
pub(crate) fn report_generated<'a>(
    diagnostics: &mut BTreeMap<String, Vec<SchemaDiagnostic>>,
    dirs: impl IntoIterator<Item = &'a str>,
) {
    for dir in dirs {
        let found = diagnostics.entry(join(dir, SENTINEL_FILE)).or_default();
        if found
            .iter()
            .any(|diagnostic| diagnostic.code == "generated-plugin-tree-skipped")
        {
            continue;
        }
        found.push(SchemaDiagnostic::new(
            "generated-plugin-tree-skipped",
            Severity::Info,
            Vec::new(),
            format!(
                "skipping generated plugin tree at `{}` while reading authored package input",
                if dir.is_empty() { "." } else { dir }
            ),
        ));
    }
}

/// Breadth-first over the directories convention scans look in: the root, then each directory
/// the boundary admits that is not hidden or on the skip list, up to five levels down.
/// `visit` sees each directory and says whether to descend; it may record generated trees it
/// meets in the list it is given. Returns every generated tree the walk skipped.
pub(crate) fn walk(
    source: &impl TreeSource,
    boundary: &Boundary,
    mut visit: impl FnMut(&str, &mut Vec<String>) -> bool,
) -> Vec<String> {
    let mut generated = Vec::new();
    let mut queue = VecDeque::from([(String::new(), 0usize)]);
    while let Some((dir, depth)) = queue.pop_front() {
        if !visit(&dir, &mut generated) || depth == MAX_DISCOVERY_WALK_DEPTH {
            continue;
        }
        for child in source.children(&dir) {
            if !is_hidden(&child)
                && !RECURSIVE_SKIP_DIRS.contains(&basename(&child))
                && source.is_dir(&child)
                && boundary.admits(source, &child, &mut generated)
            {
                queue.push_back((child, depth + 1));
            }
        }
    }
    generated
}

/// The directories of a root a convention scan visits, with the generated plugin trees the
/// walk skipped (the caller reports them). This is the walk `mars-schema` selects Agents and
/// skills with; a caller scanning for something else (the binary's bootstrap documents)
/// reuses it rather than keeping its own skip list, depth, or marker rule.
pub fn walk_dirs(source: &impl TreeSource, mut visit: impl FnMut(&str)) -> Vec<String> {
    walk(source, &Boundary::of(source), |dir, _| {
        visit(dir);
        true
    })
}

/// Mars's convention walk: `agents/` and `skills/` containers at the shallowest layer.
/// Output paths equal source paths. The root is not a generated tree: `dispatch` checks first.
pub(crate) fn discover(source: &impl TreeSource, fallback_name: Option<&str>) -> Discovery {
    let boundary = Boundary::of(source);
    let mut found = Vec::new();
    let generated = walk(source, &boundary, |dir, generated| {
        let layer = component_count(dir);
        match basename(dir) {
            "agents" => {
                for path in source.children(dir) {
                    if source.is_file(&path) && path.ends_with(".md") {
                        found.push(LayeredDefinition {
                            definition: SelectedDefinition::new(
                                DefinitionKind::Agent,
                                file_stem(&path).to_owned(),
                                path,
                                None,
                            ),
                            layer,
                        });
                    }
                }
            }
            "skills" => {
                for path in source.children(dir) {
                    if !is_hidden(&path)
                        && source.is_dir(&path)
                        && boundary.admits(source, &path, generated)
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
                                layer,
                            });
                        }
                    }
                }
            }
            _ => {}
        }
        true
    });

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
        boundary,
        generated,
        BTreeMap::new(),
    )
}

/// Sort, drop duplicate files, and refuse two definitions of one kind sharing a slug.
pub(crate) fn finish(
    mut definitions: Vec<SelectedDefinition>,
    dropped: Vec<DroppedItem>,
    boundary: Boundary,
    generated: Vec<String>,
    mut diagnostics: BTreeMap<String, Vec<SchemaDiagnostic>>,
) -> Discovery {
    report_generated(&mut diagnostics, generated.iter().map(String::as_str));
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
        boundary,
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

/// The generated plugin directory that holds `path` or any parent of it, if one does.
pub(crate) fn enclosing_generated_dir(source: &impl TreeSource, path: &str) -> Option<String> {
    let mut prefix = String::new();
    let mut components = std::iter::once("").chain(path.split('/').filter(|part| !part.is_empty()));
    components.find_map(|component| {
        prefix = join(&prefix, component);
        has_marker(source, &prefix).then(|| prefix.clone())
    })
}

/// The files a skill carries, source path to output path. `discovery.definitions` is every
/// discovered definition, so a nested skill is never its parent's payload whether or not it is
/// selected; `selected` are the skills whose payload is wanted. A foreign repository's skill
/// leaves out `.git` and `node_modules` (vendored trees, not the skill); a Mars package installs
/// whatever its skill holds, as every Mars release has.
pub(crate) fn payload_paths<'a>(
    source: &impl TreeSource,
    discovery: &Discovery,
    foreign: bool,
    selected: impl IntoIterator<Item = &'a SelectedDefinition>,
    diagnostics: &mut BTreeMap<String, Vec<SchemaDiagnostic>>,
) -> Vec<(String, String)> {
    let all = &discovery.definitions;
    let mut generated = Vec::new();
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
                if source.is_dir(&child) {
                    if (child != root && skill_roots.contains(child.as_str()))
                        || (foreign && matches!(basename(&child), ".git" | "node_modules"))
                        || !discovery.boundary.admits(source, &child, &mut generated)
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
    report_generated(diagnostics, generated.iter().map(String::as_str));
    payload.into_iter().collect()
}

pub(crate) fn join(parent: &str, child: &str) -> String {
    match (parent.is_empty(), child.is_empty()) {
        (true, _) => child.to_owned(),
        (_, true) => parent.to_owned(),
        _ => format!("{parent}/{child}"),
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
