use mars_schema::version::{Constraint, SelectionPolicy};
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;

use indexmap::IndexMap;

use super::compat::{CompatibilityResult, compatible_with_resolved};
use crate::config::{EffectiveDependency, Manifest};
use crate::error::ResolutionError;
use crate::source::ResolvedRef;
use crate::types::{SourceId, SourceName};
use mars_schema::claude::manifest::PluginSource;
use mars_schema::filter::FilterMode;
use mars_schema::item::ItemKind;
use mars_schema::names::ItemName;

/// The resolved dependency graph — all sources with concrete versions.
///
/// Produced by the resolver after fetching sources, reading manifests,
/// intersecting version constraints, and deterministic ordering.
#[derive(Debug, Clone)]
pub struct ResolvedGraph {
    pub nodes: IndexMap<SourceName, ResolvedNode>,
    /// Deterministic alphabetical order (prompt packages don't require dependency ordering).
    pub order: Vec<SourceName>,
    /// The Agents, skills and documents the consumer installs, chosen once the graph settled.
    pub selection: mars_schema::selection::Selection,
    /// All version constraints collected for each source (direct + transitive).
    pub version_constraints: HashMap<SourceName, Vec<(String, Constraint)>>,
    /// Hook surfaces that cannot be compiled because they use a removed schema.
    ///
    /// This is classified during staging and consumed only by the recovery halt
    /// gate at the reader/compiler boundary.
    pub unreadable_hook_surfaces:
        std::collections::BTreeMap<SourceName, std::collections::BTreeSet<String>>,
}

impl ResolvedGraph {
    /// Shortest dependency distance from the consumer's direct declarations.
    pub(crate) fn dependency_depths(
        &self,
        dependencies: &IndexMap<SourceName, EffectiveDependency>,
    ) -> HashMap<SourceName, usize> {
        let mut depths = HashMap::new();
        let mut queue = VecDeque::new();
        for name in dependencies.keys() {
            if self.nodes.contains_key(name) && !depths.contains_key(name) {
                depths.insert(name.clone(), 1);
                queue.push_back(name.clone());
            }
        }

        while let Some(current) = queue.pop_front() {
            let current_depth = depths[&current];
            if let Some(node) = self.nodes.get(&current) {
                for dep in node.deps() {
                    if !self.nodes.contains_key(dep) {
                        continue;
                    }
                    let candidate = current_depth + 1;
                    if depths.get(dep).is_none_or(|depth| candidate < *depth) {
                        depths.insert(dep.clone(), candidate);
                        queue.push_back(dep.clone());
                    }
                }
            }
        }

        depths
    }

