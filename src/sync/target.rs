use mars_schema::item::ItemKind;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use indexmap::IndexMap;

use crate::config::EffectiveConfig;
use crate::diagnostic::{DiagnosticCategory, DiagnosticCollector};
use crate::discover;
use crate::error::MarsError;
use crate::hash;
use crate::lock::ItemId;
use crate::resolve::ResolvedGraph;
use crate::sync::kept_hooks::{InvalidDependencyHook, KeptHook};
use crate::sync::ownership::Ownership;
use crate::sync::package_dir::PackageDir;
use crate::types::{ContentHash, DestPath, SourceName};
use mars_schema::names::{ItemName, RenameMap};

#[derive(Debug, Clone)]
pub struct TargetState {
    pub items: IndexMap<DestPath, TargetItem>,
    // Selection is intent, not proof of installation. Keep it when an unmanaged
    // canonical path prunes `items` so post-sync lowering can report the missing winner.
    selected_hooks: IndexMap<DestPath, SelectedHook>,
    selected_mcp: Vec<SelectedMcp>,
    // Every hook that passed the package filters and visibility, kept even when no managed
    // target takes it: plugin diagnostics and the sync's skip report need that omission.
    accepted_hooks: Vec<crate::compiler::hooks::ParsedHookItem>,
    // `(target, source name, hook name)` of a host's hook that gave way to a package's on that
    // target. The hook stays accepted for the targets it yields on nowhere else.
    yielded_hooks: HashSet<(String, String, String)>,
    // What the yield pass left out of a layer other than the project's, for the lock.
    yielded_items: Vec<crate::lock::YieldedRecord>,
    pub(super) kept_hooks: Vec<KeptHook>,
    // A dependency's MCP scan failed, so its servers are unknown: MCP records of the last
    // sync stay installed instead of being read as "wants no servers".
    pub(super) mcp_scan_failed: bool,
}

/// A hook a source offers one target, before the yield pass decides it.
struct HookClaim {
    target_name: String,
    item: TargetItem,
    parsed: crate::compiler::hooks::ParsedHookItem,
}

#[derive(Debug, Clone)]
struct SelectedHook {
    dir: PackageDir,
    item: crate::compiler::hooks::ParsedHookItem,
}

#[derive(Debug, Clone)]
struct SelectedMcp {
    dir: PackageDir,
    item: crate::compiler::mcp::ParsedMcpItem,
}

impl TargetState {
    pub(crate) fn rebase_source_root(&mut self, from: &Path, to: &Path) {
        fn rebase(path: &mut PathBuf, from: &Path, to: &Path) {
            if let Ok(relative) = path.strip_prefix(from) {
                *path = to.join(relative);
            }
        }

        for item in self.items.values_mut() {
            rebase(&mut item.source_path, from, to);
        }
        for hook in self.selected_hooks.values_mut() {
            rebase(&mut hook.item.hook_dir, from, to);
        }
        for hook in &mut self.accepted_hooks {
            rebase(&mut hook.hook_dir, from, to);
        }
        for mcp in &mut self.selected_mcp {
            rebase(&mut mcp.item.source_path, from, to);
        }
    }

    pub(crate) fn kept_hooks(&self) -> &[KeptHook] {
        &self.kept_hooks
    }

    pub(crate) fn mcp_scan_failed(&self) -> bool {
        self.mcp_scan_failed
    }

    pub(crate) fn kept_dirs(&self) -> impl Iterator<Item = &PackageDir> {
        self.kept_hooks.iter().flat_map(|hook| &hook.dirs)
    }

    pub(crate) fn accepted_hooks(&self) -> &[crate::compiler::hooks::ParsedHookItem] {
        &self.accepted_hooks
    }

    /// Whether `hook` gave way to a package's of the same name on `target`, so nothing of it is
    /// installed or lost there.
    pub(crate) fn hook_yielded(
        &self,
        hook: &crate::compiler::hooks::ParsedHookItem,
        target: &str,
    ) -> bool {
        self.yielded_hooks.contains(&(
            target.to_string(),
            hook.source_name.to_string(),
            hook.def.name.to_string(),
        ))
    }

