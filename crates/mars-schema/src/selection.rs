//! Item selection: which Agents, skills and package-level documents of a settled dependency graph
//! a consumer installs.
//!
//! A pure pass over the graph the resolver settled. It seeds items from the consumer's own
//! requests and from unfiltered manifest edges, then closes over the skills the selected Agents
//! name. Nothing here reads the filesystem, so the answer does not depend on the order the
//! resolver met the packages in. Items are identified by package, kind and slug; the names they
//! install under (renames, collision suffixes) are the caller's.
//!
//! The rule, in three steps:
//!
//! 1. **Seeds.** Each of the consumer's requests seeds its package. A request that takes
//!    everything also opens its package: every unfiltered entry of an open package's manifest
//!    seeds its target in turn and opens it. A filtered entry of a manifest seeds nothing, and its
//!    `exclude` blocks nothing; only the consumer's own `exclude` does.
//! 2. **Items per request.** `All` takes every item; `Include` takes Agents and skills by kind and
//!    slug, and the package's bootstrap documents; `Exclude` takes all but the items it names by
//!    slug or path; `only_skills` and `only_agents` take one kind. Hooks are taken by `All` and
//!    `Exclude` only.
//! 3. **Skill refs.** Every selected Agent's `skills` resolve to a package: its own, then its
//!    dependencies breadth first, then any other package in registry order. The skill is selected
//!    unless the consumer excluded it, and a package it lands in also selects its bootstrap
//!    documents. A ref `<skill>__<provider>` that no package ships is skipped when the requester
//!    declares the provider, and is a [`MissingSkill`] otherwise.
//!
//! **Registry order** is computed here, not supplied: a depth-first walk from the root package
//! (if any), then each request in order, descending each package's manifest entries in
//! declaration order. [`GraphView::packages`] is a set. A package no request reaches is never
//! selected from and never supplies a skill.

use std::collections::{HashMap, HashSet, VecDeque};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::diagnostic::{SchemaDiagnostic, Severity};
use crate::filter::{FilterConfig, FilterMode};
use crate::frontmatter::SkillsSpec;
use crate::item::{DefinitionKind, ItemKind};
use crate::names::ItemName;
use crate::prepare::Prepared;
use crate::read::Reading;

/// A dependency entry as selection sees it: the package it names and the items it asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub name: String,
    pub filter: FilterMode,
}

/// An item of a package, found by the caller: what the crate does not prepare (bootstrap
/// documents, hooks).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub kind: ItemKind,
    pub slug: String,
    /// Package-relative location, `/`-separated.
    pub path: String,
}

/// An Agent or skill of a package, with the skills an Agent names.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Definition {
    pub kind: DefinitionKind,
    pub slug: String,
    pub output_path: String,
    /// The skills the Agent names; empty for a skill.
    #[serde(default)]
    pub skills: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PackageView {
    pub definitions: Vec<Definition>,
    /// The package's manifest entries, in declaration order.
    pub edges: Vec<Request>,
    pub uses_providers: Vec<String>,
    /// Bootstrap documents and hooks. They never change which Agents and skills are selected:
    /// only Agents and skills are followed, so a caller with none to give passes `[]`.
    pub extra_items: Vec<Item>,
}

