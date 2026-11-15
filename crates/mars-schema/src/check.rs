//! What is wrong with a package, from its files and its dependencies: `mars check` and the WASM
//! `check` share this one function. It has no host I/O, and it reports what is true of the package
//! wherever it runs. A finding that exists only because a harness target or an older Mars cannot
//! take something belongs where that target or version exists (the binary).
//!
//! A level says what the author loses: `Error`, the file or package won't load or install;
//! `Warning`, something written won't take effect; `Info`, nothing written is lost.
//!
//! Per path, a result lists `prepare`'s findings, then the three kinds of change translation made
//! that an author should see ([`CHANGE_CODES`]), then the check's own rules in the order they run
//! below, stably sorted by level. Dropping the codes of those rules and changes from a path's list
//! leaves what `prepare` returns for it, so the two never disagree about a file.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use indexmap::IndexMap;
use serde::Serialize;

use crate::claude::translate::REPAIRED;
use crate::diagnostic::{PathSegment, SchemaDiagnostic, Severity, key_path};
use crate::discover::{Boundary, basename, holds_file, join, kept_skill_containers};
use crate::item::{DefinitionKind, ItemKind};
use crate::prepare::{Classification, Prepared, PreparedDefinition};
use crate::read::Reading;
use crate::selection::{
    GraphView, MissingSkill, PackageView, SelectionFacts, SelectionResult, duplicate_slugs, select,
};
use crate::set_aside::{EXECUTION_IGNORED, RESTRICTION_LOST};
use crate::tree::TreeSource;

/// The codes `check` takes from `Prepared::changes`.
const CHANGE_CODES: [&str; 3] = [RESTRICTION_LOST, EXECUTION_IGNORED, REPAIRED];

const MANIFEST: &str = "mars.toml";

/// What is wrong with a package.
#[derive(Debug, Clone, Serialize)]
pub struct Check {
    /// Findings in the root's own files, by source path: what `prepare` reported for the file,
    /// then the check's own. `mars.toml` holds what applies to the whole package and comes first,
    /// then the paths in lexical order.
    pub diagnostics: IndexMap<String, Vec<SchemaDiagnostic>>,
    /// `skill-ref.dependency` only: a dependency's own Agent names a skill no package ships. By
    /// the dependency's source name, then the Agent's output path (`agents/<slug>.md`).
    pub dependency_diagnostics: BTreeMap<String, BTreeMap<String, Vec<SchemaDiagnostic>>>,
    /// Every `[dependencies]` name reachable from the root that `dependencies` lacks. While it
    /// is not empty the result is not a verdict: no skill ref was judged.
    pub missing_dependencies: Vec<MissingDependency>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MissingDependency {
    pub name: String,
    /// The package whose `mars.toml` declares it; `None` for the root.
    pub declared_in: Option<String>,
}

/// What is wrong with the package that `prepared` (with [`crate::links::package`] applied) is
/// the result of preparing from `source`. `dependencies` is the whole closure: every package
/// reachable from the root through any `[dependencies]` entry, filtered or not, by the name the
/// declaring table gives its source.
pub fn check(
    source: &impl TreeSource,
    prepared: &Prepared,
    dependencies: &BTreeMap<String, SelectionFacts>,
) -> Check {
    let mut found = Found::default();
    let mut dependency_diagnostics = BTreeMap::new();
    let mut missing_dependencies = Vec::new();
    if matches!(
        prepared.classification,
        Classification::Schema1 | Classification::Foreign(_)
    ) {
        found.changes(prepared);
        found.requirement(prepared);
        found.definition_names(prepared);
        let graph = Graph::of(prepared, dependencies);
        found.selection_duplicates(prepared, &graph.selection);
        if graph.missing.is_empty() {
            found.skill_refs(prepared, &graph);
            dependency_diagnostics = graph.dependency_findings(dependencies);
        }
        if matches!(prepared.classification, Classification::Schema1) {
            found.skill_folders(source, prepared);
        }
        missing_dependencies = graph.missing;
    }
    Check {
        diagnostics: found.merge(prepared),
        dependency_diagnostics,
        missing_dependencies,
    }
}

/// The check's own findings by source path, in the order the rules run.
#[derive(Default)]
struct Found {
    changes: BTreeMap<String, Vec<SchemaDiagnostic>>,
    rules: BTreeMap<String, Vec<SchemaDiagnostic>>,
}

impl Found {
    fn rule(&mut self, path: &str, diagnostic: SchemaDiagnostic) {
        self.rules
            .entry(path.to_owned())
            .or_default()
            .push(diagnostic);
    }