    /// The (hook, managed target) pairs where `[hooks]` leaves something out and no copy of the
    /// hook is installed on the target. No item records them, so the lock keeps an empty
    /// one (`hook/<name>@<target>`): a change to the hook, or the target becoming new, is what
    /// makes the loss worth reporting again.
    pub(crate) fn unplaced_hook_losses<'a>(
        &'a self,
        managed_targets: &'a [String],
    ) -> impl Iterator<Item = (&'a crate::compiler::hooks::ParsedHookItem, &'a str)> {
        self.accepted_hooks
            .iter()
            .filter(|hook| hook.def.portable().is_some())
            .flat_map(move |hook| {
                managed_targets
                    .iter()
                    .map(String::as_str)
                    .filter(move |target| {
                        crate::layer::offered_to(&hook.source_name, target)
                            && !self.hook_yielded(hook, target)
                            && self.placed_hook(hook, target).is_none()
                            && !hook.def.skips_for(target).is_empty()
                    })
                    .map(move |target| (hook, target))
            })
    }

    /// The directory that installs `hook` on `target`, if any.
    pub(crate) fn placed_hook(
        &self,
        hook: &crate::compiler::hooks::ParsedHookItem,
        target: &str,
    ) -> Option<&PackageDir> {
        self.selected_hooks_for_target(target)
            .find(|(_, placed)| {
                placed.source_name == hook.source_name && placed.def.name == hook.def.name
            })
            .map(|(dir, _)| dir)
    }

    /// The one pass where a layer's content yields to a better layer's (`layer::yielded`): after
    /// collection, an agent, skill, hook or MCP server that a better-ranked layer also has on the
    /// same target is dropped, with one warning. The project's rank-1 claims are its
    /// dependencies' items, its own `.mars-src` items and, against a machine-local item, a path at
    /// that item's location the project holds (`machine_local::ProjectClaims`). Agents and skills
    /// are keyed by their name before collision renames, so a name the project renamed still
    /// blocks the user level's copy. The warning is news when the yielding source's record
    /// changes or the winner's does, so it prints once whichever came first. Nothing of a dropped
    /// item is installed, so no loss is recorded for it; `yielded_items` keeps them for `mars
    /// list` and `mars why`.
    fn select_and_yield(
        &mut self,
        collected: &mut Vec<TargetItem>,
        claims: Vec<HookClaim>,
        project: &crate::sync::machine_local::ProjectClaims<'_>,
        diag: &mut DiagnosticCollector,
    ) {
        use crate::layer::{Layer, yielded};
        use crate::sync::machine_local::ClaimKey;

        let item_key = |item: &TargetItem| -> ClaimKey {
            (
                item.id.kind,
                crate::lock::CANONICAL_TARGET_ROOT.to_string(),
                item.dest_path.item_name(item.id.kind),
            )
        };
        // Every claim with its layer; a machine-local one whose path the project holds is also
        // the project's claim, with the path for the warning.
        let mut keyed: Vec<(ClaimKey, Layer)> = project
            .names()
            .map(|key| (key.clone(), Layer::Project))
            .collect();
        let mut held_paths: HashMap<ClaimKey, String> = HashMap::new();
        let mut claim = |key: ClaimKey, source: &str, kind: ItemKind, dest: Option<DestPath>| {
            let layer = Layer::of(source);
            if layer.machine_local()
                && let Some(path) = dest.and_then(|dest| project.path_claim(kind, &dest, source))
            {
                keyed.push((key.clone(), Layer::Project));
                held_paths.entry(key.clone()).or_insert(path);
            }
            keyed.push((key, layer));
        };
        for item in collected.iter() {
            claim(
                item_key(item),
                &item.source_name,
                item.id.kind,
                Some(item.dest_path.clone()),
            );
        }
        for hook in &claims {
            claim(
                (
                    ItemKind::Hook,
                    hook.target_name.clone(),
                    hook.parsed.def.name.clone(),
                ),
                &hook.parsed.source_name,
                ItemKind::Hook,
                Some(hook.item.dest_path.clone()),
            );
        }
        for selected in &self.selected_mcp {
            claim(
                (
                    ItemKind::McpServer,
                    selected.dir.target().to_string(),
                    selected.item.name.clone(),
                ),
                &selected.item.source_name,
                ItemKind::McpServer,
                selected
                    .item
                    .uses_dir_token()
                    .then(|| selected.dir.canonical()),
            );
        }
        let dropped = yielded(keyed);
        let gives_way =
            |key: &ClaimKey, source: &str| dropped.contains(&(key.clone(), Layer::of(source)));
        let mut yielded_items = Vec::new();
        let mut warn = |diag: &mut DiagnosticCollector,
                        key: &ClaimKey,
                        source: &str,
                        what: String| {
            let why = match (held_paths.get(key), Layer::of(source)) {
                (Some(path), _) => format!("the project has its own `{path}`"),
                (None, Layer::Host) => "a package or the user level also provides it".to_string(),
                (None, _) => "the project also provides it".to_string(),
            };
            crate::sync::machine_local::leave_out(
                &mut yielded_items,
                diag,
                "provided-shadowed",
                key.clone(),
                source,
                what,
                why,
            );
        };
        collected.retain(|item| {
            let key = item_key(item);
            if !gives_way(&key, &item.source_name) {
                return true;
            }
            warn(
                diag,
                &key,
                &item.source_name,
                format!("{} `{}`", item.id.kind, item.id.name),
            );
            false
        });
        for claim in claims {
            let name = &claim.parsed.def.name;
            let target = claim.target_name.trim_start_matches('.');
            let source = &claim.parsed.source_name;
            let key = (ItemKind::Hook, claim.target_name.clone(), name.clone());
            if gives_way(&key, source) {
                self.yielded_hooks.insert((
                    claim.target_name.clone(),
                    source.to_string(),
                    name.to_string(),
                ));
                warn(diag, &key, source, format!("hook `{name}` on `.{target}`"));
                continue;
            }
            self.select_hook(&claim.target_name, &claim.item, claim.parsed);
            collected.push(claim.item);
        }
        let (yielded_mcp, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.selected_mcp)
            .into_iter()
            .partition(|selected| {
                gives_way(
                    &(
                        ItemKind::McpServer,
                        selected.dir.target().to_string(),
                        selected.item.name.clone(),
                    ),
                    &selected.item.source_name,
                )
            });
        self.selected_mcp = kept;
        for selected in yielded_mcp {
            let target = selected.dir.target();
            warn(
                diag,
                &(
                    ItemKind::McpServer,
                    target.to_string(),
                    selected.item.name.clone(),
                ),
                &selected.item.source_name,
                format!("MCP server `{}` on `{target}`", selected.item.name),
            );
        }
        self.yielded_items = yielded_items;
    }

    /// What the claim rule and `machine_local::decide` left out, with why, for the lock.
    pub(crate) fn yielded_items(&self) -> &[crate::lock::YieldedRecord] {
        &self.yielded_items
    }

    pub(crate) fn record_yielded(&mut self, records: Vec<crate::lock::YieldedRecord>) {
        self.yielded_items.extend(records);
    }

    pub(crate) fn accept_hook(&mut self, hook: &crate::compiler::hooks::ParsedHookItem) {
        self.accepted_hooks.push(hook.clone());
    }

    pub(crate) fn selected_hooks(
        &self,
    ) -> impl Iterator<Item = (&PackageDir, &crate::compiler::hooks::ParsedHookItem)> {
        self.selected_hooks
            .values()
            .map(|hook| (&hook.dir, &hook.item))
    }

    pub(crate) fn selected_hooks_for_target(
        &self,
        target_root: &str,
    ) -> impl Iterator<Item = (&PackageDir, &crate::compiler::hooks::ParsedHookItem)> {
        self.selected_hooks
            .values()
            .filter(move |hook| hook.dir.belongs_to(target_root))
            .map(|hook| (&hook.dir, &hook.item))
    }

    pub(crate) fn select_hook(
        &mut self,
        target_root: &str,
        target_item: &TargetItem,
        parsed: crate::compiler::hooks::ParsedHookItem,
    ) {
        debug_assert_eq!(target_item.id.kind, ItemKind::Hook);
        debug_assert_eq!(target_item.source_name.as_str(), parsed.source_name);
        let dir = PackageDir::new(ItemKind::Hook, target_root, &parsed.def.name)
            .expect("a hook installs one directory per target");
        self.selected_hooks.insert(
            target_item.dest_path.clone(),
            SelectedHook { dir, item: parsed },
        );
    }

    /// Every selected server, one record per receiving target. Callers that
    /// report something about the server itself must deduplicate.
    pub(crate) fn selected_mcp(
        &self,
    ) -> impl Iterator<Item = (&PackageDir, &crate::compiler::mcp::ParsedMcpItem)> {
        self.selected_mcp
            .iter()
            .map(|selected| (&selected.dir, &selected.item))
    }

    pub(crate) fn selected_mcp_targets(&self) -> impl Iterator<Item = &str> {
        self.selected_mcp
            .iter()
            .map(|selected| selected.dir.target())
    }

    pub(crate) fn selected_mcp_for_target(
        &self,
        target_root: &str,
    ) -> impl Iterator<Item = (&PackageDir, &crate::compiler::mcp::ParsedMcpItem)> {
        self.selected_mcp
            .iter()
            .filter(move |selected| selected.dir.belongs_to(target_root))
            .map(|selected| (&selected.dir, &selected.item))
    }

    pub(crate) fn resolve_mcp_collisions(
        &mut self,
        managed_targets: &[String],
        local_source: &str,
        diag: &mut DiagnosticCollector,
    ) -> Result<(), MarsError> {
        use crate::compiler::config_entries::resolve::resolve_mcp_collisions_for_target;

        let candidates = std::mem::take(&mut self.selected_mcp);
        let mut resolved = Vec::with_capacity(candidates.len());
        let mut source_hashes: HashMap<PathBuf, ContentHash> = HashMap::new();
        for target_root in managed_targets {
            let winners = resolve_mcp_collisions_for_target(
                candidates
                    .iter()
                    .filter(|selected| selected.dir.belongs_to(target_root))
                    .map(|selected| &selected.item),
                target_root,
                local_source,
                diag,
            );
            for winner in winners {
                let dir = PackageDir::new(ItemKind::McpServer, target_root, &winner.name)
                    .expect("an MCP server installs one directory per target");
                if winner.uses_dir_token() {
                    let source_hash = match source_hashes.get(winner.server_dir()) {
                        Some(hash) => hash.clone(),
                        None => {
                            let hash = ContentHash::from(hash::compute_hash(
                                winner.server_dir(),
                                ItemKind::McpServer,
                            )?);
                            source_hashes.insert(winner.server_dir().to_path_buf(), hash.clone());
                            hash
                        }
                    };
                    let source_name = SourceName::from(winner.source_name.clone());
                    let item = package_dir_target_item(
                        &dir,
                        winner.server_dir(),
                        &source_name,
                        &source_hash,
                    );
                    self.items.insert(dir.canonical(), item);
                }
                resolved.push(SelectedMcp {
                    dir,
                    item: winner.clone(),
                });
            }
        }
        self.selected_mcp = resolved;
        Ok(())
    }

    pub(crate) fn select_mcp(
        &mut self,
        item: crate::compiler::mcp::ParsedMcpItem,
        targets: &[String],
    ) {
        for target_root in targets {
            if !item.def.targets.is_empty() && !item.def.targets.contains(target_root) {
                continue;
            }
            let dir = PackageDir::new(ItemKind::McpServer, target_root, &item.name)
                .expect("an MCP server installs one directory per target");
            self.selected_mcp.push(SelectedMcp {
                dir,
                item: item.clone(),
            });
        }
    }

    #[cfg(test)]
    pub(crate) fn from_items(items: IndexMap<DestPath, TargetItem>) -> Self {
        Self {
            items,
            selected_hooks: IndexMap::new(),
            selected_mcp: Vec::new(),
            accepted_hooks: Vec::new(),
            yielded_hooks: HashSet::new(),
            yielded_items: Vec::new(),
            kept_hooks: Vec::new(),
            mcp_scan_failed: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TargetItem {
    pub id: ItemId,
    pub source_name: SourceName,
    pub source_path: PathBuf,
    pub dest_path: DestPath,
    pub source_hash: ContentHash,
    pub is_flat_skill: bool,
    pub rewritten_content: Option<String>,
}

impl TargetItem {
    pub(crate) fn expected_installed_hash(&self) -> Result<ContentHash, MarsError> {
        let Some(content) = &self.rewritten_content else {
            return Ok(self.source_hash.clone());
        };
        let hash = match self.id.kind {
            ItemKind::Skill => crate::hash::compute_skill_hash_with_override(
                &self.source_path,
                "SKILL.md",
                content.as_bytes(),
                if self.is_flat_skill {
                    crate::fs::FLAT_SKILL_EXCLUDED_TOP_LEVEL
                } else {
                    &[]
                },
            )?,
            _ => crate::hash::hash_bytes(content.as_bytes()),
        };
        Ok(ContentHash::from(hash))
    }
}

#[derive(Debug, Clone)]
pub struct ExplicitRename {
    pub original_name: ItemName,
    pub new_name: ItemName,
    pub source_name: SourceName,
    pub kind: ItemKind,
}

#[derive(Debug, Clone)]
pub struct CollisionRename {
    pub original_name: ItemName,
    pub new_name: ItemName,
    pub source_name: SourceName,
    pub kind: ItemKind,
}

pub(crate) type CollectedTarget = (
    TargetState,
    Vec<ExplicitRename>,
    Vec<CollisionRename>,
    Vec<InvalidDependencyHook>,
);

/// `local_source` names the consumer's own package. It keeps its installed name
/// when a dependency lands on the same destination; sync's local package is
/// `_self`, a plugin build's is its root package.
pub(crate) fn build_with_collisions_and_diag(
    graph: &ResolvedGraph,
    config: &EffectiveConfig,
    local_source: &SourceName,
    project: &crate::sync::machine_local::ProjectClaims<'_>,
    diag: &mut DiagnosticCollector,
) -> Result<CollectedTarget, MarsError> {
    let mut collected_items = Vec::new();
    let mut hook_claims = Vec::new();
    let mut invalid_hooks = Vec::new();
    let mut target = TargetState {
        items: IndexMap::new(),
        selected_hooks: IndexMap::new(),
        selected_mcp: Vec::new(),
        accepted_hooks: Vec::new(),
        yielded_hooks: HashSet::new(),
        yielded_items: Vec::new(),
        kept_hooks: Vec::new(),
        mcp_scan_failed: false,
    };
    let mut explicit_renames = Vec::new();
    let managed_target_roots = crate::config::targets::managed_targets(&config.settings);
    let dependency_depths = graph.dependency_depths(&config.dependencies);
    let decl_orders = graph.dependency_decl_orders(&config.dependencies);

    let selected: HashSet<(&str, ItemKind, &str)> = graph
        .selection
        .items
        .iter()
        .filter_map(|item| Some((item.package.as_deref()?, item.kind, item.slug.as_str())))
        .collect();

    for source_name in &graph.order {
        let node = &graph.nodes[source_name];
        let source_config = config.dependencies.get(source_name);
        let offered_targets: Vec<String> = managed_target_roots
            .iter()
            .filter(|target| crate::layer::offered_to(source_name.as_str(), target))
            .cloned()
            .collect();
        let offered_mcp_targets = mcp_config_targets(&offered_targets);

        // Removed hook schemas are a compatibility error even when dependency
        // filters select no hook. Discovery also supplies the hooks for filters,
        // avoiding a second hook.toml parse. Any other invalid `hook.toml` leaves
        // its hook out and the sync going: see `kept_hooks::settle`.
        let package_depth = dependency_depths.get(source_name).copied().unwrap_or(1);
        let decl_order = decl_orders
            .get(source_name)
            .copied()
            .unwrap_or_else(|| config.dependencies.len() + graph.order.len() + 1);
        let hook_results = crate::compiler::hooks::discover_resolved_hook_items(
            node,
            source_name,
            package_depth,
            decl_order,
            &offered_targets,
            diag,
        )?;
        let mut parsed_hooks = Vec::new();
        for result in hook_results {
            match result {
                Ok(hook) => parsed_hooks.push(hook),
                // A hook the consumer filtered out or is not shown is not theirs to hear about,
                // and keeps nothing.
                Err(invalid)
                    if invalid.exported != Some(false)
                        && selected.contains(&(
                            source_name.as_str(),
                            ItemKind::Hook,
                            invalid.slug.as_str(),
                        )) =>
                {
                    invalid_hooks.push(InvalidDependencyHook {
                        source_name: source_name.clone(),
                        source_path: crate::sync::kept_hooks::lock_source_path(
                            &node.rooted_ref.package_root,
                            &invalid.hook_dir,
                        ),
                        installed_names: invalid.installed_names,
                        visibility_unread: invalid.exported.is_none(),
                        rank: crate::compiler::hooks::HookRank {
                            package_depth,
                            decl_order,
                            order: invalid.order,
                        },
                        message: crate::compiler::hooks::error_message(invalid.error)
                            .trim_end()
                            .to_string(),
                    })
                }
                Err(_) => {}
            }
        }
        let discovered_hooks = parsed_hooks
            .iter()
            .map(|hook| hook.discovered_item(&node.rooted_ref.package_root))
            .collect::<Vec<_>>();
        let discovered = discover::package_items(
            &node.rooted_ref.package_root,
            Some(source_name.as_str()),
            &node.prepared,
            Some(&discovered_hooks),
            diag,
        )?;

        // Filters select agents, skills and bootstrap docs; a dependency's MCP
        // servers stay visible to a consumer that includes nothing else.
        match crate::compiler::mcp::discover_mcp_items(
            &node.rooted_ref.package_root,
            source_name.as_str(),
            dependency_depths.get(source_name).copied().unwrap_or(1),
            decl_order,
            diag,
        ) {
            Ok(items) => {
                // Depth 1 is a dependency this consumer declares itself; `local`
                // servers reach only those, `exported` ones reach every depth.
                let is_direct = dependency_depths.get(source_name) == Some(&1);
                for item in items {
                    if is_direct || item.visibility == mars_schema::visibility::Visibility::Exported
                    {
                        target.select_mcp(item, &offered_mcp_targets);
                    }
                }
            }
            Err(error) => {
                target.mcp_scan_failed = true;
                diag.warn(
                    "mcp-discover",
                    format!(
                        "failed to scan MCP items in `{}`: {error}; \
                         MCP servers installed by the last sync stay as they are",
                        crate::layer::source_label(source_name)
                    ),
                );
            }
        }

        let filtered = discovered.iter().filter(|item| {
            selected.contains(&(source_name.as_str(), item.id.kind, item.id.name.as_str()))
        });

        let renames = source_config
            .map(|s| &s.rename)
            .cloned()
            .unwrap_or_default();

        for item in filtered {
            if item.id.kind == ItemKind::Hook {
                let hook_dir = node.rooted_ref.package_root.join(&item.source_path);
                let parsed = parsed_hooks
                    .iter()
                    .find(|hook| hook.hook_dir == hook_dir)
                    .expect("discovered hook directory has a parsed definition");
                if parsed.def.visibility != mars_schema::visibility::Visibility::Exported {
                    continue;
                }
                target.accepted_hooks.push(parsed.clone());
                for (target_name, _dest_path, target_item) in
                    hook_target_items(parsed, source_name, &offered_targets)?
                {
                    hook_claims.push(HookClaim {
                        target_name,
                        item: target_item,
                        parsed: parsed.clone(),
                    });
                }
                continue;
            }
            let is_flat_skill =
                item.id.kind == ItemKind::Skill && item.source_path == Path::new(".");
            let source_content_path = node.rooted_ref.package_root.join(&item.source_path);
            let source_hash = if is_flat_skill {
                ContentHash::from(hash::compute_skill_hash_filtered(
                    &source_content_path,
                    crate::fs::FLAT_SKILL_EXCLUDED_TOP_LEVEL,
                )?)
            } else {
                ContentHash::from(hash::compute_hash(&source_content_path, item.id.kind)?)
            };

            let rename_kind = RenamableKind::of(item.id.kind)
                .expect("hooks and MCP servers install through PackageDir, not a rename");
            let (dest_name, dest_path) =
                apply_item_rename(rename_kind, &item.id.name, &renames, source_name)?;
            if item.id.kind == ItemKind::Agent
                && let Err(message) = crate::target::validate_agent_filename(dest_name.as_str())
            {
                diag.error_with_category(
                    "invalid-agent-filename",
                    format!("{message}; skipping agent from source `{source_name}`"),
                    DiagnosticCategory::Validation,
                );
                continue;
            }
            if matches!(item.id.kind, ItemKind::Agent | ItemKind::Skill)
                && dest_name != item.id.name
            {
                explicit_renames.push(ExplicitRename {
                    original_name: item.id.name.clone(),
                    new_name: dest_name.clone(),
                    source_name: source_name.clone(),
                    kind: item.id.kind,
                });
            }

            let target_item = TargetItem {
                id: ItemId {
                    kind: item.id.kind,
                    name: dest_name,
                },
                source_name: source_name.clone(),
                source_path: source_content_path,
                dest_path,
                source_hash,
                is_flat_skill,
                rewritten_content: None,
            };

            collected_items.push(target_item);
        }
    }

    target.select_and_yield(&mut collected_items, hook_claims, project, diag);

    let collision_renames =
        rename_destination_collisions(&mut collected_items, local_source, diag)?;

    let mut items: IndexMap<DestPath, TargetItem> = IndexMap::new();
    for target_item in collected_items {
        if let Some(existing) = items.get(&target_item.dest_path) {
            return Err(MarsError::Collision {
                item: format!(
                    "{} `{}` at `{}` after auto-rename, which writes a `/` in a dependency name as `-` (name one of the two dependencies differently in mars.toml)",
                    target_item.id.kind, target_item.id.name, target_item.dest_path
                ),
                source_a: existing.source_name.to_string(),
                source_b: target_item.source_name.to_string(),
            });
        }

        items.insert(target_item.dest_path.clone(), target_item);
    }

    target.items = items;

    Ok((target, explicit_renames, collision_renames, invalid_hooks))
}

pub(crate) fn mcp_config_targets(managed_targets: &[String]) -> Vec<String> {
    managed_targets
        .iter()
        .filter(|target| crate::target::spec(target).is_some_and(|spec| spec.mcp.is_some()))
        .cloned()
        .collect()
}

pub(crate) fn hook_checksum(
    parsed: &crate::compiler::hooks::ParsedHookItem,
) -> Result<ContentHash, MarsError> {
    Ok(ContentHash::from(hash::compute_hash(
        &parsed.hook_dir,
        ItemKind::Hook,
    )?))
}

pub(crate) fn hook_target_items(
    parsed: &crate::compiler::hooks::ParsedHookItem,
    source_name: &SourceName,
    managed_targets: &[String],
) -> Result<Vec<(String, DestPath, TargetItem)>, MarsError> {
    let target_roots = parsed
        .def
        .select(managed_targets)
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let source_hash = hook_checksum(parsed)?;
    Ok(target_roots
        .into_iter()
        .map(|target_root| {
            let dir = PackageDir::new(ItemKind::Hook, &target_root, &parsed.def.name)
                .expect("a hook installs one directory per target");
            let dest_path = dir.canonical();
            let target_item =
                package_dir_target_item(&dir, &parsed.hook_dir, source_name, &source_hash);
            (target_root, dest_path, target_item)
        })
        .collect())
}

pub(crate) fn package_dir_target_item(
    dir: &PackageDir,
    source_dir: &Path,
    source_name: &SourceName,
    source_hash: &ContentHash,
) -> TargetItem {
    TargetItem {
        id: ItemId {
            kind: dir.kind(),
            name: dir.lock_name().into(),
        },
        source_name: source_name.clone(),
        source_path: source_dir.to_path_buf(),
        dest_path: dir.canonical(),
        source_hash: source_hash.clone(),
        is_flat_skill: false,
        rewritten_content: None,
    }
}

fn rename_destination_collisions(
    items: &mut [TargetItem],
    local_source: &SourceName,
    diag: &mut DiagnosticCollector,
) -> Result<Vec<CollisionRename>, MarsError> {
    let mut groups: IndexMap<DestPath, Vec<usize>> = IndexMap::new();
    for (index, item) in items.iter().enumerate() {
        groups
            .entry(item.dest_path.clone())
            .or_default()
            .push(index);
    }

    let mut renames = Vec::new();
    for indices in groups.values().filter(|indices| indices.len() > 1) {
        let first = &items[indices[0]];
        let distinct_sources: HashSet<&SourceName> = indices
            .iter()
            .map(|&index| &items[index].source_name)
            .collect();
        let auto_renamable = matches!(first.id.kind, ItemKind::Agent | ItemKind::Skill)
            && indices
                .iter()
                .all(|&index| items[index].id.kind == first.id.kind)
            && distinct_sources.len() == indices.len();
        if !auto_renamable {
            let second = &items[indices[1]];
            return Err(MarsError::Collision {
                item: format!("{} `{}`", second.id.kind, second.id.name),
                source_a: first.source_name.to_string(),
                source_b: second.source_name.to_string(),
            });
        }
        let kind = RenamableKind::of(first.id.kind)
            .expect("an auto-renamable collision is an agent or a skill");

        for &index in indices {
            let item = &mut items[index];
            // The consumer's own package keeps the destination it declares; every
            // dependency that landed on it takes the suffix instead.
            // So does the user level's (`_user-self`): a collision here is within its layer,
            // since one across layers has yielded before this.
            if item.source_name == *local_source || item.source_name == crate::layer::USER_SELF {
                continue;
            }
            let original_name = item.id.name.clone();
            let new_dest_path =
                suffixed_collision_dest_path(&item.dest_path, kind, &item.source_name)?;
            let new_name = ItemName::from(new_dest_path.item_name(kind.item_kind()));

            item.id.name = new_name.clone();
            diag.hold_warning(
                crate::lock::RecordRef::Item(crate::lock::item_key(&item.id)),
                "auto-rename-collision",
                format!(
                    "auto-renamed {} `{}` from source `{}` → `{}`",
                    item.id.kind, original_name, item.source_name, new_name
                ),
            );
            item.dest_path = new_dest_path;
            renames.push(CollisionRename {
                original_name,
                new_name,
                source_name: item.source_name.clone(),
                kind: item.id.kind,
            });
        }
    }

    Ok(renames)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnmanagedCollision {
    pub source_name: SourceName,
    pub path: DestPath,
}

pub(crate) fn check_unmanaged_collisions(
    install_target: &Path,
    owns: &Ownership,
    target: &TargetState,
    force: bool,
) -> Vec<UnmanagedCollision> {
    target
        .items
        .values()
        .filter(|target_item| unmanaged_collision(install_target, owns, target_item, force))
        .map(|target_item| UnmanagedCollision {
            source_name: target_item.source_name.clone(),
            path: target_item.dest_path.clone(),
        })
        .collect()
}

fn unmanaged_collision(
    install_target: &Path,
    owns: &Ownership,
    target_item: &TargetItem,
    force: bool,
) -> bool {
    if owns.owns(&target_item.dest_path) {
        return false;
    }

    let disk_path = target_item.dest_path.resolve(install_target);
    if !disk_path.exists() || force {
        return false;
    }
    // Disk content matching what we'd install is a partial prior install (crash
    // recovery), not an unmanaged user file. Safe to overwrite.
    let matches_install = hash::compute_hash(&disk_path, target_item.id.kind).is_ok_and(|hash| {
        target_item
            .expected_installed_hash()
            .is_ok_and(|expected| hash == expected.as_str())
    });
    !matches_install
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RenamableKind {
    Agent,
    Skill,
    BootstrapDoc,
}

impl RenamableKind {
    pub(crate) fn of(kind: ItemKind) -> Option<Self> {
        match kind {
            ItemKind::Agent => Some(Self::Agent),
            ItemKind::Skill => Some(Self::Skill),
            ItemKind::BootstrapDoc => Some(Self::BootstrapDoc),
            ItemKind::Hook | ItemKind::McpServer => None,
        }
    }

    fn item_kind(self) -> ItemKind {
        match self {
            Self::Agent => ItemKind::Agent,
            Self::Skill => ItemKind::Skill,
            Self::BootstrapDoc => ItemKind::BootstrapDoc,
        }
    }
}

fn apply_item_rename(
    kind: RenamableKind,
    item_name: &str,
    renames: &RenameMap,
    source_name: &SourceName,
) -> Result<(ItemName, DestPath), MarsError> {
    let default_dest = default_dest_path(kind, item_name);
    let rename_value = renames
        .get(default_dest.as_str())
        .or_else(|| renames.get(item_name));

    let dest_path = match rename_value {
        Some(value) => parse_rename_dest(kind, value.as_str(), source_name)?,
        None => default_dest,
    };

    Ok((
        ItemName::from(dest_path.item_name(kind.item_kind())),
        dest_path,
    ))
}

/// Uses string formatting to guarantee forward slashes on all platforms.
pub(crate) fn default_dest_path(kind: RenamableKind, name: &str) -> DestPath {
    let path_str = match kind {
        RenamableKind::Agent => format!("agents/{name}.md"),
        RenamableKind::Skill => format!("skills/{name}"),
        RenamableKind::BootstrapDoc => format!("bootstrap/{name}/BOOTSTRAP.md"),
    };
    DestPath::new(path_str).expect("internal default path is always valid")
}

fn parse_rename_dest(
    kind: RenamableKind,
    rename_value: &str,
    source_name: &SourceName,
) -> Result<DestPath, MarsError> {
    let normalized = rename_value.replace('\\', "/");
    let has_prefix = normalized.starts_with("agents/")
        || normalized.starts_with("skills/")
        || normalized.starts_with("hooks/")
        || normalized.starts_with("mcp/")
        || normalized.starts_with("bootstrap/");
    let has_parent = normalized.contains('/');

    if has_prefix || has_parent {
        let dest = if kind == RenamableKind::BootstrapDoc && !normalized.ends_with("/BOOTSTRAP.md")
        {
            format!("{normalized}/BOOTSTRAP.md")
        } else {
            normalized.clone()
        };
        return DestPath::new(&dest).map_err(|e| MarsError::Source {
            source_name: source_name.to_string(),
            message: format!("invalid rename destination `{rename_value}`: {e}"),
        });
    }

    let path_str = match kind {
        RenamableKind::Agent => {
            if normalized.ends_with(".md") {
                format!("agents/{normalized}")
            } else {
                format!("agents/{normalized}.md")
            }
        }
        RenamableKind::Skill => format!("skills/{normalized}"),
        RenamableKind::BootstrapDoc => format!("bootstrap/{normalized}/BOOTSTRAP.md"),
    };
    DestPath::new(path_str).map_err(|e| MarsError::Source {
        source_name: source_name.to_string(),
        message: format!("invalid rename destination `{rename_value}`: {e}"),
    })
}

/// The `__<source>` suffix sync gives a dependency's item renamed on a collision. Readers that
/// recognise a rename from the lock (`content.rs`) use it too, so the two cannot drift.
pub(crate) fn collision_suffix(source_name: &SourceName) -> String {
    // A `--plugin` dependency is named `<repo>/<plugin>`; a `/` in the suffix would nest the
    // skill one directory too deep for any harness to load it.
    format!("__{}", source_name.as_str().replace(['/', '\\'], "-"))
}

fn suffixed_collision_dest_path(
    dest_path: &DestPath,
    kind: RenamableKind,
    source_name: &SourceName,
) -> Result<DestPath, MarsError> {
    let suffix = collision_suffix(source_name);
    let path = dest_path.as_str();
    let renamed = match kind {
        RenamableKind::Agent => {
            let (parent, leaf) = split_parent_leaf(path);
            let stem = leaf.strip_suffix(".md").unwrap_or(leaf);
            mars_schema::discover::join(parent, &format!("{stem}{suffix}.md"))
        }
        RenamableKind::Skill => {
            let (parent, leaf) = split_parent_leaf(path);
            mars_schema::discover::join(parent, &format!("{leaf}{suffix}"))
        }
        RenamableKind::BootstrapDoc => {
            unreachable!("only agent and skill collisions are auto-renamed")
        }
    };

    DestPath::new(&renamed).map_err(|e| MarsError::Source {
        source_name: source_name.to_string(),
        message: format!("invalid auto-renamed destination `{renamed}`: {e}"),
    })
}

fn split_parent_leaf(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

#[cfg(test)]
mod tests;
