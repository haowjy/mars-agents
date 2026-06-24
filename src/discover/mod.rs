//! Filesystem discovery for package-provided agents, skills, and bootstrap docs.
//!
//! Discovery is intentionally convention-based: a bounded walk finds directories
//! named `agents`, `skills`, and `bootstrap` instead of carrying harness-specific
//! blocklists. Hidden dot-directories are skipped during that walk so generated
//! harness surfaces like `.claude/` and tool caches like `.git/` are not imported
//! unless a dependency explicitly roots discovery there with `subpath`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Component, Path, PathBuf};

use serde_json::Value;

use crate::error::MarsError;
use crate::lock::{ItemId, ItemKind};
use crate::skill_source_name::flat_root_skill_source_name;
use crate::types::ItemName;

// These high-volume generated directories are skipped in addition to the dot-dir
// rule to avoid slow walks and false-positive imports from dependency/build
// outputs that sometimes contain docs shaped like agents or skills.
const RECURSIVE_SKIP_DIRS: &[&str] = &["node_modules", ".git", "dist", "build", "__pycache__"];
const PLUGIN_MANIFESTS: &[&str] = &[
    ".claude-plugin/plugin.json",
    ".claude-plugin/marketplace.json",
];
// Covers real package layouts like `vendor/pkg/.claude/skills/foo` and
// `packages/group/tooling/agents/foo.md` while intentionally skipping
// over-depth convention dirs silently so arbitrary repo trees do not become
// unbounded discovery surfaces.
const MAX_DISCOVERY_WALK_DEPTH: usize = 5;
const AGENTS_DIR_NAME: &str = "agents";
const SKILLS_DIR_NAME: &str = "skills";
const BOOTSTRAP_DIR_NAME: &str = "bootstrap";
const MANIFEST_SKILL_KEYS: &[&str] = &["skills", "skill_paths", "skillPaths"];
const MANIFEST_AGENT_KEYS: &[&str] = &["agents", "agent_paths", "agentPaths"];
const MANIFEST_BOOTSTRAP_KEYS: &[&str] = &["bootstrapDocs", "bootstrap_docs"];

/// An item discovered in a source tree by filesystem convention.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredItem {
    pub id: ItemId,
    /// Path within source tree (relative), e.g. "agents/coder.md" or "skills/planning".
    pub source_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LayeredItem {
    item: DiscoveredItem,
    // Convention grounding depends on the container directory that registered an
    // item, not on the item's leaf path. Nested package layouts can contain
    // repeated `skills`/`agents`/`bootstrap` segments, so deriving this later
    // from `source_path` can anchor to the wrong container.
    layer: usize,
}

/// Discover items by conventional mars package layout.
pub fn discover_source(
    tree_path: &Path,
    fallback_name: Option<&str>,
) -> Result<Vec<DiscoveredItem>, MarsError> {
    let items = discover_convention_items(tree_path, fallback_name)?;
    finalize_items(fallback_name.unwrap_or("unknown-source"), items)
}

/// Discover items from a source without a mars.toml manifest.
pub fn discover_manifestless_source(
    package_root: &Path,
    source_name: Option<&str>,
) -> Result<Vec<DiscoveredItem>, MarsError> {
    let label = source_name.unwrap_or("unknown-source");
    let convention_items = discover_convention_items(package_root, source_name)?;

    let mut items = convention_items;
    items.append(&mut discover_manifest_declared_items(package_root, label)?);
    finalize_items(label, items)
}

/// Shared dispatcher for rooted-source discovery.
pub fn discover_resolved_source(
    package_root: &Path,
    source_name: Option<&str>,
) -> Result<Vec<DiscoveredItem>, MarsError> {
    if package_root.join("mars.toml").is_file() {
        discover_source(package_root, source_name)
    } else {
        discover_manifestless_source(package_root, source_name)
    }
}