    fn merge(self, prepared: &Prepared) -> IndexMap<String, Vec<SchemaDiagnostic>> {
        let mut by_path: BTreeMap<String, Vec<SchemaDiagnostic>> = prepared
            .all_diagnostics()
            .into_iter()
            .map(|(path, found)| (path.to_owned(), found.into_iter().cloned().collect()))
            .collect();
        for (path, found) in self.changes.into_iter().chain(self.rules) {
            by_path.entry(path).or_default().extend(found);
        }
        let mut merged = IndexMap::new();
        if let Some(found) = by_path.remove(MANIFEST) {
            merged.insert(MANIFEST.to_owned(), found);
        }
        merged.extend(by_path);
        for found in merged.values_mut() {
            // `Severity` is declared most severe first.
            found.sort_by_key(|diagnostic| diagnostic.severity as u8);
        }
        merged
    }

    /// The changes translation made that the author can act on.
    fn changes(&mut self, prepared: &Prepared) {
        for (path, changes) in &prepared.changes {
            for change in changes
                .iter()
                .filter(|change| CHANGE_CODES.contains(&change.code.as_str()))
            {
                self.changes
                    .entry(path.clone())
                    .or_default()
                    .push(change.clone());
            }
        }
    }

    /// `package.requirement`.
    fn requirement(&mut self, prepared: &Prepared) {
        // A generated manifest has no `requires-*` to read.
        let Some(package) = prepared
            .manifest
            .as_ref()
            .filter(|manifest| manifest.source_path.is_some())
            .and_then(|manifest| manifest.reading.package.fields.as_ref())
        else {
            return;
        };
        for (key, message) in crate::package::requirement_errors(package) {
            self.rule(
                MANIFEST,
                SchemaDiagnostic::new(
                    "package.requirement",
                    Severity::Error,
                    vec![
                        PathSegment::Key("package".into()),
                        PathSegment::Key(key.into()),
                    ],
                    message,
                ),
            );
        }
    }

