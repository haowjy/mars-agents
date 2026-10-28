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
use crate::sync::ownership::Ownership;
use crate::sync::package_dir::PackageDir;
use crate::types::{ContentHash, DestPath, SourceName};
use mars_schema::names::{ItemName, RenameMap};

/// What the `.mars/` canonical store should look like after sync.
///
/// Built from the resolved graph's selection.
#[derive(Debug, Clone)]
pub struct TargetState {
    /// Keyed by dest_path (relative to `.mars/`).
    pub items: IndexMap<DestPath, TargetItem>,
    // Selection is intent, not proof of installation. Keep it when an unmanaged
    // canonical path prunes `items` so post-sync lowering can report the missing winner.
    selected_hooks: IndexMap<DestPath, SelectedHook>,
    selected_mcp: Vec<SelectedMcp>,
    // Keep exported graph hooks even when none of the managed targets select
    // one of their fragments. Plugin diagnostics need to report that omission.
    exported_hooks: Vec<crate::compiler::hooks::ParsedHookItem>,
    // Dependency hooks whose `hook.toml` failed validation, until `settle_invalid_hooks`.
    invalid_hooks: Vec<InvalidDependencyHook>,
    // Dependency hooks standing on what the last lock installed.
    kept_hooks: Vec<KeptHook>,
}

#[derive(Debug, Clone)]
struct InvalidDependencyHook {
    source_name: SourceName,
    name: String,
    message: String,
}

/// A dependency hook whose `hook.toml` no longer validates. It keeps what the previous lock
/// installed: its per-target directories (never copied from the invalid source), and,
/// downstream, its merge entries and file outputs. See `compiler::config_entries`.
#[derive(Debug, Clone)]
pub(crate) struct KeptHook {
    pub name: String,
    pub dirs: Vec<PackageDir>,
}

#[derive(Debug, Clone)]
struct SelectedHook {
    dir: PackageDir,
    item: crate::compiler::hooks::ParsedHookItem,
}

/// One target's winning MCP server. A server that names its own directory also
/// has a target-scoped directory item in `TargetState::items`.
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
        for hook in &mut self.exported_hooks {
            rebase(&mut hook.hook_dir, from, to);
        }
        for mcp in &mut self.selected_mcp {
            rebase(&mut mcp.item.source_path, from, to);
        }
    }

    /// Decide what each invalid dependency hook keeps from `old_lock` and say so; `None`
    /// (a build with no previous install) keeps nothing.
    pub(crate) fn settle_invalid_hooks(
        &mut self,
        old_lock: Option<&crate::lock::LockFile>,
        managed_targets: &[String],
        diag: &mut DiagnosticCollector,
    ) {
        let lock_index = old_lock.map(crate::lock::LockIndex::new);
        for invalid in std::mem::take(&mut self.invalid_hooks) {
            let dirs: Vec<PackageDir> = managed_targets
                .iter()
                .filter_map(|target| PackageDir::new(ItemKind::Hook, target, &invalid.name))
                .filter(|dir| {
                    let dest = dir.canonical();
                    !self.items.contains_key(&dest)
                        && lock_index
                            .as_ref()
                            .and_then(|index| {
                                index.find_output(crate::lock::CANONICAL_TARGET_ROOT, &dest)
                            })
                            .is_some_and(|locked| locked.source == invalid.source_name)
                })
                .collect();
            let outcome = if dirs.is_empty() {
                "it was never installed, so nothing is installed"
            } else {
                "keeping what the last sync installed"
            };
            diag.warn(
                "hook-invalid-kept",
                format!("{}; {outcome}", invalid.message),
            );
            if !dirs.is_empty() {
                self.kept_hooks.push(KeptHook {
                    name: invalid.name,
                    dirs,
                });
            }
        }
    }

    pub(crate) fn kept_hooks(&self) -> &[KeptHook] {
        &self.kept_hooks
    }

    pub(crate) fn kept_dirs(&self) -> impl Iterator<Item = &PackageDir> {
        self.kept_hooks.iter().flat_map(|hook| &hook.dirs)
    }

    pub(crate) fn exported_hooks(&self) -> &[crate::compiler::hooks::ParsedHookItem] {
        &self.exported_hooks
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

    /// Resolve MCP name collisions once per target, before installation and
    /// config lowering. Each surviving server carries its directory item with
    /// it, so the files, config entry, lock ownership and pruning all name the
    /// same winner.
    pub(crate) fn resolve_mcp_collisions(
        &mut self,
        managed_targets: &[String],
        local_source: &str,
        diag: &mut DiagnosticCollector,
    ) -> Result<(), MarsError> {
        use crate::compiler::config_entries::resolve::resolve_mcp_collisions_for_target;

        let candidates = std::mem::take(&mut self.selected_mcp);
        let mut resolved = Vec::with_capacity(candidates.len());
        // One source directory can win on several targets; hash it once.
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

    /// Select a server for the managed targets that write MCP config;
    /// `targets` must already be limited to those.
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
            exported_hooks: Vec::new(),
            invalid_hooks: Vec::new(),
            kept_hooks: Vec::new(),
        }
    }
}