fn discover_convention_items(
    package_root: &Path,
    source_name: Option<&str>,
) -> Result<Vec<DiscoveredItem>, MarsError> {
    if !package_root.is_dir() {
        return Ok(Vec::new());
    }

    let mut items = Vec::new();
    let mut scratch = Vec::new();
    let mut visited_agents = HashSet::new();
    let mut visited_skills = HashSet::new();
    let mut visited_bootstrap = HashSet::new();
    let mut queue = VecDeque::from([(package_root.to_path_buf(), 0usize)]);

    while let Some((base_dir, depth)) = queue.pop_front() {
        let base_rel = if base_dir == package_root {
            PathBuf::new()
        } else {
            relative_to(package_root, &base_dir)?
        };

        match base_dir.file_name().and_then(|name| name.to_str()) {
            Some(AGENTS_DIR_NAME) => {
                scan_agent_dir(package_root, &base_rel, &mut scratch, &mut visited_agents)?;
                push_layered_items(&mut items, &mut scratch, convention_layer(&base_rel));
            }
            Some(SKILLS_DIR_NAME) => {
                scan_skill_dir(package_root, &base_rel, &mut scratch, &mut visited_skills)?;
                push_layered_items(&mut items, &mut scratch, convention_layer(&base_rel));
            }
            Some(BOOTSTRAP_DIR_NAME) => {
                scan_bootstrap_dir(
                    package_root,
                    &base_rel,
                    &mut scratch,
                    &mut visited_bootstrap,
                )?;
                push_layered_items(&mut items, &mut scratch, convention_layer(&base_rel));
            }
            _ => {}
        }

        if depth == MAX_DISCOVERY_WALK_DEPTH {
            continue;
        }

        for path in read_dir_paths_sorted(&base_dir)? {
            if !path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            // Hidden directories are generated/cache/control surfaces by convention.
            // Consumers can still import a hidden foreign layout explicitly by rooting
            // the package at that directory with `subpath = ".claude"`.
            if name.starts_with('.') || RECURSIVE_SKIP_DIRS.contains(&name) {
                continue;
            }
            queue.push_back((path, depth + 1));
        }
    }

    let found_agent_or_skill_before_grounding = items
        .iter()
        .any(|item| matches!(item.item.id.kind, ItemKind::Agent | ItemKind::Skill));

    items = ground_items_to_shallowest_layer(items);

    if !found_agent_or_skill_before_grounding && package_root.join("SKILL.md").is_file() {
        let name = flat_root_skill_source_name(package_root, source_name);
        items.push(LayeredItem {
            item: DiscoveredItem {
                id: ItemId {
                    kind: ItemKind::Skill,
                    name: ItemName::from(name),
                },
                source_path: PathBuf::from("."),
            },
            layer: 0,
        });
    }

    Ok(items.into_iter().map(|layered| layered.item).collect())
}

fn push_layered_items(
    items: &mut Vec<LayeredItem>,
    scratch: &mut Vec<DiscoveredItem>,
    layer: usize,
) {
    items.extend(scratch.drain(..).map(|item| LayeredItem { item, layer }));
}

fn convention_layer(relative_root: &Path) -> usize {
    relative_root.components().count()
}

fn scan_skill_dir(
    package_root: &Path,
    relative_root: &Path,
    items: &mut Vec<DiscoveredItem>,
    visited: &mut HashSet<PathBuf>,
) -> Result<(), MarsError> {
    let dir = package_root.join(relative_root);
    if !dir.is_dir() {
        return Ok(());
    }

    for path in read_dir_paths_sorted(&dir)? {
        if !path.is_dir() {
            continue;
        }
        if let Some(name) = path.file_name().and_then(|name| name.to_str())
            && name.starts_with('.')
        {
            continue;
        }
        let rel = relative_to(package_root, &path)?;
        register_skill_dir(package_root, &rel, items, visited)?;
    }

    Ok(())
}

fn scan_agent_dir(
    package_root: &Path,
    relative_root: &Path,
    items: &mut Vec<DiscoveredItem>,
    visited: &mut HashSet<PathBuf>,
) -> Result<(), MarsError> {
    let dir = package_root.join(relative_root);
    if !dir.is_dir() {
        return Ok(());
    }

    for path in read_dir_paths_sorted(&dir)? {
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }
        let rel = relative_to(package_root, &path)?;
        register_agent_file(&rel, items, visited);
    }

    Ok(())
}

fn scan_bootstrap_dir(
    package_root: &Path,
    relative_root: &Path,
    items: &mut Vec<DiscoveredItem>,
    visited: &mut HashSet<PathBuf>,
) -> Result<(), MarsError> {
    let dir = package_root.join(relative_root);
    if !dir.is_dir() {
        return Ok(());
    }

    for path in read_dir_paths_sorted(&dir)? {
        if !path.is_dir() {
            continue;
        }
        if let Some(name) = path.file_name().and_then(|name| name.to_str())
            && name.starts_with('.')
        {
            continue;
        }
        let rel = relative_to(package_root, &path)?;
        register_bootstrap_doc(package_root, &rel, items, visited)?;
    }

    Ok(())
}