    pub(crate) fn dependency_decl_orders(
        &self,
        dependencies: &IndexMap<SourceName, EffectiveDependency>,
    ) -> HashMap<SourceName, usize> {
        let mut orders = HashMap::new();
        let mut queue = VecDeque::new();

        for (idx, source_name) in dependencies.keys().enumerate() {
            if self.nodes.contains_key(source_name) {
                orders.insert(source_name.clone(), idx + 1);
                queue.push_back(source_name.clone());
            }
        }

        while let Some(current) = queue.pop_front() {
            let current_order = orders[&current];
            let Some(node) = self.nodes.get(&current) else {
                continue;
            };

            for dep in node.deps() {
                if !self.nodes.contains_key(dep) {
                    continue;
                }
                match orders.get_mut(dep) {
                    Some(existing) if current_order < *existing => {
                        *existing = current_order;
                        queue.push_back(dep.clone());
                    }
                    Some(_) => {}
                    None => {
                        orders.insert(dep.clone(), current_order);
                        queue.push_back(dep.clone());
                    }
                }
            }
        }

        orders
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct EngineFallbackSkippedVersion {
    pub version: String,
    pub requirements: Vec<EngineFallbackRequirement>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct EngineFallbackRequirement {
    pub engine: String,
    pub requirement: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct EngineFallback {
    pub source: String,
    pub skipped: Vec<EngineFallbackSkippedVersion>,
    pub selected_version: String,
    pub engines: Vec<String>,
}

/// A single node in the resolved graph.
#[derive(Debug, Clone)]
pub struct ResolvedNode {
    pub source_name: SourceName,
    pub source_id: SourceId,
    pub rooted_ref: RootedSourceRef,
    /// Crate-owned semantic preparation result for this authored package.
    pub prepared: Option<std::sync::Arc<mars_schema::prepare::Prepared>>,
    pub resolved_ref: ResolvedRef,
    /// None if source has no mars.toml.
    pub manifest: Option<Manifest>,
    /// This package's manifest entries, in declaration order.
    pub edges: Vec<Edge>,
}

impl ResolvedNode {
    /// Source names this depends on.
    pub fn deps(&self) -> impl Iterator<Item = &SourceName> {
        self.edges.iter().map(|edge| &edge.name)
    }
}

/// One entry of a package's manifest: the package it names and the items it asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    pub name: SourceName,
    pub filter: FilterMode,
}

impl Edge {
    pub fn unfiltered(name: impl Into<SourceName>) -> Self {
        Self {
            name: name.into(),
            filter: FilterMode::All,
        }
    }
}

/// Source checkout provenance and rooted package boundary.
#[derive(Debug, Clone)]
pub struct RootedSourceRef {
    /// The checkout the package's files came from: the marketplace's for a local plugin, the
    /// plugin's own repository for a remote one.
    pub checkout_root: PathBuf,
    /// Immutable package origin after applying `subpath` (or the plugin's root).
    pub authored_root: PathBuf,
    /// Current materialized content view. Relative dependencies must never use this.
    pub package_root: PathBuf,
    /// Set when the package is one plugin of a marketplace.
    pub plugin: Option<PluginRoot>,
}

/// A marketplace plugin as acquired: kept with the rooted package so that preparing it again
/// (a restart pass) reads it through the same marketplace entry, never a fresh fetch.
#[derive(Debug, Clone)]
pub struct PluginRoot {
    pub hint: mars_schema::inspect::PluginHint,
    /// Where the plugin's files came from.
    pub content: crate::lock::LockedContent,
    /// The dependency named this plugin; otherwise its marketplace lists only this one.
    pub named: bool,
}

impl PluginRoot {
    pub fn name(&self) -> &str {
        &self.hint.entry.name
    }

    /// The plugin the lock records: only one the dependency named, so a lock written for a
    /// dependency that never named one (v0.19.0's) still describes it.
    pub fn locked_name(&self) -> Option<&str> {
        self.named.then(|| self.name())
    }

    /// Where the plugin's files came from, when the lock must say: for a plugin the dependency
    /// named, or one in another repository, which a later sync fetches again by this record. An
    /// only plugin in the marketplace's own tree is the marketplace's commit and needs none.
    pub fn locked_content(&self) -> Option<&crate::lock::LockedContent> {
        (self.named || matches!(self.hint.entry.source, PluginSource::Remote { .. }))
            .then_some(&self.content)
    }
}

/// An item waiting to be processed in DFS traversal.
#[derive(Debug, Clone)]
pub struct PendingItem {
    /// Package containing this item.
    pub package: SourceName,
    /// Item name.
    pub item: ItemName,
    /// Agent or Skill.
    pub kind: ItemKind,
    /// Version constraint from config.
    pub constraint: Constraint,
    /// Who requested this item (for error context).
    pub required_by: String,
    /// True if from a local path dependency (skip version checks).
    pub is_local: bool,
}

/// Result of checking whether an item was seen already.
#[derive(Debug)]
pub enum VersionCheckResult {
    /// Item has not been visited yet.
    NotSeen,
    /// Item was visited with a compatible version.
    SameVersion,
    /// Item was visited with a potentially conflicting version (latest vs pinned).
    PotentiallyConflicting {
        existing: Constraint,
        requested: Constraint,
    },
    /// Item was visited with a conflicting version.
    DifferentVersion {
        existing: Constraint,
        requested: Constraint,
    },
}

/// Stable key for visited items.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
struct VisitedItem {
    package: SourceName,
    item: ItemName,
}

/// Stored version information for a visited item.
#[derive(Debug, Clone)]
pub struct ResolvedVersion {
    pub constraint: Constraint,
    pub resolved_ref: ResolvedRef,
}

/// Tracks visited items with version-aware lookup for DFS traversal.
pub struct VisitedSet {
    /// Fast lookup by (package, item).
    index: HashMap<(SourceName, ItemName), ResolvedVersion>,
}

impl Default for VisitedSet {
    fn default() -> Self {
        Self::new()
    }
}

impl VisitedSet {
    pub fn new() -> Self {
        Self {
            index: HashMap::new(),
        }
    }

    fn index_key(package: &SourceName, item: &ItemName) -> (SourceName, ItemName) {
        let key = VisitedItem {
            package: package.clone(),
            item: item.clone(),
        };
        (key.package, key.item)
    }

    /// Check whether an item was visited and compare version constraints.
    pub fn check_version(
        &self,
        package: &SourceName,
        item: &ItemName,
        constraint: &Constraint,
    ) -> VersionCheckResult {
        match self.index.get(&Self::index_key(package, item)) {
            None => VersionCheckResult::NotSeen,
            Some(existing) => match compatible_with_resolved(
                &existing.constraint,
                constraint,
                existing.resolved_ref.version.as_ref(),
            ) {
                CompatibilityResult::Compatible => VersionCheckResult::SameVersion,
                CompatibilityResult::PotentiallyConflicting => {
                    VersionCheckResult::PotentiallyConflicting {
                        existing: existing.constraint.clone(),
                        requested: constraint.clone(),
                    }
                }
                CompatibilityResult::Conflicting => VersionCheckResult::DifferentVersion {
                    existing: existing.constraint.clone(),
                    requested: constraint.clone(),
                },
            },
        }
    }

    /// Insert an item as visited.
    pub fn insert(
        &mut self,
        package: SourceName,
        item: ItemName,
        constraint: Constraint,
        resolved_ref: ResolvedRef,
    ) {
        self.index.insert(
            Self::index_key(&package, &item),
            ResolvedVersion {
                constraint,
                resolved_ref,
            },
        );
    }
}

/// Tracks resolved version per package and rejects divergent refs.
pub struct PackageVersions {
    /// package -> (resolved_ref, first_constraint, first_required_by)
    versions: HashMap<SourceName, (ResolvedRef, Constraint, String)>,
}

impl Default for PackageVersions {
    fn default() -> Self {
        Self::new()
    }
}

impl PackageVersions {
    pub fn new() -> Self {
        Self {
            versions: HashMap::new(),
        }
    }

    /// Check existing package version or insert if first time seen.
    pub fn check_or_insert(
        &mut self,
        package: &SourceName,
        resolved: &ResolvedRef,
        requested: &Constraint,
        required_by: &str,
        is_local: bool,
    ) -> Result<(), ResolutionError> {
        if is_local {
            return Ok(());
        }

        match self.versions.entry(package.clone()) {
            Entry::Vacant(entry) => {
                entry.insert((resolved.clone(), requested.clone(), required_by.to_string()));
                Ok(())
            }
            Entry::Occupied(entry) => {
                let (existing_ref, existing_constraint, existing_by) = entry.get();
                match compatible_with_resolved(
                    existing_constraint,
                    requested,
                    existing_ref.version.as_ref().or(resolved.version.as_ref()),
                ) {
                    CompatibilityResult::Compatible
                    | CompatibilityResult::PotentiallyConflicting => {
                        if resolved_ref_matches(existing_ref, resolved) {
                            Ok(())
                        } else {
                            Err(ResolutionError::PackageVersionConflict {
                                package: package.to_string(),
                                existing: format!("{existing_ref:?} (required by {existing_by})"),
                                requested: format!("{resolved:?} (required by {required_by})"),
                                chain: required_by.to_string(),
                            })
                        }
                    }
                    CompatibilityResult::Conflicting => {
                        Err(ResolutionError::PackageVersionConflict {
                            package: package.to_string(),
                            existing: format!("{existing_constraint} (required by {existing_by})"),
                            requested: format!("{requested} (required by {required_by})"),
                            chain: required_by.to_string(),
                        })
                    }
                }
            }
        }
    }
}

fn resolved_ref_matches(existing: &ResolvedRef, incoming: &ResolvedRef) -> bool {
    existing.source_name == incoming.source_name
        && existing.version == incoming.version
        && existing.version_tag == incoming.version_tag
        && existing.commit == incoming.commit
        && crate::target::paths_equivalent(
            &existing.tree_path.to_string_lossy(),
            &incoming.tree_path.to_string_lossy(),
        )
}

/// High-level resolver mode shared by sync and upgrade.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveMode {
    /// Normal sync: replay compatible lock entries, otherwise pick newest compatible.
    Sync,
    /// Frozen sync: require the lock to replay exactly.
    Frozen,
    /// Upgrade: bypass lock replay for targets, leave non-targets lock-preferred.
    Upgrade {
        /// Empty means every source is an upgrade target.
        targets: HashSet<SourceName>,
        /// Treat direct target constraints as unconstrained so the manifest can be bumped.
        bump_direct_constraints: bool,
    },
}

/// Options controlling resolution behavior.
#[derive(Debug, Clone)]
pub struct ResolveOptions {
    pub mode: ResolveMode,
    /// Running mars version override. Production uses the crate version.
    pub mars_version: Option<semver::Version>,
    /// Running meridian version override. Production reads `MERIDIAN_VERSION`.
    pub meridian_version: Option<semver::Version>,
    pub ignore_requires_mars: bool,
    pub ignore_requires_meridian: bool,
    /// Local source substitutions, including names first introduced transitively.
    pub(crate) source_overrides: indexmap::IndexMap<SourceName, std::path::PathBuf>,
    pub(crate) prepared_sources:
        indexmap::IndexMap<SourceName, std::sync::Arc<mars_schema::prepare::Prepared>>,
}

impl Default for ResolveOptions {
    fn default() -> Self {
        Self {
            mode: ResolveMode::Sync,
            mars_version: None,
            meridian_version: None,
            ignore_requires_mars: false,
            ignore_requires_meridian: false,
            source_overrides: indexmap::IndexMap::new(),
            prepared_sources: indexmap::IndexMap::new(),
        }
    }
}

impl ResolveOptions {
    pub fn sync() -> Self {
        Self {
            mode: ResolveMode::Sync,
            mars_version: None,
            meridian_version: None,
            ignore_requires_mars: false,
            ignore_requires_meridian: false,
            source_overrides: indexmap::IndexMap::new(),
            prepared_sources: indexmap::IndexMap::new(),
        }
    }

    pub fn frozen() -> Self {
        Self {
            mode: ResolveMode::Frozen,
            mars_version: None,
            meridian_version: None,
            ignore_requires_mars: false,
            ignore_requires_meridian: false,
            source_overrides: indexmap::IndexMap::new(),
            prepared_sources: indexmap::IndexMap::new(),
        }
    }

    pub fn upgrade(targets: HashSet<SourceName>, bump_direct_constraints: bool) -> Self {
        Self {
            mode: ResolveMode::Upgrade {
                targets,
                bump_direct_constraints,
            },
            mars_version: None,
            meridian_version: None,
            ignore_requires_mars: false,
            ignore_requires_meridian: false,
            source_overrides: indexmap::IndexMap::new(),
            prepared_sources: indexmap::IndexMap::new(),
        }
    }

    pub(crate) fn with_source_overrides(
        mut self,
        source_overrides: indexmap::IndexMap<SourceName, std::path::PathBuf>,
    ) -> Self {
        self.source_overrides = source_overrides;
        self
    }

    pub(crate) fn with_prepared_source(
        mut self,
        source_name: SourceName,
        prepared: std::sync::Arc<mars_schema::prepare::Prepared>,
    ) -> Self {
        self.prepared_sources.insert(source_name, prepared);
        self
    }

    pub(crate) fn direct_constraint_for(
        &self,
        source_name: &SourceName,
        declared: Constraint,
    ) -> Constraint {
        if matches!(
            &self.mode,
            ResolveMode::Upgrade {
                bump_direct_constraints: true,
                ..
            }
        ) && self.is_upgrade_target(source_name)
        {
            Constraint::Latest
        } else {
            declared
        }
    }

    pub(crate) fn is_upgrade_target(&self, source_name: &SourceName) -> bool {
        match &self.mode {
            ResolveMode::Upgrade { targets, .. } => {
                targets.is_empty() || targets.contains(source_name)
            }
            ResolveMode::Sync | ResolveMode::Frozen => false,
        }
    }

    pub(crate) fn version_selection_policy(&self, source_name: &SourceName) -> SelectionPolicy {
        match &self.mode {
            ResolveMode::Frozen => SelectionPolicy::LockOnly,
            ResolveMode::Upgrade { .. } if self.is_upgrade_target(source_name) => {
                SelectionPolicy::LatestOnly
            }
            ResolveMode::Sync | ResolveMode::Upgrade { .. } => {
                SelectionPolicy::PreferLockThenLatest
            }
        }
    }
}
