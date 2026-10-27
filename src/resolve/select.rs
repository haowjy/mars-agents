//! Item selection: which Agents, skills and package-level documents of a settled dependency graph
//! a consumer installs.
//!
//! A pure pass over the graph the resolver settled. It seeds items from the consumer's own
//! requests and from unfiltered manifest edges, then closes over the skills the selected Agents
//! and skills name. Nothing here reads the filesystem, so the answer does not depend on the order
//! the resolver met the packages in.

use std::collections::{HashMap, HashSet, VecDeque};

use indexmap::IndexMap;
use mars_schema::filter::FilterMode;
use mars_schema::item::{DefinitionKind, ItemKind};
use mars_schema::prepare::Prepared;
use mars_schema::read::Reading;

use crate::types::SourceName;

/// A dependency entry as selection sees it: the package it names and the items it asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub name: String,
    pub filter: FilterMode,
}

/// An item of a package, found by the caller: what the crate does not prepare (bootstrap
/// documents, hooks).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub kind: ItemKind,
    pub slug: String,
    /// Package-relative location, `/`-separated.
    pub path: String,
}

#[derive(Debug)]
pub struct PackageView<'a> {
    pub prepared: &'a Prepared,
    /// The package's manifest entries, in declaration order.
    pub edges: Vec<Request>,
    pub uses_providers: Vec<String>,
    /// Bootstrap documents and hooks.
    pub extra_items: Vec<Item>,
}

#[derive(Debug)]
pub struct GraphView<'a> {
    /// The consumer's own entries.
    pub requests: Vec<Request>,
    /// Every package of the graph, in registry order: the last place a skill ref is looked up.
    pub packages: IndexMap<String, PackageView<'a>>,
}

/// Why an item is selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Requester {
    /// An entry of the consumer.
    Consumer,
    /// An unfiltered entry of this package's manifest, reached through unfiltered entries only.
    Package(String),
    /// A skill ref of this Agent or skill.
    Item {
        package: String,
        kind: ItemKind,
        slug: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingSkill {
    pub skill: String,
    /// `<package>/<slug>` of the Agent or skill that names it.
    pub required_by: String,
    pub searched: Vec<String>,
    /// For a name shaped like a provider's, when no package declares that provider.
    pub hint: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Selection {
    /// In package order of the graph, then the package's own item order.
    pub items: Vec<SelectedItem>,
    pub missing_skills: Vec<MissingSkill>,
}

/// An item with the definition it came from, when it has one.
struct Entry<'a> {
    item: Item,
    definition: Option<&'a mars_schema::prepare::PreparedDefinition>,
}

type Key<'a> = (&'a str, ItemKind, &'a str);

pub fn select(view: &GraphView) -> Selection {
    let entries: HashMap<&str, Vec<Entry>> = view
        .packages
        .iter()
        .map(|(name, package)| (name.as_str(), package_entries(package)))
        .collect();

    let seeds = seeds(view);
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
        let requester = Requester::Item {
            package: package.to_string(),
            kind,
            slug: slug.to_string(),
        };
        for skill in &skill_refs(entry.definition) {
            match resolve_skill(view, &entries, package, skill) {
                Resolved::Deferred => {}
                Resolved::Missing => missing_skills.push(missing_skill(view, package, slug, skill)),
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
            if entry.item.kind == ItemKind::BootstrapDoc {
                let key = (*package, entry.item.kind, entry.item.slug.as_str());
                let requesters = selected.entry(key).or_default();
                if !requesters.contains(requester) {
                    requesters.push(requester.clone());
                }
            }
        }
    }

    let mut items = Vec::with_capacity(selected.len());
    for (package, _) in &view.packages {
        for entry in &entries[package.as_str()] {
            let key = (package.as_str(), entry.item.kind, entry.item.slug.as_str());
            if let Some(required_by) = selected.get(&key) {
                items.push(SelectedItem {
                    package: package.clone(),
                    kind: entry.item.kind,
                    slug: entry.item.slug.clone(),
                    path: entry.item.path.clone(),
                    required_by: required_by.clone(),
                });
            }
        }
    }
    Selection {
        items,
        missing_skills,
    }
}

struct Seed<'a> {
    package: &'a str,
    filter: &'a FilterMode,
    requester: Requester,
}

/// The requests that seed items: the consumer's own, and each unfiltered manifest entry of a
/// package reached through unfiltered requests only. A filtered entry of a manifest seeds
/// nothing, and its `exclude` blocks nothing.
fn seeds<'a>(view: &'a GraphView) -> Vec<Seed<'a>> {
    let mut seeds = Vec::new();
    let mut open: HashSet<&str> = HashSet::new();
    let mut queue: VecDeque<&str> = VecDeque::new();
    for request in &view.requests {
        let Some((package, _)) = view.packages.get_key_value(&request.name) else {
            continue;
        };
        seeds.push(Seed {
            package: package.as_str(),
            filter: &request.filter,
            requester: Requester::Consumer,
        });
        if request.filter == FilterMode::All && open.insert(package.as_str()) {
            queue.push_back(package.as_str());
        }
    }
    while let Some(from) = queue.pop_front() {
        for edge in &view.packages[from].edges {
            let Some((package, _)) = view.packages.get_key_value(&edge.name) else {
                continue;
            };
            if edge.filter != FilterMode::All {
                continue;
            }
            seeds.push(Seed {
                package: package.as_str(),
                filter: &edge.filter,
                requester: Requester::Package(from.to_string()),
            });
            if open.insert(package.as_str()) {
                queue.push_back(package.as_str());
            }
        }
    }
    seeds
}