fn scan_manifest_declared_path(
    package_root: &Path,
    declared_path: &DeclaredPath,
    items: &mut Vec<DiscoveredItem>,
) -> Result<(), MarsError> {
    let mut visited = HashSet::new();
    let candidate = package_root.join(&declared_path.relative_path);
    match declared_path.kind {
        ItemKind::Skill => {
            if candidate.join("SKILL.md").is_file() {
                register_skill_dir(
                    package_root,
                    &declared_path.relative_path,
                    items,
                    &mut visited,
                )?;
            } else if candidate.is_dir() {
                scan_skill_dir(
                    package_root,
                    &declared_path.relative_path,
                    items,
                    &mut visited,
                )?;
            }
        }
        ItemKind::Agent => {
            if candidate.is_file()
                && candidate.extension().and_then(|ext| ext.to_str()) == Some("md")
            {
                register_agent_file(&declared_path.relative_path, items, &mut visited);
            } else if candidate.is_dir() {
                scan_agent_dir(
                    package_root,
                    &declared_path.relative_path,
                    items,
                    &mut visited,
                )?;
            }
        }
        ItemKind::BootstrapDoc => {
            if candidate.join("BOOTSTRAP.md").is_file() {
                register_bootstrap_doc(
                    package_root,
                    &declared_path.relative_path,
                    items,
                    &mut visited,
                )?;
            } else if candidate
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name == "BOOTSTRAP.md")
                && candidate.is_file()
                && let Some(parent) = declared_path.relative_path.parent()
            {
                register_bootstrap_doc(package_root, parent, items, &mut visited)?;
            } else if candidate.is_dir() {
                scan_bootstrap_dir(
                    package_root,
                    &declared_path.relative_path,
                    items,
                    &mut visited,
                )?;
            }
        }
        // New config kinds not yet handled by source discovery.
        ItemKind::Hook | ItemKind::McpServer => {}
    }

    Ok(())
}

fn register_skill_dir(
    package_root: &Path,
    relative_path: &Path,
    items: &mut Vec<DiscoveredItem>,
    visited: &mut HashSet<PathBuf>,
) -> Result<(), MarsError> {
    let normalized = normalize_relative_path(relative_path);
    if !visited.insert(normalized.clone()) {
        return Ok(());
    }
    if !package_root.join(&normalized).join("SKILL.md").is_file() {
        return Ok(());
    }
    let name = normalized
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    items.push(DiscoveredItem {
        id: ItemId {
            kind: ItemKind::Skill,
            name: ItemName::from(name.to_string()),
        },
        source_path: normalized,
    });
    Ok(())
}

fn register_agent_file(
    relative_path: &Path,
    items: &mut Vec<DiscoveredItem>,
    visited: &mut HashSet<PathBuf>,
) {
    let normalized = normalize_relative_path(relative_path);
    if !visited.insert(normalized.clone()) {
        return;
    }
    let name = normalized
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    items.push(DiscoveredItem {
        id: ItemId {
            kind: ItemKind::Agent,
            name: ItemName::from(name.to_string()),
        },
        source_path: normalized,
    });
}

fn register_bootstrap_doc(
    package_root: &Path,
    relative_path: &Path,
    items: &mut Vec<DiscoveredItem>,
    visited: &mut HashSet<PathBuf>,
) -> Result<(), MarsError> {
    let normalized = normalize_relative_path(relative_path);
    if !visited.insert(normalized.clone()) {
        return Ok(());
    }
    if !package_root
        .join(&normalized)
        .join("BOOTSTRAP.md")
        .is_file()
    {
        return Ok(());
    }
    let name = normalized
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    items.push(DiscoveredItem {
        id: ItemId {
            kind: ItemKind::BootstrapDoc,
            name: ItemName::from(name.to_string()),
        },
        source_path: normalized,
    });
    Ok(())
}

fn discover_manifest_declared_items(
    package_root: &Path,
    source_name: &str,
) -> Result<Vec<DiscoveredItem>, MarsError> {
    let mut items = Vec::new();
    for declared_path in collect_manifest_declared_paths(package_root, source_name)? {
        scan_manifest_declared_path(package_root, &declared_path, &mut items)?;
    }
    Ok(dedupe_items_by_path(items))
}

fn ground_items_to_shallowest_layer(items: Vec<LayeredItem>) -> Vec<LayeredItem> {
    let Some(min_layer) = items.iter().map(|item| item.layer).min() else {
        return items;
    };

    // Grounding: agents/skills/bootstrap docs live at one logical layer; find
    // the shallowest layer that has them and ignore deeper containers. This
    // prevents importing nested fixture, example, or vendored package layouts
    // when a package also exposes its own top-level convention directories.
    items
        .into_iter()
        .filter(|item| item.layer == min_layer)
        .collect()
}