impl PackageView {
    /// A package whose edges and `uses-providers` are those its prepared manifest reads: the
    /// `[dependencies]` table, never `[local-dependencies]`.
    pub fn from_prepared(prepared: &Prepared, extra_items: Vec<Item>) -> Self {
        let reading = prepared.manifest.as_ref().map(|manifest| &manifest.reading);
        Self {
            definitions: prepared
                .definitions
                .iter()
                .map(|definition| Definition {
                    kind: definition.kind,
                    slug: definition.slug.clone(),
                    output_path: definition.output_path.clone(),
                    skills: match &definition.reading {
                        Reading::Agent(read) => read
                            .fields
                            .as_ref()
                            .map(|profile| profile.skills.all())
                            .unwrap_or_default(),
                        _ => Vec::new(),
                    },
                })
                .collect(),
            edges: reading
                .map(|reading| {
                    reading
                        .dependencies
                        .dependencies
                        .iter()
                        .map(|(name, entry)| Request {
                            name: name.clone(),
                            filter: entry.filter.to_mode(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            uses_providers: reading
                .and_then(|reading| reading.package.fields.as_ref())
                .map(|package| package.uses_providers.clone())
                .unwrap_or_default(),
            extra_items,
        }
    }
}

/// A package as a caller holds it across the JSON boundary: the [`Prepared`] it got earlier
/// (`prepare`'s result, as serialized) and the items preparation does not read. Only the parts
/// of `prepared` that [`PackageView::from_prepared`] reads are read here; the rest is ignored, so
/// a caller passes the result whole and re-derives nothing.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageInput {
    prepared: PreparedFacts,
    extra_items: Option<Vec<Item>>,
}

#[derive(Debug, Deserialize)]
struct PreparedFacts {
    manifest: Option<ManifestFacts>,
    definitions: Vec<DefinitionFacts>,
}

#[derive(Debug, Deserialize)]
struct ManifestFacts {
    reading: ManifestReadingFacts,
}

#[derive(Debug, Deserialize)]
struct ManifestReadingFacts {
    #[serde(default)]
    fields: Option<PackageFacts>,
    #[serde(default)]
    dependencies: IndexMap<String, DependencyFacts>,
}

#[derive(Debug, Deserialize)]
struct PackageFacts {
    #[serde(default, rename = "uses-providers")]
    uses_providers: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct DependencyFacts {
    #[serde(flatten)]
    filter: FilterConfig,
}

#[derive(Debug, Deserialize)]
struct DefinitionFacts {
    kind: DefinitionKind,
    slug: String,
    output_path: String,
    reading: DefinitionReadingFacts,
}

#[derive(Debug, Deserialize)]
struct DefinitionReadingFacts {
    #[serde(default)]
    fields: Option<DefinitionFieldsFacts>,
}

#[derive(Debug, Deserialize)]
struct DefinitionFieldsFacts {
    #[serde(default)]
    skills: SkillsSpec,
}

impl From<PackageInput> for PackageView {
    fn from(input: PackageInput) -> Self {
        let PreparedFacts {
            manifest,
            definitions,
        } = input.prepared;
        let (fields, dependencies) = manifest
            .map(|manifest| (manifest.reading.fields, manifest.reading.dependencies))
            .unwrap_or_default();
        Self {
            definitions: definitions
                .into_iter()
                .map(|definition| Definition {
                    kind: definition.kind,
                    slug: definition.slug,
                    output_path: definition.output_path,
                    // A skill's reading has no `skills`.
                    skills: match definition.kind {
                        DefinitionKind::Agent => definition
                            .reading
                            .fields
                            .map(|fields| fields.skills.all())
                            .unwrap_or_default(),
                        DefinitionKind::Skill => Vec::new(),
                    },
                })
                .collect(),
            edges: dependencies
                .into_iter()
                .map(|(name, entry)| Request {
                    name,
                    filter: entry.filter.to_mode(),
                })
                .collect(),
            uses_providers: fields
                .map(|fields| fields.uses_providers)
                .unwrap_or_default(),
            extra_items: input.extra_items.unwrap_or_default(),
        }
    }
}

#[derive(Debug)]
pub struct GraphView {
    /// The consumer's own entries.
    pub requests: Vec<Request>,
    /// The package being selected for, such as a project's own package: it is requested whole,
    /// as the first request, and its manifest entries are its edges. Its Agents' skill refs
    /// resolve like any package's, but its own items are not in the result.
    pub root: Option<PackageView>,
    /// Every package of the graph, in no particular order: [`select`] orders them (see the module
    /// documentation).
    pub packages: HashMap<String, PackageView>,
}

/// Why an item is selected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "by", rename_all = "kebab-case")]
pub enum Requester {
    /// An entry of the consumer, or of the root package: the root's manifest entries and its
    /// Agents' skill refs are the consumer's own.
    Consumer,
    /// An unfiltered entry of this package's manifest, reached through unfiltered entries only.
    Package { package: String },
    /// A skill ref of this Agent.
    Item {
        package: String,
        kind: ItemKind,
        slug: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SelectedItem {
    pub package: String,
    pub kind: ItemKind,
    pub slug: String,
    /// Location in the prepared package: an Agent's file, a skill's directory (`.` for a flat
    /// skill), a document's or hook's directory.
    pub path: String,
    pub required_by: Vec<Requester>,
}

/// A skill ref no package ships.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MissingSkill {
    pub skill: String,
    /// The Agent that names it.
    pub agent: String,
    /// The package of that Agent; `None` for the root.
    pub package: Option<String>,
    /// The packages looked in, sorted; the root is not among them.
    pub searched: Vec<String>,
    /// For a name shaped like a provider's, when no package declares that provider.
    pub hint: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct SelectionResult {
    /// In package order of the graph, then the package's own item order.
    pub items: Vec<SelectedItem>,
    pub missing_skills: Vec<MissingSkill>,
    /// `selection.duplicate-slug`: an Agent or skill slug that two packages both install, which
    /// installs under one name only after a rename or a collision suffix.
    pub diagnostics: Vec<SchemaDiagnostic>,
}

/// An item with the definition it came from, when it has one.
struct Entry<'a> {
    item: Item,
    definition: Option<&'a Definition>,
}

type Key<'a> = (&'a str, ItemKind, &'a str);

/// The root's key among the packages. No dependency has an empty name.
const ROOT: &str = "";

/// The packages in registry order, the root among them.
struct Graph<'v> {
    packages: HashMap<&'v str, &'v PackageView>,
    order: Vec<&'v str>,
}

impl<'v> Graph<'v> {
    fn new(view: &'v GraphView) -> Self {
        let mut packages: HashMap<&str, &PackageView> = view
            .packages
            .iter()
            .map(|(name, package)| (name.as_str(), package))
            .collect();
        if let Some(root) = &view.root {
            packages.insert(ROOT, root);
        }
        // Depth first, a package before its entries; a stack pops the first entry first.
        let mut order = Vec::new();
        let mut seen: HashSet<&str> = HashSet::new();
        let mut stack: Vec<&str> = Vec::new();
        let starts = view
            .root
            .iter()
            .map(|_| ROOT)
            .chain(view.requests.iter().map(|request| request.name.as_str()));
        for start in starts {
            stack.push(start);
            while let Some(name) = stack.pop() {
                let Some((&name, package)) = packages.get_key_value(name) else {
                    continue;
                };
                if seen.insert(name) {
                    order.push(name);
                    stack.extend(package.edges.iter().rev().map(|edge| edge.name.as_str()));
                }
            }
        }
        Self { packages, order }
    }
}

pub fn select(view: &GraphView) -> SelectionResult {
    let graph = Graph::new(view);
    let entries: HashMap<&str, Vec<Entry>> = graph
        .order
        .iter()
        .map(|&name| (name, package_entries(graph.packages[name])))
        .collect();

    let seeds = seeds(view, &graph);
    let excluded: HashMap<&str, Vec<&FilterMode>> = {
        let mut excluded: HashMap<&str, Vec<&FilterMode>> = HashMap::new();
        for seed in &seeds {
            if matches!(seed.filter, FilterMode::Exclude(_)) {
                excluded.entry(seed.package).or_default().push(seed.filter);
            }
        }
        excluded
    };

    let mut selected: IndexMap<Key, Vec<Requester>> = IndexMap::new();
    let mut queue: VecDeque<Key> = VecDeque::new();
    for seed in &seeds {
        for entry in &entries[seed.package] {
            if !seed_accepts(seed.filter, &entry.item) {
                continue;
            }
            let key = (seed.package, entry.item.kind, entry.item.slug.as_str());
            let requesters = selected.entry(key).or_default();
            if !requesters.contains(&seed.requester) {
                requesters.push(seed.requester.clone());
            }
            if matches!(entry.item.kind, ItemKind::Agent | ItemKind::Skill) {
                queue.push_back(key);
            }
        }
    }

    let mut missing_skills = Vec::new();
    // A package a skill ref lands in installs its bootstrap documents, as any requested part of
    // a package does.
    let mut pulled: IndexMap<&str, Requester> = IndexMap::new();
    let mut processed: HashSet<Key> = HashSet::new();
    while let Some(key) = queue.pop_front() {
        if !processed.insert(key) {
            continue;
        }
        let (package, kind, slug) = key;
        let Some(entry) = entries[package]
            .iter()
            .find(|entry| entry.item.kind == kind && entry.item.slug == slug)
        else {
            continue;
        };
        let requester = if package == ROOT {
            Requester::Consumer
        } else {
            Requester::Item {
                package: package.to_string(),
                kind,
                slug: slug.to_string(),
            }
        };
        for skill in entry
            .definition
            .map_or(&[][..], |definition| definition.skills.as_slice())
        {
            match resolve_skill(&graph, &entries, package, skill) {
                Resolved::Deferred => {}
                Resolved::Missing => {
                    missing_skills.push(missing_skill(&graph, package, slug, skill))
                }
                Resolved::In(target) => {
                    let target_entry = entries[target]
                        .iter()
                        .find(|entry| {
                            entry.item.kind == ItemKind::Skill && entry.item.slug == *skill
                        })
                        .expect("a package that has the skill lists it");
                    if is_excluded(&excluded, target, &target_entry.item) {
                        continue;
                    }
                    pulled.entry(target).or_insert_with(|| requester.clone());
                    let key = (target, ItemKind::Skill, target_entry.item.slug.as_str());
                    let requesters = selected.entry(key).or_default();
                    if !requesters.contains(&requester) {
                        requesters.push(requester.clone());
                    }
                    queue.push_back(key);
                }
            }
        }
    }
    for (package, requester) in &pulled {
        for entry in &entries[package] {
            if entry.item.kind == ItemKind::BootstrapDoc
                && !is_excluded(&excluded, package, &entry.item)
            {
                let key = (*package, entry.item.kind, entry.item.slug.as_str());
                let requesters = selected.entry(key).or_default();
                if !requesters.contains(requester) {
                    requesters.push(requester.clone());
                }
            }
        }
    }

    let mut items = Vec::with_capacity(selected.len());
    for &package in graph.order.iter().filter(|&&package| package != ROOT) {
        for entry in &entries[package] {
            let key = (package, entry.item.kind, entry.item.slug.as_str());
            if let Some(required_by) = selected.get(&key) {
                items.push(SelectedItem {
                    package: package.to_string(),
                    kind: entry.item.kind,
                    slug: entry.item.slug.clone(),
                    path: entry.item.path.clone(),
                    required_by: required_by.clone(),
                });
            }
        }
    }
    let diagnostics = duplicate_slugs(&items);
    SelectionResult {
        items,
        missing_skills,
        diagnostics,
    }
}

fn duplicate_slugs(items: &[SelectedItem]) -> Vec<SchemaDiagnostic> {
    let mut packages: IndexMap<(ItemKind, &str), Vec<&str>> = IndexMap::new();
    for item in items
        .iter()
        .filter(|item| matches!(item.kind, ItemKind::Agent | ItemKind::Skill))
    {
        packages
            .entry((item.kind, item.slug.as_str()))
            .or_default()
            .push(item.package.as_str());
    }
    packages
        .into_iter()
        .filter(|(_, packages)| packages.len() > 1)
        .map(|((kind, slug), packages)| {
            SchemaDiagnostic::new(
                "selection.duplicate-slug",
                Severity::Warning,
                Vec::new(),
                format!("{kind} `{slug}` is installed from {}", packages.join(", ")),
            )
        })
        .collect()
}

struct Seed<'a> {
    package: &'a str,
    filter: &'a FilterMode,
    requester: Requester,
}

/// The requests that seed items: the root and the consumer's own, and each unfiltered manifest
/// entry of a package reached through unfiltered requests only. A filtered entry of a manifest
/// seeds nothing, and its `exclude` blocks nothing.
fn seeds<'a>(view: &'a GraphView, graph: &Graph<'a>) -> Vec<Seed<'a>> {
    static ALL: FilterMode = FilterMode::All;
    let mut seeds = Vec::new();
    let mut open: HashSet<&str> = HashSet::new();
    let mut queue: VecDeque<&str> = VecDeque::new();
    if view.root.is_some() {
        seeds.push(Seed {
            package: ROOT,
            filter: &ALL,
            requester: Requester::Consumer,
        });
        open.insert(ROOT);
        queue.push_back(ROOT);
    }
    for request in &view.requests {
        let Some((&package, _)) = graph.packages.get_key_value(request.name.as_str()) else {
            continue;
        };
        seeds.push(Seed {
            package,
            filter: &request.filter,
            requester: Requester::Consumer,
        });
        if request.filter == FilterMode::All && open.insert(package) {
            queue.push_back(package);
        }
    }
    while let Some(from) = queue.pop_front() {
        for edge in &graph.packages[from].edges {
            let Some((&package, _)) = graph.packages.get_key_value(edge.name.as_str()) else {
                continue;
            };
            if edge.filter != FilterMode::All {
                continue;
            }
            seeds.push(Seed {
                package,
                filter: &edge.filter,
                requester: if from == ROOT {
                    Requester::Consumer
                } else {
                    Requester::Package {
                        package: from.to_string(),
                    }
                },
            });
            if open.insert(package) {
                queue.push_back(package);
            }
        }
    }
    seeds
}

fn package_entries<'a>(package: &'a PackageView) -> Vec<Entry<'a>> {
    let definitions = package.definitions.iter().map(|definition| {
        let (kind, path) = match definition.kind {
            DefinitionKind::Agent => (ItemKind::Agent, definition.output_path.clone()),
            DefinitionKind::Skill => (
                ItemKind::Skill,
                match definition.output_path.rsplit_once('/') {
                    Some((parent, _)) if !parent.is_empty() => parent.to_string(),
                    _ => ".".to_string(),
                },
            ),
        };
        Entry {
            item: Item {
                kind,
                slug: definition.slug.clone(),
                path,
            },
            definition: Some(definition),
        }
    });
    let extras = package.extra_items.iter().map(|item| Entry {
        item: item.clone(),
        definition: None,
    });
    definitions.chain(extras).collect()
}

/// Whether a request's filter takes the item. Hooks and MCP servers are not selectable by name or
/// category, only by taking everything.
fn seed_accepts(filter: &FilterMode, item: &Item) -> bool {
    match filter {
        FilterMode::All => true,
        FilterMode::Include { agents, skills } => match item.kind {
            ItemKind::Agent => agents.iter().any(|name| name.as_str() == item.slug),
            ItemKind::Skill => skills.iter().any(|name| name.as_str() == item.slug),
            // Package-level documents travel with any requested part of the package.
            ItemKind::BootstrapDoc => true,
            ItemKind::Hook | ItemKind::McpServer => false,
        },
        FilterMode::Exclude(excluded) => !names_item(excluded, item),
        FilterMode::OnlySkills => item.kind == ItemKind::Skill,
        FilterMode::OnlyAgents => item.kind == ItemKind::Agent,
    }
}

/// An exclude entry names an item by slug or by path, whatever the item's kind.
fn names_item(excluded: &[ItemName], item: &Item) -> bool {
    excluded
        .iter()
        .any(|name| name.as_str() == item.slug || name.as_str() == item.path)
}

fn is_excluded(excluded: &HashMap<&str, Vec<&FilterMode>>, package: &str, item: &Item) -> bool {
    excluded.get(package).is_some_and(|filters| {
        filters.iter().any(|filter| match filter {
            FilterMode::Exclude(names) => names_item(names, item),
            _ => false,
        })
    })
}

enum Resolved<'a> {
    In(&'a str),
    /// A ref `<skill>__<provider>` to a provider the requester declares, which no package ships.
    Deferred,
    Missing,
}

/// The package that supplies a skill ref: the requester's own, then its dependencies breadth
/// first, then any other package in registry order.
fn resolve_skill<'a>(
    graph: &Graph<'a>,
    entries: &HashMap<&str, Vec<Entry>>,
    requester: &'a str,
    skill: &str,
) -> Resolved<'a> {
    let has_skill = |package: &str| {
        entries[package]
            .iter()
            .any(|entry| entry.item.kind == ItemKind::Skill && entry.item.slug == skill)
    };
    if has_skill(requester) {
        return Resolved::In(requester);
    }

