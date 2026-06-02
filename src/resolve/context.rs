use std::collections::HashMap;

use super::filter::push_filter_constraint;
use super::{
    PackageResolutionState, PackageVersions, PendingItem, RegisteredPackage, ResolvedGraph,
    ResolvedNode, RootedSourceRef, VersionConstraint, VersionMetadata, VisitedSet,
};
use crate::config::FilterMode;
use crate::source::ResolvedRef;
use crate::types::{SourceId, SourceName};
use indexmap::IndexMap;

/// Mutable resolver state threaded through bottom-up resolution and DFS traversal.
pub struct ResolverContext {
    registry: IndexMap<SourceName, RegisteredPackage>,
    package_states: HashMap<SourceName, PackageResolutionState>,
    id_index: HashMap<SourceId, SourceName>,
    version_constraints: HashMap<SourceName, Vec<(String, VersionConstraint)>>,
    materialization_filters: HashMap<SourceName, Vec<FilterMode>>,
    stack: Vec<PendingItem>,
    visited: VisitedSet,
    package_versions: PackageVersions,
    /// Version overrides carried from a prior restart pass.
    ///
    /// When a restart is triggered because package X would resolve to a different
    /// version under the full accumulated constraint set, the driver carries the
    /// correct (new) ref into the fresh context via this map. The first-resolution
    /// branch in `resolve_package_bottom_up` checks this map and uses the override
    /// directly, so the same constraint-accumulation pattern does NOT re-trigger a
    /// restart on the next pass.
    version_overrides: HashMap<SourceName, (ResolvedRef, RootedSourceRef, VersionMetadata)>,
    /// Pending restart info set by `resolve_package_bottom_up` just before it returns
    /// `ResolutionRestartNeeded`. The driver reads this before discarding the context.
    pending_restart: Option<(SourceName, ResolvedRef, RootedSourceRef, VersionMetadata)>,
}

impl Default for ResolverContext {
    fn default() -> Self {
        Self::new()
    }
}

impl ResolverContext {
    pub fn new() -> Self {
        Self {
            registry: IndexMap::new(),
            package_states: HashMap::new(),
            id_index: HashMap::new(),
            version_constraints: HashMap::new(),
            materialization_filters: HashMap::new(),
            stack: Vec::new(),
            visited: VisitedSet::new(),
            package_versions: PackageVersions::new(),
            version_overrides: HashMap::new(),
            pending_restart: None,
        }
    }

    /// Set version overrides from a prior restart pass.
    /// These are used by `resolve_package_bottom_up` to skip re-resolution for
    /// packages where the correct version was already computed.
    pub(super) fn set_version_overrides(
        &mut self,
        overrides: HashMap<SourceName, (ResolvedRef, RootedSourceRef, VersionMetadata)>,
    ) {
        self.version_overrides = overrides;
    }

    /// Look up an override for the first resolution of `name`.
    /// Returns the pre-computed (ResolvedRef, RootedSourceRef, version metadata) if present.
    pub(super) fn version_override(
        &self,
        name: &SourceName,
    ) -> Option<&(ResolvedRef, RootedSourceRef, VersionMetadata)> {
        self.version_overrides.get(name)
    }

    /// Record the restart info: the package that triggered a restart and the ref
    /// it should be resolved to on the next pass. Called by `resolve_package_bottom_up`
    /// just before returning `ResolutionRestartNeeded`.
    pub(super) fn set_pending_restart(
        &mut self,
        package: SourceName,
        new_ref: ResolvedRef,
        new_rooted: RootedSourceRef,
        metadata: VersionMetadata,
    ) {
        self.pending_restart = Some((package, new_ref, new_rooted, metadata));
    }

    /// Drain the pending restart info. Called by the driver after catching the signal.
    pub(super) fn take_pending_restart(
        &mut self,
    ) -> Option<(SourceName, ResolvedRef, RootedSourceRef, VersionMetadata)> {
        self.pending_restart.take()
    }

    pub(super) fn registry(&self) -> &IndexMap<SourceName, RegisteredPackage> {
        &self.registry
    }

    pub(super) fn registry_mut(&mut self) -> &mut IndexMap<SourceName, RegisteredPackage> {
        &mut self.registry
    }

    pub(super) fn package_states(&self) -> &HashMap<SourceName, PackageResolutionState> {
        &self.package_states
    }

    pub(super) fn package_states_mut(
        &mut self,
    ) -> &mut HashMap<SourceName, PackageResolutionState> {
        &mut self.package_states
    }

    pub(super) fn id_index(&self) -> &HashMap<SourceId, SourceName> {
        &self.id_index
    }

    pub(super) fn id_index_mut(&mut self) -> &mut HashMap<SourceId, SourceName> {
        &mut self.id_index
    }

    pub(super) fn version_constraints(
        &self,
    ) -> &HashMap<SourceName, Vec<(String, VersionConstraint)>> {
        &self.version_constraints
    }

    pub(super) fn materialization_filters(&self) -> &HashMap<SourceName, Vec<FilterMode>> {
        &self.materialization_filters
    }

    pub(super) fn visited(&self) -> &VisitedSet {
        &self.visited
    }

    pub(super) fn visited_mut(&mut self) -> &mut VisitedSet {
        &mut self.visited
    }

    pub(super) fn package_versions_mut(&mut self) -> &mut PackageVersions {
        &mut self.package_versions
    }

    pub fn add_version_constraint(
        &mut self,
        package: &SourceName,
        requester: &str,
        constraint: VersionConstraint,
    ) {
        self.version_constraints
            .entry(package.clone())
            .or_default()
            .push((requester.to_string(), constraint));
    }

    pub fn add_filter(&mut self, package: &SourceName, filter: FilterMode) {
        push_filter_constraint(&mut self.materialization_filters, package, &filter);
    }

    pub fn push_pending(&mut self, item: PendingItem) {
        self.stack.push(item);
    }

    pub fn pop_pending(&mut self) -> Option<PendingItem> {
        self.stack.pop()
    }

    pub fn into_graph(self) -> ResolvedGraph {
        let mut nodes: IndexMap<SourceName, ResolvedNode> = IndexMap::new();
        for (name, package) in self.registry {
            nodes.insert(name, package.node);
        }

        let mut order: Vec<SourceName> = nodes.keys().cloned().collect();
        order.sort();

        ResolvedGraph {
            nodes,
            order,
            filters: self.materialization_filters,
        }
    }
}