    /// `agent.description-missing`, `agent.name-missing`, `definition.name-mismatch`,
    /// `definition.duplicate-name` and `definition.duplicate-slug`.
    fn definition_names(&mut self, prepared: &Prepared) {
        let items = prepared
            .definitions
            .iter()
            .filter_map(Named::of)
            .collect::<Vec<_>>();
        let agents = || items.iter().filter(|item| item.agent && !item.unfenced);

        for item in agents().filter(|item| !item.described) {
            self.rule(
                &item.definition.source_path,
                SchemaDiagnostic::new(
                    "agent.description-missing",
                    Severity::Info,
                    key_path("description"),
                    format!(
                        "agent `{}` has no `description`; Claude does not load an Agent without one",
                        item.name()
                    ),
                ),
            );
        }
        for item in agents().filter(|item| item.declared.is_none()) {
            self.rule(
                &item.definition.source_path,
                SchemaDiagnostic::new(
                    "agent.name-missing",
                    Severity::Info,
                    key_path("name"),
                    format!(
                        "agent `{}` has no `name`; the file name is used",
                        item.definition.slug
                    ),
                ),
            );
        }
        for item in items.iter().filter(|item| {
            item.declared
                .is_some_and(|name| name != item.definition.slug)
        }) {
            let (noun, file) = if item.agent {
                ("agent file", format!("{}.md", item.definition.slug))
            } else {
                ("skill folder", item.definition.slug.clone())
            };
            self.rule(
                &item.definition.source_path,
                SchemaDiagnostic::new(
                    "definition.name-mismatch",
                    Severity::Info,
                    key_path("name"),
                    format!(
                        "{noun} `{file}` doesn't match its `name` `{}`; the item loads under both",
                        item.name()
                    ),
                ),
            );
        }

        let mut by_name: BTreeMap<(DefinitionKind, &str), Vec<&Named>> = BTreeMap::new();
        for item in &items {
            by_name
                .entry((item.definition.kind, item.name()))
                .or_default()
                .push(item);
        }
        let shared = |item: &Named| by_name[&(item.definition.kind, item.name())].len() > 1;
        for item in items.iter().filter(|item| shared(item)) {
            let others = by_name[&(item.definition.kind, item.name())]
                .iter()
                .filter(|other| other.definition.source_path != item.definition.source_path)
                .map(|other| format!("`{}`", other.definition.source_path))
                .collect::<Vec<_>>()
                .join(", ");
            self.rule(
                &item.definition.source_path,
                SchemaDiagnostic::new(
                    "definition.duplicate-name",
                    Severity::Error,
                    key_path("name"),
                    format!(
                        "{} name `{}` is also the name of {others}",
                        item.definition.kind,
                        item.name()
                    ),
                ),
            );
        }

        // `prepare` reports a slug that two files share on the second only. The first is marked
        // here unless its `name` is shared too: `definition.duplicate-name` reports that.
        let mut by_slug: BTreeMap<(DefinitionKind, &str), Vec<&Named>> = BTreeMap::new();
        for item in &items {
            by_slug
                .entry((item.definition.kind, item.definition.slug.as_str()))
                .or_default()
                .push(item);
        }
        for items in by_slug.values() {
            let [first, second, ..] = items.as_slice() else {
                continue;
            };
            if !shared(first) {
                self.rule(
                    &first.definition.source_path,
                    SchemaDiagnostic::new(
                        "definition.duplicate-slug",
                        Severity::Error,
                        Vec::new(),
                        format!(
                            "{} slug `{}` is also selected by `{}`",
                            first.definition.kind,
                            first.definition.slug,
                            second.definition.source_path
                        ),
                    ),
                );
            }
        }
    }

    /// `selection.duplicate-slug`: a root item over a dependency's is only information, a slug
    /// two dependencies ship is a dependency the author declared installing less than it ships.
    fn selection_duplicates(&mut self, prepared: &Prepared, selection: &SelectionResult) {
        for duplicate in duplicate_slugs(&selection.items) {
            // Two files of the root's own are `import.duplicate-slug`, not two packages.
            if duplicate.packages.iter().flatten().next().is_none() {
                continue;
            }
            let dependencies = duplicate
                .packages
                .iter()
                .flatten()
                .map(|name| format!("`{name}`"))
                .collect::<Vec<_>>()
                .join(", ");
            if duplicate.packages.contains(&None) {
                let Some(root) = prepared.definitions.iter().find(|definition| {
                    ItemKind::from(definition.kind) == duplicate.kind
                        && definition.slug == duplicate.slug
                }) else {
                    continue;
                };
                self.rule(
                    &root.source_path,
                    SchemaDiagnostic::new(
                        "selection.duplicate-slug",
                        Severity::Info,
                        Vec::new(),
                        format!(
                            "{} `{}` is also shipped by {dependencies}; this package's is the one installed",
                            duplicate.kind, duplicate.slug
                        ),
                    ),
                );
            } else {
                self.rule(
                    MANIFEST,
                    SchemaDiagnostic::new(
                        "selection.duplicate-slug",
                        Severity::Warning,
                        key_path("dependencies"),
                        format!(
                            "{} `{}` is installed from {dependencies}, which ship it under one name",
                            duplicate.kind, duplicate.slug
                        ),
                    ),
                );
            }
        }
    }