fn package_entries<'a>(package: &'a PackageView) -> Vec<Entry<'a>> {
    let definitions = package.prepared.definitions.iter().map(|definition| {
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
fn names_item(excluded: &[mars_schema::names::ItemName], item: &Item) -> bool {
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

/// The skills an Agent names. A skill names none: skills are chosen by the Agent profile.
fn skill_refs(definition: Option<&mars_schema::prepare::PreparedDefinition>) -> Vec<String> {
    match definition.map(|definition| &definition.reading) {
        Some(Reading::Agent(read)) => read
            .fields
            .as_ref()
            .map(|profile| profile.skills.all())
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

enum Resolved<'a> {
    In(&'a str),
    /// A ref `<skill>__<provider>` to a provider the requester declares, which no package ships.
    Deferred,
    Missing,
}

/// The package that supplies a skill ref: the requester's own, then its dependencies breadth
/// first, then any package in registry order.
fn resolve_skill<'a>(
    view: &'a GraphView,
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
        for edge in &view.packages[package].edges {
            if let Some((name, _)) = view.packages.get_key_value(&edge.name)
                && seen.insert(name.as_str())
            {
                queue.push_back(name.as_str());
            }
        }
    }
    if let Some(package) = closure.into_iter().find(|package| has_skill(package)) {
        return Resolved::In(package);
    }
    for name in view.packages.keys() {
        if !seen.contains(name.as_str()) && has_skill(name) {
            return Resolved::In(name);
        }
    }

    if mars_schema::provider::deferred_provider(
        skill,
        Some(&view.packages[requester].uses_providers),
    )
    .is_some()
    {
        return Resolved::Deferred;
    }
    Resolved::Missing
}

fn missing_skill(view: &GraphView, package: &str, slug: &str, skill: &str) -> MissingSkill {
    let mut searched: Vec<String> = view.packages.keys().cloned().collect();
    searched.sort();
    MissingSkill {
        skill: skill.to_string(),
        required_by: format!("{package}/{slug}"),
        searched,
        hint: mars_schema::provider::provider_of(skill)
            .map(mars_schema::provider::undeclared_provider_hint),
    }
}

/// Select from the registry the resolver settled and the consumer's own entries.
pub(super) fn select_registry(
    config: &crate::config::EffectiveConfig,
    registry: &IndexMap<SourceName, super::RegisteredPackage>,
) -> Selection {
    select_nodes(
        config,
        registry.iter().map(|(name, package)| {
            let extras = package
                .items()
                .filter(|item| !matches!(item.id.kind, ItemKind::Agent | ItemKind::Skill))
                .map(|item| Item {
                    kind: item.id.kind,
                    slug: item.id.name.to_string(),
                    path: slashed(&item.source_path.to_string_lossy()),
                })
                .collect();
            (name, &package.node, extras)
        }),
    )
}

/// Selection for a hand-built graph, whose bootstrap documents and hooks are found on disk.
#[cfg(test)]
pub(crate) fn select_for_test(
    config: &crate::config::EffectiveConfig,
    nodes: &IndexMap<SourceName, super::ResolvedNode>,
) -> Selection {
    select_nodes(
        config,
        nodes.iter().map(|(name, node)| {
            let prepared = node.prepared.as_deref().expect("a prepared test node");
            let extras = crate::discover::package_items(
                &node.rooted_ref.package_root,
                Some(name.as_str()),
                prepared,
                None,
                &mut crate::diagnostic::DiagnosticCollector::new(),
            )
            .expect("test tree")
            .into_iter()
            .filter(|item| !matches!(item.id.kind, ItemKind::Agent | ItemKind::Skill))
            .map(|item| Item {
                kind: item.id.kind,
                slug: item.id.name.to_string(),
                path: slashed(&item.source_path.to_string_lossy()),
            })
            .collect();
            (name, node, extras)
        }),
    )
}

fn select_nodes<'a>(
    config: &crate::config::EffectiveConfig,
    packages: impl Iterator<Item = (&'a SourceName, &'a super::ResolvedNode, Vec<Item>)>,
) -> Selection {
    let requests = config
        .dependencies
        .iter()
        .map(|(name, dependency)| Request {
            name: name.to_string(),
            filter: portable_filter(&dependency.filter),
        })
        .collect();
    let packages = packages
        .filter_map(|(name, node, extra_items)| {
            let view = PackageView {
                prepared: node.prepared.as_deref()?,
                edges: node
                    .edges
                    .iter()
                    .map(|edge| Request {
                        name: edge.name.to_string(),
                        filter: portable_filter(&edge.filter),
                    })
                    .collect(),
                uses_providers: node
                    .manifest
                    .as_ref()
                    .map(|manifest| manifest.package.uses_providers.clone())
                    .unwrap_or_default(),
                extra_items,
            };
            Some((name.to_string(), view))
        })
        .collect();
    select(&GraphView { requests, packages })
}

/// Selection compares paths as written with `/`; on Windows a path-style exclude may spell it `\`.
fn portable_filter(filter: &FilterMode) -> FilterMode {
    match filter {
        FilterMode::Exclude(names) if cfg!(windows) => FilterMode::Exclude(
            names
                .iter()
                .map(|name| slashed(name.as_str()).into())
                .collect(),
        ),
        other => other.clone(),
    }
}

fn slashed(path: &str) -> String {
    if cfg!(windows) {
        path.replace('\\', "/")
    } else {
        path.to_string()
    }
}