fn finalize_items(
    source_name: &str,
    mut items: Vec<DiscoveredItem>,
) -> Result<Vec<DiscoveredItem>, MarsError> {
    items = dedupe_items_by_path(items);
    ensure_unique_names(source_name, &items)?;
    sort_items(&mut items);
    Ok(items)
}

fn dedupe_items_by_path(items: Vec<DiscoveredItem>) -> Vec<DiscoveredItem> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::with_capacity(items.len());
    for item in items {
        if seen.insert(item.source_path.clone()) {
            deduped.push(item);
        }
    }
    deduped
}

fn collect_manifest_declared_paths(
    package_root: &Path,
    source_name: &str,
) -> Result<Vec<DeclaredPath>, MarsError> {
    let mut declared = Vec::new();
    for manifest in PLUGIN_MANIFESTS {
        let path = package_root.join(manifest);
        if !path.is_file() {
            continue;
        }
        let content = std::fs::read_to_string(&path)?;
        let json: Value = serde_json::from_str(&content).map_err(|e| MarsError::Source {
            source_name: source_name.to_string(),
            message: format!("failed to parse plugin manifest `{}`: {e}", path.display()),
        })?;
        declared.extend(parse_declared_paths(&json));
    }

    let mut resolved = Vec::new();
    let mut seen = HashSet::new();
    for raw in declared {
        if !raw.raw_path.starts_with("./") {
            continue;
        }
        let normalized = normalize_manifest_declared_path(&raw.raw_path).ok_or_else(|| {
            MarsError::ManifestDeclaredPathEscape {
                source_name: source_name.to_string(),
                manifest_path: raw.raw_path.display().to_string(),
                package_root: package_root.to_path_buf(),
            }
        })?;
        let candidate = package_root.join(&normalized);
        if !candidate.exists() {
            return Err(MarsError::ManifestDeclaredPathMissing {
                source_name: source_name.to_string(),
                manifest_path: raw.raw_path.display().to_string(),
                package_root: package_root.to_path_buf(),
            });
        }
        let canonical = dunce::canonicalize(&candidate).map_err(|_| {
            MarsError::ManifestDeclaredPathMissing {
                source_name: source_name.to_string(),
                manifest_path: raw.raw_path.display().to_string(),
                package_root: package_root.to_path_buf(),
            }
        })?;
        let canonical_root = dunce::canonicalize(package_root).map_err(|e| MarsError::Source {
            source_name: source_name.to_string(),
            message: format!(
                "failed to canonicalize package root `{}`: {e}",
                package_root.display()
            ),
        })?;
        if !canonical.starts_with(&canonical_root) {
            return Err(MarsError::ManifestDeclaredPathEscape {
                source_name: source_name.to_string(),
                manifest_path: raw.raw_path.display().to_string(),
                package_root: package_root.to_path_buf(),
            });
        }
        let rel = relative_to(package_root, &candidate)?;
        if seen.insert((raw.kind, rel.clone())) {
            resolved.push(DeclaredPath {
                kind: raw.kind,
                relative_path: rel,
            });
        }
    }
    Ok(resolved)
}

fn ensure_unique_names(source_name: &str, items: &[DiscoveredItem]) -> Result<(), MarsError> {
    let mut seen: HashMap<(ItemKind, String), PathBuf> = HashMap::new();
    for item in items {
        let key = (item.id.kind, item.id.name.to_string());
        if let Some(existing) = seen.insert(key.clone(), item.source_path.clone()) {
            return Err(MarsError::DiscoveryCollision {
                source_name: source_name.to_string(),
                kind: item.id.kind.to_string(),
                item_name: item.id.name.to_string(),
                path_a: existing,
                path_b: item.source_path.clone(),
            });
        }
    }
    Ok(())
}

fn relative_to(base: &Path, child: &Path) -> Result<PathBuf, MarsError> {
    child
        .strip_prefix(base)
        .map(|path| path.to_path_buf())
        .map_err(|_| MarsError::Source {
            source_name: "discover".to_string(),
            message: format!(
                "path `{}` is not under package root `{}`",
                child.display(),
                base.display()
            ),
        })
}

fn normalize_relative_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        normalized.push(component.as_os_str());
    }
    normalized
}

fn normalize_manifest_declared_path(path: &Path) -> Option<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(seg) => normalized.push(seg),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    if normalized.as_os_str().is_empty() {
        None
    } else {
        Some(normalized)
    }
}

fn read_dir_paths_sorted(dir: &Path) -> Result<Vec<PathBuf>, MarsError> {
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        paths.push(entry?.path());
    }
    paths.sort();
    Ok(paths)
}