    /// `skill-ref.missing`, `skill-ref.provider` and `skill-ref.external`, for the root's Agents.
    fn skill_refs(&mut self, prepared: &Prepared, graph: &Graph) {
        for missing in graph
            .selection
            .missing_skills
            .iter()
            .filter(|missing| missing.package.is_none())
        {
            let Some(agent) = prepared.definitions.iter().find(|definition| {
                definition.kind == DefinitionKind::Agent && definition.slug == missing.agent
            }) else {
                continue;
            };
            let (code, severity) = match &missing.hint {
                Some(_) if graph.has_package => ("skill-ref.provider", Severity::Error),
                _ if graph.has_dependencies => ("skill-ref.missing", Severity::Error),
                _ => ("skill-ref.external", Severity::Warning),
            };
            let message = if code == "skill-ref.external" {
                format!(
                    "agent `{}` references skill `{}`, which this package does not ship; a consumer must hold a package that does",
                    missing.agent, missing.skill
                )
            } else {
                missing_message(missing)
            };
            self.rule(
                &agent.source_path,
                SchemaDiagnostic::new(code, severity, key_path("skills"), message),
            );
        }
    }

    /// `skill.no-definition`: a folder directly under a `skills/` that discovery read, holding
    /// files and no `SKILL.md`.
    fn skill_folders(&mut self, source: &impl TreeSource, prepared: &Prepared) {
        let boundary = Boundary::of(source);
        let containers = kept_skill_containers(
            source,
            prepared
                .definitions
                .iter()
                .map(|definition| definition.source_path.as_str()),
        );
        for container in containers {
            for dir in source.children(&container) {
                if basename(&dir).starts_with('.')
                    || !source.is_dir(&dir)
                    || source.is_file(&join(&dir, "SKILL.md"))
                    || !boundary.admits(source, &dir, &mut Vec::new())
                    || !holds_file(source, &boundary, &dir)
                {
                    continue;
                }
                self.rule(
                    &dir,
                    SchemaDiagnostic::new(
                        "skill.no-definition",
                        Severity::Warning,
                        Vec::new(),
                        format!("`{dir}` has files and no `SKILL.md`, so none of it is installed"),
                    ),
                );
            }
        }
    }
}

/// An Agent or skill of the root that was read, with what its check rules need.
struct Named<'a> {
    definition: &'a PreparedDefinition,
    agent: bool,
    /// The author's `name`.
    declared: Option<&'a str>,
    described: bool,
    /// An Agent with no opening `---`: `frontmatter.missing` already says it has no fields.
    unfenced: bool,
}

impl<'a> Named<'a> {
    fn of(definition: &'a PreparedDefinition) -> Option<Self> {
        let (declared, described, unfenced) = match &definition.reading {
            Reading::Agent(reading) => {
                let profile = reading.fields.as_ref()?;
                (
                    profile.name.as_deref(),
                    profile.description.is_some(),
                    reading
                        .diagnostics
                        .iter()
                        .any(|diagnostic| diagnostic.code == "frontmatter.missing"),
                )
            }
            Reading::Skill(reading) => {
                let profile = reading.fields.as_ref()?;
                (
                    profile.name.as_deref(),
                    profile.description.is_some(),
                    false,
                )
            }
            Reading::Manifest(_) => return None,
        };
        Some(Self {
            agent: definition.kind == DefinitionKind::Agent,
            definition,
            declared,
            described,
            unfenced,
        })
    }

    /// The name the item goes by: its `name`, else the file or folder name.
    fn name(&self) -> &str {
        self.declared.unwrap_or(&self.definition.slug)
    }
}

/// Selection over the root and its dependencies, whole, as a sync of the package alone selects.
struct Graph {
    selection: SelectionResult,
    /// The root has `[package]`.
    has_package: bool,
    /// The root has `[package]` and `[dependencies]`; without them it cannot know what its
    /// consumer adds, and a ref to a skill it doesn't ship is only a warning.
    has_dependencies: bool,
    /// Dependencies a ref could resolve through that the caller left out.
    missing: Vec<MissingDependency>,
}

