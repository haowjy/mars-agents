use mars_schema::version::Constraint;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;

use indexmap::IndexMap;

use crate::config::{EffectiveDependency, Manifest};
use crate::source::ResolvedRef;
use crate::types::{SourceId, SourceName};
use mars_schema::claude::manifest::PluginSource;

/// The resolved dependency graph — all sources with concrete versions.
///
/// Produced by the resolver after fetching sources, reading manifests,
/// intersecting version constraints, and sorting the packages by name.
#[derive(Debug, Clone)]
pub struct ResolvedGraph {
    pub nodes: IndexMap<SourceName, ResolvedNode>,
    /// Alphabetical, for the order sync walks the packages in. The order skill refs resolve in is
    /// not this one; `selection` computes it.
    pub order: Vec<SourceName>,
    /// The Agents, skills and documents the consumer installs, chosen once the graph settled.
    pub selection: mars_schema::selection::SelectionResult,
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
                    if !self.nodes.contains_key(&dep) {
                        continue;
                    }
                    let candidate = current_depth + 1;
                    if depths.get(&dep).is_none_or(|depth| candidate < *depth) {
                        queue.push_back(dep.clone());
                        depths.insert(dep, candidate);
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
                if !self.nodes.contains_key(&dep) {
                    continue;
                }
                match orders.get_mut(&dep) {
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
    pub prepared: std::sync::Arc<mars_schema::prepare::Prepared>,
    pub resolved_ref: ResolvedRef,
    /// None if source has no mars.toml.
    pub manifest: Option<Manifest>,
}

impl ResolvedNode {
    /// Source names this depends on: the `[dependencies]` its prepared manifest reads, in
    /// declaration order.
    pub fn deps(&self) -> impl Iterator<Item = SourceName> + '_ {
        self.prepared
            .manifest
            .iter()
            .flat_map(|manifest| manifest.reading.dependencies.dependencies.keys())
            .map(|name| SourceName::from(name.as_str()))
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

/// How a source's version relates to the lock: the schema crate's choices, plus `--frozen`,
/// which replays the lock exactly and is the resolver's own rule, not a way of choosing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelectionPolicy {
    PreferLockThenLatest,
    LatestOnly,
    LockOnly,
}

impl SelectionPolicy {
    /// The crate's policy; `None` under `--frozen`, which never chooses.
    pub(crate) fn choosing(self) -> Option<mars_schema::version::SelectionPolicy> {
        use mars_schema::version::SelectionPolicy as Choosing;
        match self {
            Self::PreferLockThenLatest => Some(Choosing::PreferLockThenLatest),
            Self::LatestOnly => Some(Choosing::LatestOnly),
            Self::LockOnly => None,
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

/// A prepared package whose manifest depends on `deps` (path entries, unfiltered; no manifest when
/// there are none), for tests that build a `ResolvedNode` by hand.
#[cfg(test)]
pub(crate) fn prepared_with_deps(deps: &[&str]) -> std::sync::Arc<mars_schema::prepare::Prepared> {
    let manifest = (!deps.is_empty()).then(|| {
        let mut text = String::from("[package]\nname = \"p\"\nversion = \"1.0.0\"\n");
        for dep in deps {
            text.push_str(&format!("\n[dependencies.{dep}]\npath = \"/{dep}\"\n"));
        }
        mars_schema::prepare::PreparedManifest {
            source_path: Some("mars.toml".to_string()),
            output_path: "mars.toml".to_string(),
            reading: mars_schema::manifest::read(&text),
            normalized_bytes: text.into_bytes(),
            change: mars_schema::prepare::ChangeRecord::None,
        }
    });
    std::sync::Arc::new(mars_schema::prepare::Prepared {
        classification: mars_schema::prepare::Classification::Empty,
        selection: mars_schema::prepare::selection_facts(manifest.as_ref(), &[]),
        manifest,
        definitions: Vec::new(),
        payload: Vec::new(),
        diagnostics: Default::default(),
        changes: Default::default(),
    })
}