/// A single item in the desired target state.
#[derive(Debug, Clone)]
pub struct TargetItem {
    pub id: ItemId,
    pub source_name: SourceName,
    /// Path to content in fetched source tree.
    pub source_path: PathBuf,
    /// Relative path under `.mars/` (reflects rename if any).
    pub dest_path: DestPath,
    /// SHA-256 of source content.
    pub source_hash: ContentHash,
    /// True when this item comes from root-level `SKILL.md` flat skill discovery.
    pub is_flat_skill: bool,
    /// Optional in-memory content override after frontmatter rewrites.
    pub rewritten_content: Option<String>,
}

impl TargetItem {
    /// Hash of the bytes/tree mars intends to install after in-memory rewrites.
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

/// Explicit skill rename that changes the installed skill name.
#[derive(Debug, Clone)]
pub struct ExplicitRename {
    pub original_name: ItemName,
    pub new_name: ItemName,
    pub source_name: SourceName,
    pub kind: ItemKind,
}

/// Automatic rename applied when multiple sources target the same destination.
#[derive(Debug, Clone)]
pub struct CollisionRename {
    pub original_name: ItemName,
    pub new_name: ItemName,
    pub source_name: SourceName,
    pub kind: ItemKind,
}
/// `local_source` names the consumer's own package. It keeps its installed name
/// when a dependency lands on the same destination; sync's local package is
/// `_self`, a plugin build's is its root package.
pub fn build_with_collisions_and_diag(
    graph: &ResolvedGraph,
    config: &EffectiveConfig,
    local_source: &SourceName,
    diag: &mut DiagnosticCollector,
) -> Result<(TargetState, Vec<ExplicitRename>, Vec<CollisionRename>), MarsError> {
    let mut collected_items = Vec::new();
    let mut target = TargetState {
        items: IndexMap::new(),
        selected_hooks: IndexMap::new(),
        selected_mcp: Vec::new(),
        exported_hooks: Vec::new(),
        invalid_hooks: Vec::new(),
        kept_hooks: Vec::new(),
    };
    let mut explicit_renames = Vec::new();
    let managed_target_roots = config.settings.managed_targets();
    let mcp_target_roots = mcp_config_targets(&managed_target_roots);
    // Hooks and MCP share shortest graph distance as their precedence key.
    // Keeping one definition avoids a second meaning of dependency depth.
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

        // Removed hook schemas are a compatibility error even when dependency
        // filters select no hook. Discovery also supplies the hooks for filters,
        // avoiding a second hook.toml parse. Any other invalid `hook.toml` leaves
        // its hook out and the sync going: see `settle_invalid_hooks`.
        let hook_results = crate::compiler::hooks::discover_resolved_hook_items(
            node,
            source_name,
            dependency_depths.get(source_name).copied().unwrap_or(1),
            decl_orders
                .get(source_name)
                .copied()
                .unwrap_or_else(|| config.dependencies.len() + graph.order.len() + 1),
            diag,
        )?;
        let mut parsed_hooks = Vec::new();
        for result in hook_results {
            match result {
                Ok(hook) => parsed_hooks.push(hook),
                Err(invalid) => target.invalid_hooks.push(InvalidDependencyHook {
                    source_name: source_name.clone(),
                    name: invalid.name,
                    message: crate::compiler::hooks::error_message(invalid.error)
                        .trim_end()
                        .to_string(),
                }),
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
        let decl_order = decl_orders
            .get(source_name)
            .copied()
            .unwrap_or_else(|| config.dependencies.len() + graph.order.len() + 1);
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
                        target.select_mcp(item, &mcp_target_roots);
                    }
                }
            }
            Err(error) => diag.warn(
                "mcp-discover",
                format!("failed to scan MCP items in `{source_name}`: {error}"),
            ),
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
                target.exported_hooks.push(parsed.clone());
                for (target_name, _dest_path, target_item) in
                    hook_target_items(parsed, source_name, &managed_target_roots)?
                {
                    target.select_hook(&target_name, &target_item, parsed.clone());
                    collected_items.push(target_item);
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

    let collision_renames =
        rename_destination_collisions(&mut collected_items, local_source, diag)?;

    let mut items: IndexMap<DestPath, TargetItem> = IndexMap::new();
    for target_item in collected_items {
        if let Some(existing) = items.get(&target_item.dest_path) {
            return Err(MarsError::Collision {
                item: format!(
                    "{} `{}` at `{}` after auto-rename",
                    target_item.id.kind, target_item.id.name, target_item.dest_path
                ),
                source_a: existing.source_name.to_string(),
                source_b: target_item.source_name.to_string(),
            });
        }

        items.insert(target_item.dest_path.clone(), target_item);
    }

    target.items = items;

    Ok((target, explicit_renames, collision_renames))
}

/// Managed targets whose adapter writes an MCP config file. Only these get a
/// server's directory or config entry.
pub(crate) fn mcp_config_targets(managed_targets: &[String]) -> Vec<String> {
    let registry = crate::target::TargetRegistry::new();
    managed_targets
        .iter()
        .filter(|target| {
            registry
                .get(target)
                .is_some_and(|adapter| adapter.mcp_file().is_some())
        })
        .cloned()
        .collect()
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
    let source_hash = ContentHash::from(hash::compute_hash(&parsed.hook_dir, ItemKind::Hook)?);
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

/// One target's copy of a package-owned directory. Hooks and MCP server
/// directories share this constructor so copying, lock ownership, and pruning
/// behave the same way.
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
            if item.source_name == *local_source {
                continue;
            }
            let original_name = item.id.name.clone();
            let new_dest_path =
                suffixed_collision_dest_path(&item.dest_path, kind, &item.source_name)?;
            let new_name = ItemName::from(new_dest_path.item_name(kind.item_kind()));

            diag.warn(
                "auto-rename-collision",
                format!(
                    "auto-renamed {} `{}` from source `{}` → `{}`",
                    item.id.kind, original_name, item.source_name, new_name
                ),
            );

            item.id.name = new_name.clone();
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

/// Existing on-disk destination that is not lock-managed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnmanagedCollision {
    pub source_name: SourceName,
    pub path: DestPath,
}

/// Detect target installs that would overwrite unmanaged on-disk content.
///
/// If a target destination already exists but Mars does not own it (`mars.lock` claims it or a
/// provider installed it), treat it as user-authored content and report it as a collision so
/// callers can skip installation while leaving existing files untouched.
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

/// Whether installing one item would overwrite content Mars does not own.
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

/// Item kinds whose installed destination is derived from the item's own name.
///
/// Hooks and MCP servers install as target-scoped package directories whose
/// paths `PackageDir` owns, so no rename names their destination.
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

/// Construct the default destination path for an item.
/// Uses string formatting to guarantee forward slashes on all platforms.
pub(crate) fn default_dest_path(kind: RenamableKind, name: &str) -> DestPath {
    let path_str = match kind {
        RenamableKind::Agent => format!("agents/{name}.md"),
        RenamableKind::Skill => format!("skills/{name}"),
        RenamableKind::BootstrapDoc => format!("bootstrap/{name}/BOOTSTRAP.md"),
    };
    // Safe: internal paths constructed from validated item names
    DestPath::new(path_str).expect("internal default path is always valid")
}

fn parse_rename_dest(
    kind: RenamableKind,
    rename_value: &str,
    source_name: &SourceName,
) -> Result<DestPath, MarsError> {
    // Normalize backslashes to forward slashes for cross-platform handling
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

fn suffixed_collision_dest_path(
    dest_path: &DestPath,
    kind: RenamableKind,
    source_name: &SourceName,
) -> Result<DestPath, MarsError> {
    let suffix = format!("__{source_name}");
    let path = dest_path.as_str();
    let renamed = match kind {
        RenamableKind::Agent => {
            let (parent, leaf) = split_parent_leaf(path);
            let stem = leaf.strip_suffix(".md").unwrap_or(leaf);
            mars_schema::discover::join(parent, &format!("{stem}{suffix}.md"))
        }
        RenamableKind::Skill => {
            return suffixed_leaf_dest_path(path, &suffix, source_name);
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

fn suffixed_leaf_dest_path(
    path: &str,
    suffix: &str,
    source_name: &SourceName,
) -> Result<DestPath, MarsError> {
    let (parent, leaf) = split_parent_leaf(path);
    let renamed = mars_schema::discover::join(parent, &format!("{leaf}{suffix}"));
    DestPath::new(&renamed).map_err(|e| MarsError::Source {
        source_name: source_name.to_string(),
        message: format!("invalid auto-renamed destination `{renamed}`: {e}"),
    })
}

fn split_parent_leaf(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::*;
    use crate::lock::LockFile;
    use crate::provided::ProvidedNames;
    use crate::resolve::{ResolvedGraph, ResolvedNode};
    use crate::source::ResolvedRef;
    use crate::types::SourceId;
    use indexmap::IndexMap;
    use mars_schema::filter::FilterMode;
    use std::fs;
    use tempfile::TempDir;

    /// The consumer's own package in a dependency-only test graph.
    fn test_local_source() -> SourceName {
        crate::types::SourceOrigin::LocalPackage.to_string().into()
    }

    /// Helper: create a source tree with agents and skills
    fn make_source_tree(agents: &[(&str, &str)], skills: &[(&str, &str)]) -> TempDir {
        let dir = TempDir::new().unwrap();
        if !agents.is_empty() {
            let agents_dir = dir.path().join("agents");
            fs::create_dir_all(&agents_dir).unwrap();
            for (name, content) in agents {
                fs::write(agents_dir.join(name), content).unwrap();
            }
        }
        if !skills.is_empty() {
            let skills_dir = dir.path().join("skills");
            fs::create_dir_all(&skills_dir).unwrap();
            for (name, content) in skills {
                let skill_dir = skills_dir.join(name);
                fs::create_dir_all(&skill_dir).unwrap();
                fs::write(skill_dir.join("SKILL.md"), content).unwrap();
            }
        }
        dir
    }

    fn make_graph_and_config(
        sources: Vec<(&str, &TempDir, Option<&str>, FilterMode)>,
    ) -> (ResolvedGraph, EffectiveConfig) {
        let mut nodes = IndexMap::new();
        let mut order = Vec::new();
        let mut config_dependencies = IndexMap::new();

        for (name, tree, url, filter) in sources {
            nodes.insert(
                name.into(),
                ResolvedNode {
                    fetch_url: None,
                    source_name: name.into(),
                    source_id: if let Some(u) = url {
                        SourceId::git_with_subpath(crate::types::SourceUrl::from(u), None)
                    } else {
                        SourceId::Path {
                            canonical: tree.path().to_path_buf(),
                            subpath: None,
                            plugin: None,
                        }
                    },
                    rooted_ref: crate::resolve::RootedSourceRef {
                        checkout_root: tree.path().to_path_buf(),
                        authored_root: tree.path().to_path_buf(),
                        package_root: tree.path().to_path_buf(),
                        plugin: None,
                    },
                    prepared: crate::package_prepare::prepare_tree_for_test(name, tree.path()),
                    resolved_ref: ResolvedRef {
                        source_name: name.into(),
                        version: None,
                        version_tag: None,
                        commit: None,
                        tree_path: tree.path().to_path_buf(),
                    },
                    manifest: None,
                },
            );
            order.push(name.into());

            let spec = if let Some(u) = url {
                SourceSpec::Git(GitSpec {
                    url: crate::types::SourceUrl::from(u),
                    version: None,
                })
            } else {
                SourceSpec::Path(tree.path().to_path_buf())
            };

            config_dependencies.insert(
                name.into(),
                EffectiveDependency {
                    declared_source_id: if let Some(u) = url {
                        SourceId::git_with_subpath(crate::types::SourceUrl::from(u), None)
                    } else {
                        SourceId::Path {
                            canonical: tree.path().to_path_buf(),
                            subpath: None,
                            plugin: None,
                        }
                    },
                    source_id: if let Some(u) = url {
                        SourceId::git_with_subpath(crate::types::SourceUrl::from(u), None)
                    } else {
                        SourceId::Path {
                            canonical: tree.path().to_path_buf(),
                            subpath: None,
                            plugin: None,
                        }
                    },
                    spec,
                    subpath: None,
                    filter,
                    rename: RenameMap::new(),
                    dialect: None,
                },
            );
        }

        let config = EffectiveConfig {
            dependencies: config_dependencies,
            settings: Settings::default(),
            models: indexmap::IndexMap::new(),
            agents: indexmap::IndexMap::new(),
            skills: indexmap::IndexMap::new(),
        };
        let graph = ResolvedGraph {
            selection: crate::resolve::select_for_test(&config, &nodes),
            nodes,
            order,
            version_constraints: std::collections::HashMap::new(),
            unreadable_hook_surfaces: std::collections::BTreeMap::new(),
        };
        (graph, config)
    }

    fn graph_node(name: &str, deps: &[&str]) -> ResolvedNode {
        let root = PathBuf::from(format!("/tmp/{name}"));
        ResolvedNode {
            fetch_url: None,
            source_name: name.into(),
            source_id: SourceId::Path {
                canonical: root.clone(),
                subpath: None,
                plugin: None,
            },
            rooted_ref: crate::resolve::RootedSourceRef {
                checkout_root: root.clone(),
                authored_root: root.clone(),
                package_root: root.clone(),
                plugin: None,
            },
            prepared: crate::resolve::prepared_with_deps(deps),
            resolved_ref: ResolvedRef {
                source_name: name.into(),
                version: None,
                version_tag: None,
                commit: None,
                tree_path: root,
            },
            manifest: None,
        }
    }

    #[test]
    fn hook_depths_are_stable_for_longer_path_discovered_after_shorter_path() {
        let definitions = [
            ("direct-short", vec!["shared"]),
            ("shared", vec!["leaf"]),
            ("leaf", vec![]),
            ("direct-long", vec!["middle"]),
            ("middle", vec!["shared"]),
        ];
        let graph_order: Vec<SourceName> =
            ["direct-short", "shared", "leaf", "direct-long", "middle"]
                .into_iter()
                .map(SourceName::from)
                .collect();
        let expected_emission = vec![
            "direct-long".to_string(),
            "direct-short".to_string(),
            "middle".to_string(),
            "shared".to_string(),
            "leaf".to_string(),
        ];
        let direct_dependencies = ["direct-short", "direct-long"]
            .into_iter()
            .map(|name| {
                let path = PathBuf::from(format!("/tmp/{name}"));
                (
                    SourceName::from(name),
                    EffectiveDependency {
                        declared_source_id: SourceId::Path {
                            canonical: path.clone(),
                            subpath: None,
                            plugin: None,
                        },
                        source_id: SourceId::Path {
                            canonical: path.clone(),
                            subpath: None,
                            plugin: None,
                        },
                        spec: SourceSpec::Path(path),
                        subpath: None,
                        filter: FilterMode::All,
                        rename: RenameMap::new(),
                        dialect: None,
                    },
                )
            })
            .collect();

        for run in 0..128 {
            let mut insertion_order = definitions.clone();
            let definition_count = insertion_order.len();
            insertion_order.rotate_left(run % definition_count);
            if run % 2 == 1 {
                insertion_order.reverse();
            }
            let nodes = insertion_order
                .into_iter()
                .map(|(name, deps)| (name.into(), graph_node(name, &deps)))
                .collect::<IndexMap<_, _>>();
            let graph = ResolvedGraph {
                order: graph_order.clone(),
                nodes,
                selection: Default::default(),
                version_constraints: HashMap::new(),
                unreadable_hook_surfaces: std::collections::BTreeMap::new(),
            };
            let depths = graph.dependency_depths(&direct_dependencies);
            let mut emitted_order = graph.order.clone();
            emitted_order.sort_by_key(|name| (depths[name], name.clone()));
            assert_eq!(
                emitted_order
                    .into_iter()
                    .map(|name| name.to_string())
                    .collect::<Vec<_>>(),
                expected_emission,
            );
        }
    }

    // === Target build tests ===

    #[test]
    fn build_single_source_no_filter() {
        let tree = make_source_tree(&[("coder.md", "# coder")], &[("planning", "# planning")]);
        let (graph, config) = make_graph_and_config(vec![(
            "base",
            &tree,
            Some("https://github.com/org/base"),
            FilterMode::All,
        )]);

        let (target, renames, _) = build_with_collisions_and_diag(
            &graph,
            &config,
            &test_local_source(),
            &mut DiagnosticCollector::new(),
        )
        .unwrap();
        assert!(renames.is_empty());
        assert_eq!(target.items.len(), 2);
        assert!(target.items.contains_key("agents/coder.md"));
        assert!(target.items.contains_key("skills/planning"));
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn invalid_windows_agent_filename_emits_diagnostic_and_skips() {
        // This test creates a file with `:` in the name, which is only possible on
        // non-Windows. The validation catches names that would break on Windows when
        // created on POSIX systems.
        let tree = make_source_tree(&[("bad:name.md", "# bad"), ("coder.md", "# coder")], &[]);
        let (graph, config) = make_graph_and_config(vec![(
            "base",
            &tree,
            Some("https://github.com/org/base"),
            FilterMode::All,
        )]);
        let mut diag = DiagnosticCollector::new();

        let (target, _, _) =
            build_with_collisions_and_diag(&graph, &config, &test_local_source(), &mut diag)
                .unwrap();
        let diagnostics = diag.drain();

        assert!(!target.items.contains_key("agents/bad:name.md"));
        assert!(target.items.contains_key("agents/coder.md"));
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, "invalid-agent-filename");
    }

    #[test]
    fn build_with_path_rename_mapping() {
        let tree = make_source_tree(&[("old-name.md", "# old")], &[]);

        let (graph, mut config) = make_graph_and_config(vec![(
            "base",
            &tree,
            Some("https://github.com/org/base"),
            FilterMode::All,
        )]);

        // Add rename mapping
        config
            .dependencies
            .get_mut("base")
            .unwrap()
            .rename
            .insert("agents/old-name.md".into(), "agents/new-name.md".into());

        let (target, renames, _) = build_with_collisions_and_diag(
            &graph,
            &config,
            &test_local_source(),
            &mut DiagnosticCollector::new(),
        )
        .unwrap();
        assert_eq!(renames.len(), 1);
        assert_eq!(renames[0].kind, ItemKind::Agent);
        assert_eq!(renames[0].original_name.as_str(), "old-name");
        assert_eq!(renames[0].new_name.as_str(), "new-name");
        assert_eq!(target.items.len(), 1);
        assert!(target.items.contains_key("agents/new-name.md"));
        assert_eq!(target.items["agents/new-name.md"].id.name, "new-name");
    }

    #[test]
    fn default_dest_path_uses_forward_slashes_for_agents_and_skills() {
        let agent = default_dest_path(RenamableKind::Agent, "coder");
        let skill = default_dest_path(RenamableKind::Skill, "planning");

        assert_eq!(agent.as_str(), "agents/coder.md");
        assert_eq!(skill.as_str(), "skills/planning");
        assert!(!agent.as_str().contains('\\'));
        assert!(!skill.as_str().contains('\\'));
    }

    #[test]
    fn parse_rename_dest_normalizes_backslashes_to_forward_slashes() {
        let source_name = SourceName::from("base");

        let agent = parse_rename_dest(
            RenamableKind::Agent,
            r"agents\nested\renamed.md",
            &source_name,
        )
        .unwrap();
        let skill = parse_rename_dest(
            RenamableKind::Skill,
            r"skills\nested\planning",
            &source_name,
        )
        .unwrap();

        assert_eq!(agent.as_str(), "agents/nested/renamed.md");
        assert_eq!(skill.as_str(), "skills/nested/planning");
        assert!(!agent.as_str().contains('\\'));
        assert!(!skill.as_str().contains('\\'));
    }

    #[test]
    fn parse_rename_dest_rejects_absolute_and_escape_destinations() {
        let source_name = SourceName::from("base");

        let absolute = parse_rename_dest(RenamableKind::Agent, "/tmp/escape", &source_name)
            .expect_err("absolute rename should fail");
        assert!(matches!(absolute, MarsError::Source { .. }));

        let traversal = parse_rename_dest(RenamableKind::Skill, "../escape", &source_name)
            .expect_err("traversal rename should fail");
        assert!(matches!(traversal, MarsError::Source { .. }));
    }

    #[test]
    fn build_with_invalid_rename_destination_returns_error() {
        let tree = make_source_tree(&[("old-name.md", "# old")], &[]);

        let (graph, mut config) =
            make_graph_and_config(vec![("base", &tree, None, FilterMode::All)]);

        config
            .dependencies
            .get_mut("base")
            .unwrap()
            .rename
            .insert("agents/old-name.md".into(), "../escape.md".into());

        let err = build_with_collisions_and_diag(
            &graph,
            &config,
            &test_local_source(),
            &mut DiagnosticCollector::new(),
        )
        .unwrap_err();
        assert!(matches!(err, MarsError::Source { .. }));
    }

    // === Collision tests ===

    #[test]
    fn collision_auto_renames_both() {
        let tree1 = make_source_tree(&[("coder.md", "# coder from source 1")], &[]);
        let tree2 = make_source_tree(&[("coder.md", "# coder from source 2")], &[]);

        let (graph, config) = make_graph_and_config(vec![
            (
                "source-a",
                &tree1,
                Some("https://github.com/alice/agents"),
                FilterMode::All,
            ),
            (
                "source-b",
                &tree2,
                Some("https://github.com/bob/agents"),
                FilterMode::All,
            ),
        ]);
        let mut diag = DiagnosticCollector::new();

        let (target, explicit_renames, collision_renames) =
            build_with_collisions_and_diag(&graph, &config, &test_local_source(), &mut diag)
                .unwrap();
        let diagnostics = diag.drain();

        assert!(explicit_renames.is_empty());
        assert_eq!(collision_renames.len(), 2);
        assert!(target.items.contains_key("agents/coder__source-a.md"));
        assert!(target.items.contains_key("agents/coder__source-b.md"));
        assert!(!target.items.contains_key("agents/coder.md"));
        assert_eq!(
            target.items["agents/coder__source-a.md"].id.name,
            "coder__source-a"
        );
        assert_eq!(
            target.items["agents/coder__source-b.md"].id.name,
            "coder__source-b"
        );
        assert_eq!(
            diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.code == "auto-rename-collision")
                .count(),
            2
        );
    }

    #[test]
    fn skill_collision_auto_renames_both() {
        let tree1 = make_source_tree(&[], &[("planning", "# planning from source 1")]);
        let tree2 = make_source_tree(&[], &[("planning", "# planning from source 2")]);

        let (graph, config) = make_graph_and_config(vec![
            ("source-a", &tree1, None, FilterMode::All),
            ("source-b", &tree2, None, FilterMode::All),
        ]);

        let (target, explicit_renames, collision_renames) = build_with_collisions_and_diag(
            &graph,
            &config,
            &test_local_source(),
            &mut DiagnosticCollector::new(),
        )
        .unwrap();

        assert!(explicit_renames.is_empty());
        assert_eq!(collision_renames.len(), 2);
        assert!(target.items.contains_key("skills/planning__source-a"));
        assert!(target.items.contains_key("skills/planning__source-b"));
        assert!(!target.items.contains_key("skills/planning"));
        assert_eq!(
            target.items["skills/planning__source-a"].id.name,
            "planning__source-a"
        );
        assert_eq!(
            target.items["skills/planning__source-b"].id.name,
            "planning__source-b"
        );
    }

    #[test]
    fn three_way_collision_renames_all() {
        let tree1 = make_source_tree(&[("coder.md", "# coder from source 1")], &[]);
        let tree2 = make_source_tree(&[("coder.md", "# coder from source 2")], &[]);
        let tree3 = make_source_tree(&[("coder.md", "# coder from source 3")], &[]);

        let (graph, config) = make_graph_and_config(vec![
            ("source-a", &tree1, None, FilterMode::All),
            ("source-b", &tree2, None, FilterMode::All),
            ("source-c", &tree3, None, FilterMode::All),
        ]);

        let (target, _, collision_renames) = build_with_collisions_and_diag(
            &graph,
            &config,
            &test_local_source(),
            &mut DiagnosticCollector::new(),
        )
        .unwrap();

        assert_eq!(collision_renames.len(), 3);
        assert!(target.items.contains_key("agents/coder__source-a.md"));
        assert!(target.items.contains_key("agents/coder__source-b.md"));
        assert!(target.items.contains_key("agents/coder__source-c.md"));
        assert!(!target.items.contains_key("agents/coder.md"));
    }

    #[test]
    fn explicit_rename_prevents_collision() {
        let tree1 = make_source_tree(&[("coder.md", "# coder from source 1")], &[]);
        let tree2 = make_source_tree(&[("coder.md", "# coder from source 2")], &[]);

        let (graph, mut config) = make_graph_and_config(vec![
            ("source-a", &tree1, None, FilterMode::All),
            ("source-b", &tree2, None, FilterMode::All),
        ]);
        config
            .dependencies
            .get_mut("source-a")
            .unwrap()
            .rename
            .insert("agents/coder.md".into(), "agents/source-a-coder.md".into());

        let (target, _, collision_renames) = build_with_collisions_and_diag(
            &graph,
            &config,
            &test_local_source(),
            &mut DiagnosticCollector::new(),
        )
        .unwrap();

        assert!(collision_renames.is_empty());
        assert!(target.items.contains_key("agents/source-a-coder.md"));
        assert!(target.items.contains_key("agents/coder.md"));
        assert!(!target.items.contains_key("agents/coder__source-a.md"));
        assert!(!target.items.contains_key("agents/coder__source-b.md"));
    }

    #[test]
    fn same_source_explicit_rename_collision_stays_hard_error() {
        let tree = make_source_tree(
            &[
                ("coder.md", "# coder"),
                ("reviewer.md", "# reviewer renamed into coder"),
            ],
            &[],
        );
        let (graph, mut config) =
            make_graph_and_config(vec![("source-a", &tree, None, FilterMode::All)]);
        config
            .dependencies
            .get_mut("source-a")
            .unwrap()
            .rename
            .insert("agents/reviewer.md".into(), "agents/coder.md".into());

        let err = build_with_collisions_and_diag(
            &graph,
            &config,
            &test_local_source(),
            &mut DiagnosticCollector::new(),
        )
        .unwrap_err();

        assert!(matches!(err, MarsError::Collision { .. }));
    }

    #[test]
    fn mixed_kind_explicit_rename_collision_stays_hard_error() {
        let tree = make_source_tree(&[("coder.md", "# coder")], &[("planning", "# planning")]);
        let (graph, mut config) =
            make_graph_and_config(vec![("source-a", &tree, None, FilterMode::All)]);
        config
            .dependencies
            .get_mut("source-a")
            .unwrap()
            .rename
            .insert("agents/coder.md".into(), "skills/planning".into());

        let err = build_with_collisions_and_diag(
            &graph,
            &config,
            &test_local_source(),
            &mut DiagnosticCollector::new(),
        )
        .unwrap_err();

        assert!(matches!(err, MarsError::Collision { .. }));
    }

    #[test]
    fn duplicate_source_in_cross_source_group_stays_hard_error_without_auto_warning() {
        let tree1 = make_source_tree(
            &[
                ("coder.md", "# coder"),
                ("reviewer.md", "# reviewer renamed into coder"),
            ],
            &[],
        );
        let tree2 = make_source_tree(&[("coder.md", "# coder from source 2")], &[]);
        let (graph, mut config) = make_graph_and_config(vec![
            ("source-a", &tree1, None, FilterMode::All),
            ("source-b", &tree2, None, FilterMode::All),
        ]);
        config
            .dependencies
            .get_mut("source-a")
            .unwrap()
            .rename
            .insert("agents/reviewer.md".into(), "agents/coder.md".into());
        let mut diag = DiagnosticCollector::new();

        let err = build_with_collisions_and_diag(&graph, &config, &test_local_source(), &mut diag)
            .unwrap_err();
        let diagnostics = diag.drain();

        assert!(matches!(err, MarsError::Collision { .. }));
        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| diagnostic.code != "auto-rename-collision")
        );
    }

    #[test]
    fn no_collision_no_renames() {
        let tree1 = make_source_tree(&[("coder.md", "# coder")], &[]);
        let tree2 = make_source_tree(&[("reviewer.md", "# reviewer")], &[]);

        let (graph, config) = make_graph_and_config(vec![
            (
                "source-a",
                &tree1,
                Some("https://github.com/alice/agents"),
                FilterMode::All,
            ),
            (
                "source-b",
                &tree2,
                Some("https://github.com/bob/agents"),
                FilterMode::All,
            ),
        ]);

        let (target, renames, collision_renames) = build_with_collisions_and_diag(
            &graph,
            &config,
            &test_local_source(),
            &mut DiagnosticCollector::new(),
        )
        .unwrap();
        assert!(renames.is_empty());
        assert!(collision_renames.is_empty());
        assert_eq!(target.items.len(), 2);
    }

    // === Source with agents filter + skill deps ===

    #[test]
    fn build_with_agents_filter_pulls_transitive_skills() {
        let tree = make_source_tree(
            &[("coder.md", "---\nskills:\n  - planning\n---\n# Coder\n")],
            &[("planning", "# Planning"), ("unused-skill", "# Unused")],
        );

        let (graph, config) = make_graph_and_config(vec![(
            "base",
            &tree,
            None,
            FilterMode::Include {
                agents: vec!["coder".into()],
                skills: vec![],
            },
        )]);

        let (target, renames, _) = build_with_collisions_and_diag(
            &graph,
            &config,
            &test_local_source(),
            &mut DiagnosticCollector::new(),
        )
        .unwrap();
        assert!(renames.is_empty());
        assert_eq!(target.items.len(), 2); // coder + planning
        assert!(target.items.contains_key("agents/coder.md"));
        assert!(target.items.contains_key("skills/planning"));
        // unused-skill should NOT be present
        assert!(!target.items.contains_key("skills/unused-skill"));
    }

    #[test]
    fn build_with_exclude_filter() {
        let tree = make_source_tree(&[("coder.md", "# coder"), ("deprecated.md", "# old")], &[]);

        let (graph, config) = make_graph_and_config(vec![(
            "base",
            &tree,
            None,
            FilterMode::Exclude(vec!["deprecated".into()]),
        )]);

        let (target, renames, _) = build_with_collisions_and_diag(
            &graph,
            &config,
            &test_local_source(),
            &mut DiagnosticCollector::new(),
        )
        .unwrap();
        assert!(renames.is_empty());
        assert_eq!(target.items.len(), 1);
        assert!(target.items.contains_key("agents/coder.md"));
    }

    #[test]
    fn build_target_items_have_correct_hashes() {
        let content = "# agent content for hash test";
        let tree = make_source_tree(&[("test.md", content)], &[]);

        let (graph, config) = make_graph_and_config(vec![("base", &tree, None, FilterMode::All)]);

        let (target, renames, _) = build_with_collisions_and_diag(
            &graph,
            &config,
            &test_local_source(),
            &mut DiagnosticCollector::new(),
        )
        .unwrap();
        assert!(renames.is_empty());
        let item = &target.items["agents/test.md"];
        let expected_hash = hash::hash_bytes(content.as_bytes());
        assert_eq!(item.source_hash, expected_hash);
    }

    #[test]
    fn unmanaged_disk_path_collision_reported() {
        let tree = make_source_tree(&[("coder.md", "# managed")], &[]);
        let (graph, config) = make_graph_and_config(vec![(
            "base",
            &tree,
            Some("https://github.com/org/base"),
            FilterMode::All,
        )]);

        let (target, renames, _) = build_with_collisions_and_diag(
            &graph,
            &config,
            &test_local_source(),
            &mut DiagnosticCollector::new(),
        )
        .unwrap();
        assert!(renames.is_empty());
        let install_root = TempDir::new().unwrap();

        // Existing user-authored file at the same destination.
        let existing = install_root.path().join("agents").join("coder.md");
        fs::create_dir_all(existing.parent().unwrap()).unwrap();
        fs::write(&existing, "# user-authored").unwrap();

        let collisions = check_unmanaged_collisions(
            install_root.path(),
            &Ownership::new(&LockFile::empty(), &ProvidedNames::default()),
            &target,
            false,
        );
        assert_eq!(collisions.len(), 1);
        assert_eq!(collisions[0].source_name.as_ref(), "base");
        assert_eq!(collisions[0].path.as_str(), "agents/coder.md");
    }

    #[test]
    fn unmanaged_collision_skipped_when_hash_matches() {
        let content = "# managed agent";
        let tree = make_source_tree(&[("coder.md", content)], &[]);
        let (graph, config) = make_graph_and_config(vec![(
            "base",
            &tree,
            Some("https://github.com/org/base"),
            FilterMode::All,
        )]);

        let (target, renames, _) = build_with_collisions_and_diag(
            &graph,
            &config,
            &test_local_source(),
            &mut DiagnosticCollector::new(),
        )
        .unwrap();
        assert!(renames.is_empty());
        let install_root = TempDir::new().unwrap();

        // Simulate partial prior install: file on disk with same content
        let existing = install_root.path().join("agents").join("coder.md");
        fs::create_dir_all(existing.parent().unwrap()).unwrap();
        fs::write(&existing, content).unwrap();

        // Should skip collision — disk content matches planned install (crash recovery)
        let collisions = check_unmanaged_collisions(
            install_root.path(),
            &Ownership::new(&LockFile::empty(), &ProvidedNames::default()),
            &target,
            false,
        );
        assert!(collisions.is_empty());
    }

    #[test]
    fn unmanaged_collision_reported_on_different_content() {
        let tree = make_source_tree(&[("coder.md", "# managed")], &[]);
        let (graph, config) = make_graph_and_config(vec![(
            "base",
            &tree,
            Some("https://github.com/org/base"),
            FilterMode::All,
        )]);

        let (target, renames, _) = build_with_collisions_and_diag(
            &graph,
            &config,
            &test_local_source(),
            &mut DiagnosticCollector::new(),
        )
        .unwrap();
        assert!(renames.is_empty());
        let install_root = TempDir::new().unwrap();

        // User-authored file with different content
        let existing = install_root.path().join("agents").join("coder.md");
        fs::create_dir_all(existing.parent().unwrap()).unwrap();
        fs::write(&existing, "# different user content").unwrap();

        let collisions = check_unmanaged_collisions(
            install_root.path(),
            &Ownership::new(&LockFile::empty(), &ProvidedNames::default()),
            &target,
            false,
        );
        assert_eq!(collisions.len(), 1);
        assert_eq!(collisions[0].source_name.as_ref(), "base");
        assert_eq!(collisions[0].path.as_str(), "agents/coder.md");
    }

    #[test]
    fn unmanaged_collision_skipped_under_force() {
        let tree = make_source_tree(&[("coder.md", "# managed")], &[]);
        let (graph, config) = make_graph_and_config(vec![(
            "base",
            &tree,
            Some("https://github.com/org/base"),
            FilterMode::All,
        )]);

        let (target, renames, _) = build_with_collisions_and_diag(
            &graph,
            &config,
            &test_local_source(),
            &mut DiagnosticCollector::new(),
        )
        .unwrap();
        assert!(renames.is_empty());
        let install_root = TempDir::new().unwrap();

        let existing = install_root.path().join("agents").join("coder.md");
        fs::create_dir_all(existing.parent().unwrap()).unwrap();
        fs::write(&existing, "# stale cache content").unwrap();

        let collisions = check_unmanaged_collisions(
            install_root.path(),
            &Ownership::new(&LockFile::empty(), &ProvidedNames::default()),
            &target,
            true,
        );
        assert!(collisions.is_empty());
    }
}