impl Graph {
    fn of(prepared: &Prepared, dependencies: &BTreeMap<String, SelectionFacts>) -> Self {
        let has_package = prepared
            .manifest
            .as_ref()
            .is_some_and(|manifest| manifest.reading.package.fields.is_some());
        let has_dependencies = has_package && !prepared.selection.edges.is_empty();
        let mut root = prepared.selection.clone();
        let mut packages = BTreeMap::new();
        if has_dependencies {
            packages = dependencies.clone();
        } else {
            root.edges.clear();
        }

        let mut missing = Vec::new();
        let mut seen = BTreeSet::new();
        let mut queue: VecDeque<(Option<&str>, &SelectionFacts)> = VecDeque::from([(None, &root)]);
        while let Some((declared_in, facts)) = queue.pop_front() {
            for edge in &facts.edges {
                if !seen.insert(edge.name.as_str()) {
                    continue;
                }
                match packages.get(&edge.name) {
                    Some(package) => queue.push_back((Some(edge.name.as_str()), package)),
                    None => missing.push(MissingDependency {
                        name: edge.name.clone(),
                        declared_in: declared_in.map(str::to_owned),
                    }),
                }
            }
        }

        let view = |facts| PackageView {
            facts,
            extra_items: Vec::new(),
        };
        let selection = select(&GraphView {
            requests: Vec::new(),
            packages: packages
                .into_iter()
                .map(|(name, facts)| (name, view(facts)))
                .collect(),
            root: Some(view(root)),
        });
        Self {
            selection,
            has_package,
            has_dependencies,
            missing,
        }
    }

    /// `skill-ref.dependency`: a dependency's own Agent names a skill no package ships.
    fn dependency_findings(
        &self,
        dependencies: &BTreeMap<String, SelectionFacts>,
    ) -> BTreeMap<String, BTreeMap<String, Vec<SchemaDiagnostic>>> {
        let mut found: BTreeMap<String, BTreeMap<String, Vec<SchemaDiagnostic>>> = BTreeMap::new();
        for missing in &self.selection.missing_skills {
            let Some(package) = missing.package.as_deref() else {
                continue;
            };
            let Some(agent) = dependencies.get(package).and_then(|facts| {
                facts.definitions.iter().find(|definition| {
                    definition.kind == DefinitionKind::Agent && definition.slug == missing.agent
                })
            }) else {
                continue;
            };
            found
                .entry(package.to_owned())
                .or_default()
                .entry(agent.output_path.clone())
                .or_default()
                .push(SchemaDiagnostic::new(
                    "skill-ref.dependency",
                    Severity::Error,
                    key_path("skills"),
                    missing_message(missing),
                ));
        }
        found
    }
}

/// The error for a ref no package ships, in the words `mars check` has always used.
fn missing_message(missing: &MissingSkill) -> String {
    let skill = &missing.skill;
    // An Agent of a dependency is that package's to fix, and the author's to work around.
    let agent = match &missing.package {
        Some(package) => format!("agent `{}` of dependency `{package}`", missing.agent),
        None => format!("agent `{}`", missing.agent),
    };
    if let Some(hint) = &missing.hint {
        return format!(
            "{agent} references skill `{skill}`, which is not in this package or its dependencies; {hint}"
        );
    }
    let remedy = match &missing.package {
        Some(package) => {
            format!("use a version of `{package}` that ships it, or add a dependency that does")
        }
        None => "add the skill's source package as a dependency, or remove the skill reference"
            .to_string(),
    };
    let searched = std::iter::once("(this package)")
        .chain(missing.searched.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{agent} references skill `{skill}` not found in local package or dependencies\n  searched: {searched}\n  hint: {remedy}"
    )
}