fn parse_declared_paths(json: &Value) -> Vec<RawDeclaredPath> {
    let Some(map) = json.as_object() else {
        return Vec::new();
    };

    let mut declared = Vec::new();
    for key in MANIFEST_SKILL_KEYS {
        if let Some(value) = map.get(*key) {
            collect_declared_paths_from_value(ItemKind::Skill, value, &mut declared);
        }
    }
    for key in MANIFEST_AGENT_KEYS {
        if let Some(value) = map.get(*key) {
            collect_declared_paths_from_value(ItemKind::Agent, value, &mut declared);
        }
    }
    for key in MANIFEST_BOOTSTRAP_KEYS {
        if let Some(value) = map.get(*key) {
            collect_declared_paths_from_value(ItemKind::BootstrapDoc, value, &mut declared);
        }
    }
    declared
}

fn collect_declared_paths_from_value(
    kind: ItemKind,
    value: &Value,
    declared: &mut Vec<RawDeclaredPath>,
) {
    match value {
        Value::String(path) => declared.push(RawDeclaredPath {
            kind,
            raw_path: PathBuf::from(path),
        }),
        Value::Array(values) => {
            for child in values {
                collect_declared_paths_from_value(kind, child, declared);
            }
        }
        Value::Object(map) => {
            if let Some(path) = map.get("path").and_then(|value| value.as_str()) {
                declared.push(RawDeclaredPath {
                    kind,
                    raw_path: PathBuf::from(path),
                });
            }
        }
        _ => {}
    }
}

#[derive(Debug, Clone)]
struct RawDeclaredPath {
    kind: ItemKind,
    raw_path: PathBuf,
}

#[derive(Debug, Clone)]
struct DeclaredPath {
    kind: ItemKind,
    relative_path: PathBuf,
}

fn sort_items(items: &mut [DiscoveredItem]) {
    items.sort_by(|a, b| {
        a.id.cmp(&b.id)
            .then_with(|| a.source_path.cmp(&b.source_path))
    });
}

/// An installed item with parsed frontmatter metadata.
#[derive(Debug, Clone)]
pub struct InstalledItem {
    pub id: ItemId,
    /// Disk path (absolute) to the installed file/dir.
    pub path: PathBuf,
    /// Parsed frontmatter name (may differ from filename).
    pub frontmatter_name: Option<String>,
    /// Parsed frontmatter description.
    pub description: Option<String>,
    /// Skills referenced in frontmatter (agents only).
    pub skill_refs: Vec<String>,
}

/// Result of scanning an installed managed root.
#[derive(Debug, Clone)]
pub struct InstalledState {
    pub agents: Vec<InstalledItem>,
    pub skills: Vec<InstalledItem>,
}

/// Discover all installed agents and skills in a managed root.
pub fn discover_installed(root: &Path) -> Result<InstalledState, MarsError> {
    let mut agents = Vec::new();
    let mut skills = Vec::new();

    let mut scratch = Vec::new();
    let mut visited = HashSet::new();
    scan_agent_dir(root, Path::new("agents"), &mut scratch, &mut visited)?;
    for item in scratch.drain(..) {
        let path = root.join(&item.source_path);
        let (frontmatter_name, description, skill_refs) = parse_installed_frontmatter(&path);
        agents.push(InstalledItem {
            id: item.id,
            path,
            frontmatter_name,
            description,
            skill_refs,
        });
    }

    scan_skill_dir(root, Path::new("skills"), &mut scratch, &mut HashSet::new())?;
    for item in scratch.drain(..) {
        let path = root.join(&item.source_path);
        let skill_md = if item.source_path == Path::new(".") {
            root.join("SKILL.md")
        } else {
            path.join("SKILL.md")
        };
        let (frontmatter_name, description, _) = parse_installed_frontmatter(&skill_md);
        skills.push(InstalledItem {
            id: item.id,
            path,
            frontmatter_name,
            description,
            skill_refs: Vec::new(),
        });
    }

    sort_installed(&mut agents);
    sort_installed(&mut skills);
    Ok(InstalledState { agents, skills })
}

fn parse_installed_frontmatter(path: &Path) -> (Option<String>, Option<String>, Vec<String>) {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return (None, None, Vec::new()),
    };
    match crate::frontmatter::parse(&content) {
        Ok(fm) => {
            let name = fm.name().map(str::to_owned);
            let description = fm
                .get("description")
                .and_then(|value| value.as_str())
                .map(str::to_owned);
            (name, description, fm.skills())
        }
        Err(_) => (None, None, Vec::new()),
    }
}

fn sort_installed(items: &mut [InstalledItem]) {
    items.sort_by(|a, b| a.id.cmp(&b.id).then_with(|| a.path.cmp(&b.path)));
}

#[cfg(test)]
mod tests;