    // Breadth first keeps direct dependencies ahead of transitive ones.
    let mut closure: Vec<&str> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::from([requester]);
    let mut queue: VecDeque<&str> = VecDeque::from([requester]);
    while let Some(package) = queue.pop_front() {
        if package != requester {
            closure.push(package);
        }
        for edge in &graph.packages[package].edges {
            if let Some((&name, _)) = graph.packages.get_key_value(edge.name.as_str())
                && seen.insert(name)
            {
                queue.push_back(name);
            }
        }
    }
    if let Some(package) = closure.into_iter().find(|package| has_skill(package)) {
        return Resolved::In(package);
    }
    for &name in &graph.order {
        if !seen.contains(name) && has_skill(name) {
            return Resolved::In(name);
        }
    }

    if crate::provider::deferred_provider(skill, Some(&graph.packages[requester].uses_providers))
        .is_some()
    {
        return Resolved::Deferred;
    }
    Resolved::Missing
}

fn missing_skill(graph: &Graph, package: &str, agent: &str, skill: &str) -> MissingSkill {
    let mut searched: Vec<String> = graph
        .order
        .iter()
        .filter(|&&name| name != ROOT)
        .map(|name| name.to_string())
        .collect();
    searched.sort();
    MissingSkill {
        skill: skill.to_string(),
        agent: agent.to_string(),
        package: (package != ROOT).then(|| package.to_string()),
        searched,
        hint: crate::provider::provider_of(skill).map(crate::provider::undeclared_provider_hint),
    }
}
