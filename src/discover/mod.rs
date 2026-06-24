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

/// Discover items by conventional mars package layout.
pub fn discover_source(
    tree_path: &Path,
    fallback_name: Option<&str>,
) -> Result<Vec<DiscoveredItem>, MarsError> {
    finalize_items(
        fallback_name.unwrap_or("unknown-source"),
        discover_convention_items(tree_path, fallback_name)?,
    )
}

/// Discover items from a source without a mars.toml manifest.
pub fn discover_manifestless_source(
    package_root: &Path,
    source_name: Option<&str>,
) -> Result<Vec<DiscoveredItem>, MarsError> {
    let label = source_name.unwrap_or("unknown-source");
    let convention_items = discover_convention_items(package_root, source_name)?;
    let mut explicit_items = discover_manifest_declared_items(package_root, label)?;

    let mut items = convention_items;
    items.append(&mut explicit_items);
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
    let mut visited_agents = HashSet::new();
    let mut visited_skills = HashSet::new();
    let mut visited_bootstrap = HashSet::new();
    let mut queue = VecDeque::from([(package_root.to_path_buf(), 0usize)]);

    while let Some((base_dir, depth)) = queue.pop_front() {
        if depth > MAX_DISCOVERY_WALK_DEPTH {
            continue;
        }

        let base_rel = if base_dir == package_root {
            PathBuf::new()
        } else {
            relative_to(package_root, &base_dir)?
        };

        match base_dir.file_name().and_then(|name| name.to_str()) {
            Some(AGENTS_DIR_NAME) => {
                scan_agent_dir(package_root, &base_rel, &mut items, &mut visited_agents)?;
            }
            Some(SKILLS_DIR_NAME) => {
                scan_skill_dir(package_root, &base_rel, &mut items, &mut visited_skills)?;
            }
            Some(BOOTSTRAP_DIR_NAME) => {
                scan_bootstrap_dir(package_root, &base_rel, &mut items, &mut visited_bootstrap)?;
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

    let has_agent_or_skill = items
        .iter()
        .any(|item| matches!(item.id.kind, ItemKind::Agent | ItemKind::Skill));
    if !has_agent_or_skill && package_root.join("SKILL.md").is_file() {
        let name = flat_root_skill_source_name(package_root, source_name);
        items.push(DiscoveredItem {
            id: ItemId {
                kind: ItemKind::Skill,
                name: ItemName::from(name),
            },
            source_path: PathBuf::from("."),
        });
    }

    items.sort_by(|a, b| {
        logical_layer(a)
            .cmp(&logical_layer(b))
            .then_with(|| a.source_path.cmp(&b.source_path))
    });
    Ok(dedupe_items_by_path(items))
}

fn logical_layer(item: &DiscoveredItem) -> usize {
    if item.source_path == Path::new(".") {
        return 0;
    }

    item.source_path
        .components()
        .enumerate()
        .find_map(|(index, component)| {
            let segment = component.as_os_str().to_str()?;
            match (item.id.kind, segment) {
                (ItemKind::Agent, AGENTS_DIR_NAME)
                | (ItemKind::Skill, SKILLS_DIR_NAME)
                | (ItemKind::BootstrapDoc, BOOTSTRAP_DIR_NAME) => Some(index + 1),
                _ => None,
            }
        })
        .unwrap_or(usize::MAX)
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

fn finalize_items(
    source_name: &str,
    mut items: Vec<DiscoveredItem>,
) -> Result<Vec<DiscoveredItem>, MarsError> {
    items = dedupe_items_by_path(items);
    items = dedupe_items_by_name_precedence(items);
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

fn dedupe_items_by_name_precedence(mut items: Vec<DiscoveredItem>) -> Vec<DiscoveredItem> {
    // `logical_layer` is name-precedence, not just display order: for duplicate
    // `(kind, name)` discoveries, the shallowest convention container wins.
    // Source-path order makes equal-layer ties deterministic and preserves the
    // old first-seen behavior because directory reads are sorted.
    items.sort_by(|a, b| {
        logical_layer(a)
            .cmp(&logical_layer(b))
            .then_with(|| a.source_path.cmp(&b.source_path))
    });

    let mut seen = HashSet::new();
    let mut deduped = Vec::with_capacity(items.len());
    for item in items {
        let key = (item.id.kind, item.id.name.clone());
        if seen.insert(key) {
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
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn conventional_discovery_finds_agents_and_skills() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("agents")).unwrap();
        fs::create_dir_all(dir.path().join("skills/planning")).unwrap();
        fs::write(dir.path().join("agents/coder.md"), "# coder").unwrap();
        fs::write(dir.path().join("skills/planning/SKILL.md"), "# planning").unwrap();

        let items = discover_source(dir.path(), None).unwrap();
        assert_eq!(items.len(), 2);
        assert!(
            items
                .iter()
                .any(|item| item.source_path == Path::new("agents/coder.md"))
        );
        assert!(
            items
                .iter()
                .any(|item| item.source_path == Path::new("skills/planning"))
        );
    }

    #[test]
    fn conventional_discovery_finds_nested_convention_dirs() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("agents")).unwrap();
        fs::create_dir_all(dir.path().join("sub/agents")).unwrap();
        fs::create_dir_all(dir.path().join("a/b/skills/bar")).unwrap();
        fs::write(dir.path().join("agents/top.md"), "# top").unwrap();
        fs::write(dir.path().join("sub/agents/foo.md"), "# foo").unwrap();
        fs::write(dir.path().join("a/b/skills/bar/SKILL.md"), "# bar").unwrap();

        let items = discover_source(dir.path(), None).unwrap();

        assert_eq!(items.len(), 3);
        assert!(
            items
                .iter()
                .any(|item| item.source_path == Path::new("agents/top.md"))
        );
        assert!(
            items
                .iter()
                .any(|item| item.source_path == Path::new("sub/agents/foo.md"))
        );
        assert!(
            items
                .iter()
                .any(|item| item.source_path == Path::new("a/b/skills/bar"))
        );
    }

    #[test]
    fn conventional_discovery_finds_package_bootstrap_docs() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("bootstrap/global-auth")).unwrap();
        fs::create_dir_all(dir.path().join("bootstrap/.hidden")).unwrap();
        fs::write(
            dir.path().join("bootstrap/global-auth/BOOTSTRAP.md"),
            "# auth",
        )
        .unwrap();
        fs::write(dir.path().join("bootstrap/.hidden/BOOTSTRAP.md"), "# hide").unwrap();

        let items = discover_source(dir.path(), None).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id.kind, ItemKind::BootstrapDoc);
        assert_eq!(items[0].id.name.as_str(), "global-auth");
        assert_eq!(items[0].source_path, PathBuf::from("bootstrap/global-auth"));
    }

    #[test]
    fn conventional_bootstrap_discovery_ignores_missing_bootstrap_file() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("bootstrap/incomplete")).unwrap();
        fs::write(
            dir.path().join("bootstrap/incomplete/README.md"),
            "# readme",
        )
        .unwrap();

        let items = discover_source(dir.path(), None).unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn dispatcher_prefers_conventional_when_manifest_exists() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("mars.toml"),
            "[package]\nname='demo'\nversion='0.1.0'\n",
        )
        .unwrap();
        fs::create_dir_all(dir.path().join("skills/planning")).unwrap();
        fs::write(dir.path().join("skills/planning/SKILL.md"), "# planning").unwrap();
        fs::create_dir_all(dir.path().join("nested")).unwrap();
        fs::write(dir.path().join("nested/SKILL.md"), "# nested").unwrap();

        let items = discover_resolved_source(dir.path(), Some("demo")).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].source_path, PathBuf::from("skills/planning"));
    }

    #[test]
    fn fallback_root_skill_does_not_override_convention_items() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("SKILL.md"), "# root").unwrap();
        fs::create_dir_all(dir.path().join("skills/planning")).unwrap();
        fs::write(dir.path().join("skills/planning/SKILL.md"), "# planning").unwrap();

        let items = discover_manifestless_source(dir.path(), Some("demo")).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id.name.as_str(), "planning");
        assert_eq!(items[0].source_path, PathBuf::from("skills/planning"));
    }

    #[test]
    fn fallback_flat_root_skill_uses_package_basename_when_no_source_name() {
        let dir = TempDir::new().unwrap();
        let pkg = dir.path().join("my-pkg");
        fs::create_dir_all(&pkg).unwrap();
        fs::write(pkg.join("SKILL.md"), "# flat").unwrap();

        let items = discover_manifestless_source(&pkg, None).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id.name.as_str(), "my-pkg");
        assert_ne!(items[0].id.name.as_str(), "unknown-source");
    }

    #[test]
    fn fallback_flat_root_skill_uses_source_name_not_staged_dialect_dir() {
        let dir = TempDir::new().unwrap();
        // Simulates staged package root named after inbound dialect (e.g. codex/).
        let staged = dir.path().join("codex");
        fs::create_dir_all(&staged).unwrap();
        fs::write(staged.join("SKILL.md"), "# flat foreign skill").unwrap();

        let items = discover_manifestless_source(&staged, Some("base")).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id.name.as_str(), "base");
        assert_eq!(items[0].source_path, PathBuf::from("."));
    }

    #[test]
    fn fallback_flat_root_skill_overlay_applies_under_source_name() {
        use crate::config::SkillOverlay;
        use crate::diagnostic::DiagnosticCollector;
        use crate::dialect::Dialect;
        use crate::staging::stage_canonical_source;
        use crate::types::RenameMap;
        use indexmap::IndexMap;

        let source = TempDir::new().unwrap();
        fs::write(
            source.path().join("SKILL.md"),
            "---\nname: base\ndescription: base desc\n---\n# Flat\n",
        )
        .unwrap();

        let mut overrides = IndexMap::new();
        overrides.insert(
            "base".to_string(),
            SkillOverlay {
                description: Some("overlay desc".to_string()),
                ..SkillOverlay::default()
            },
        );

        let staged = TempDir::new().unwrap();
        let staged_root = staged.path().join("codex");
        let mut diag = DiagnosticCollector::new();
        stage_canonical_source(
            source.path(),
            &staged_root,
            Dialect::Codex,
            &overrides,
            &RenameMap::new(),
            Some("base"),
            &mut diag,
        )
        .unwrap();

        let items = discover_manifestless_source(&staged_root, Some("base")).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id.name.as_str(), "base");

        let content = fs::read_to_string(staged_root.join("SKILL.md")).unwrap();
        assert!(
            content.contains("description: overlay desc"),
            "skills.base overlay must apply during staging: {content}"
        );
    }

    #[test]
    fn fallback_flat_root_skill_overlay_applies_after_rename() {
        use crate::config::SkillOverlay;
        use crate::diagnostic::DiagnosticCollector;
        use crate::dialect::Dialect;
        use crate::staging::stage_canonical_source;
        use crate::types::{ItemName, RenameMap};
        use indexmap::IndexMap;

        let source = TempDir::new().unwrap();
        fs::write(
            source.path().join("SKILL.md"),
            "---\nname: base\ndescription: base desc\n---\n# Flat\n",
        )
        .unwrap();

        let mut overrides = IndexMap::new();
        overrides.insert(
            "renamed-skill".to_string(),
            SkillOverlay {
                description: Some("renamed overlay".to_string()),
                ..SkillOverlay::default()
            },
        );
        let mut renames = RenameMap::new();
        renames.insert(ItemName::from("base"), ItemName::from("renamed-skill"));

        let staged = TempDir::new().unwrap();
        let staged_root = staged.path().join("codex");
        let mut diag = DiagnosticCollector::new();
        stage_canonical_source(
            source.path(),
            &staged_root,
            Dialect::Codex,
            &overrides,
            &renames,
            Some("base"),
            &mut diag,
        )
        .unwrap();

        let items = discover_manifestless_source(&staged_root, Some("base")).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0].id.name.as_str(),
            "base",
            "discovery keys flat skills by source name; rename applies later in target build"
        );

        let content = fs::read_to_string(staged_root.join("SKILL.md")).unwrap();
        assert!(
            content.contains("description: renamed overlay"),
            "skills.renamed-skill overlay must apply after rename during staging: {content}"
        );
    }

    #[test]
    fn fallback_root_skill_includes_manifest_bootstrap_docs() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("SKILL.md"), "# root").unwrap();
        fs::create_dir_all(dir.path().join("docs/global-auth")).unwrap();
        fs::write(dir.path().join("docs/global-auth/BOOTSTRAP.md"), "# auth").unwrap();
        fs::create_dir_all(dir.path().join(".claude-plugin")).unwrap();
        fs::write(
            dir.path().join(".claude-plugin/plugin.json"),
            r#"{"bootstrapDocs":[{"path":"./docs/global-auth"}]}"#,
        )
        .unwrap();

        let items = discover_manifestless_source(dir.path(), Some("demo")).unwrap();

        assert_eq!(items.len(), 2);
        assert!(
            items
                .iter()
                .any(|item| item.id.kind == ItemKind::Skill && item.source_path == Path::new("."))
        );
        assert!(items.iter().any(|item| {
            item.id.kind == ItemKind::BootstrapDoc
                && item.source_path == Path::new("docs/global-auth")
        }));
    }

    #[test]
    fn manifestless_source_unions_conventions_with_manifest_declarations() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("agents")).unwrap();
        fs::create_dir_all(dir.path().join("skills/planning")).unwrap();
        fs::create_dir_all(dir.path().join("docs/global-auth")).unwrap();
        fs::write(dir.path().join("agents/coder.md"), "# coder").unwrap();
        fs::write(dir.path().join("skills/planning/SKILL.md"), "# planning").unwrap();
        fs::write(dir.path().join("docs/global-auth/BOOTSTRAP.md"), "# auth").unwrap();
        fs::create_dir_all(dir.path().join(".claude-plugin")).unwrap();
        fs::write(
            dir.path().join(".claude-plugin/plugin.json"),
            r#"{"bootstrapDocs":[{"path":"./docs/global-auth"}]}"#,
        )
        .unwrap();

        let items = discover_manifestless_source(dir.path(), Some("demo")).unwrap();

        assert_eq!(items.len(), 3);
        assert!(items.iter().any(|item| {
            item.id.kind == ItemKind::Agent && item.source_path == Path::new("agents/coder.md")
        }));
        assert!(items.iter().any(|item| {
            item.id.kind == ItemKind::Skill && item.source_path == Path::new("skills/planning")
        }));
        assert!(items.iter().any(|item| {
            item.id.kind == ItemKind::BootstrapDoc
                && item.source_path == Path::new("docs/global-auth")
        }));
    }

    #[test]
    fn duplicate_names_keep_shallowest_convention_item() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("skills/foo")).unwrap();
        fs::create_dir_all(dir.path().join("nested/skills/foo")).unwrap();
        fs::write(dir.path().join("skills/foo/SKILL.md"), "# shallow").unwrap();
        fs::write(dir.path().join("nested/skills/foo/SKILL.md"), "# nested").unwrap();

        let items = discover_manifestless_source(dir.path(), Some("demo")).unwrap();

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id.kind, ItemKind::Skill);
        assert_eq!(items[0].id.name.as_str(), "foo");
        assert_eq!(items[0].source_path, PathBuf::from("skills/foo"));
    }

    #[test]
    fn manifestless_source_discovers_top_level_canonical_agents_and_skills() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("agents")).unwrap();
        fs::create_dir_all(dir.path().join("skills/review")).unwrap();
        fs::write(dir.path().join("agents/reviewer.md"), "# reviewer").unwrap();
        fs::write(dir.path().join("skills/review/SKILL.md"), "# review").unwrap();

        let items = discover_manifestless_source(dir.path(), Some("demo")).unwrap();

        assert_eq!(items.len(), 2);
        assert!(items.iter().any(|item| {
            item.id.kind == ItemKind::Agent && item.source_path == Path::new("agents/reviewer.md")
        }));
        assert!(items.iter().any(|item| {
            item.id.kind == ItemKind::Skill && item.source_path == Path::new("skills/review")
        }));
    }

    #[test]
    fn convention_walk_finds_items_at_max_depth_and_skips_deeper_items() {
        let dir = TempDir::new().unwrap();
        let at_limit = ["a", "b", "c", "d", AGENTS_DIR_NAME].join("/");
        let beyond_limit = ["a", "b", "c", "d", "e", AGENTS_DIR_NAME].join("/");
        fs::create_dir_all(dir.path().join(&at_limit)).unwrap();
        fs::create_dir_all(dir.path().join(&beyond_limit)).unwrap();
        fs::write(dir.path().join(&at_limit).join("found.md"), "# found").unwrap();
        fs::write(
            dir.path().join(&beyond_limit).join("skipped.md"),
            "# skipped",
        )
        .unwrap();

        let items = discover_manifestless_source(dir.path(), Some("demo")).unwrap();

        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0].source_path,
            PathBuf::from("a/b/c/d/agents/found.md")
        );
    }

    #[test]
    fn explicit_claude_subpath_with_claude_dialect_imports_real_claude_layout() {
        use crate::config::SkillOverlay;
        use crate::diagnostic::DiagnosticCollector;
        use crate::dialect::Dialect;
        use crate::resolve::apply_subpath;
        use crate::staging::stage_rooted_source;
        use crate::types::{RenameMap, SourceName, SourceSubpath};
        use indexmap::IndexMap;

        let checkout = TempDir::new().unwrap();
        let claude_root = checkout.path().join(".claude");
        fs::create_dir_all(claude_root.join("agents")).unwrap();
        fs::create_dir_all(claude_root.join("skills/research")).unwrap();
        fs::write(
            claude_root.join("agents/reviewer.md"),
            "---\ndescription: reviewer\n---\n# reviewer",
        )
        .unwrap();
        fs::write(
            claude_root.join("skills/research/SKILL.md"),
            "---\ndescription: research\n---\n# research",
        )
        .unwrap();

        let source_name = SourceName::new("foreign");
        let subpath = SourceSubpath::new(".claude").unwrap();
        let rooted = apply_subpath(&source_name, checkout.path(), Some(&subpath)).unwrap();
        let staging = TempDir::new().unwrap();
        let mut diag = DiagnosticCollector::new();
        let staged = stage_rooted_source(
            &source_name,
            rooted,
            Dialect::Claude,
            &IndexMap::<String, SkillOverlay>::new(),
            &RenameMap::new(),
            staging.path(),
            &mut diag,
        )
        .unwrap();

        let items = discover_resolved_source(&staged.package_root, Some("foreign")).unwrap();

        assert_eq!(items.len(), 2);
        assert!(items.iter().any(|item| {
            item.id.kind == ItemKind::Agent && item.source_path == Path::new("agents/reviewer.md")
        }));
        assert!(items.iter().any(|item| {
            item.id.kind == ItemKind::Skill && item.source_path == Path::new("skills/research")
        }));
    }

    #[test]
    fn fallback_walk_finds_nested_convention_dirs_and_skips_dot_dirs() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("sub/agents")).unwrap();
        fs::create_dir_all(dir.path().join("a/b/skills/bar")).unwrap();
        fs::create_dir_all(dir.path().join(".claude/agents")).unwrap();
        fs::write(dir.path().join("sub/agents/foo.md"), "# agent").unwrap();
        fs::write(dir.path().join("a/b/skills/bar/SKILL.md"), "# skill").unwrap();
        fs::write(dir.path().join(".claude/agents/hidden.md"), "# hidden").unwrap();

        let items = discover_manifestless_source(dir.path(), Some("demo")).unwrap();
        assert_eq!(items.len(), 2);
        assert!(items.iter().any(|item| {
            item.id.kind == ItemKind::Agent && item.source_path == Path::new("sub/agents/foo.md")
        }));
        assert!(items.iter().any(|item| {
            item.id.kind == ItemKind::Skill && item.source_path == Path::new("a/b/skills/bar")
        }));
        assert!(
            !items
                .iter()
                .any(|item| item.source_path == Path::new(".claude/agents/hidden.md"))
        );
    }

    #[test]
    fn explicit_claude_subpath_root_imports_inner_convention_dirs() {
        let dir = TempDir::new().unwrap();
        let claude_root = dir.path().join(".claude");
        fs::create_dir_all(claude_root.join("agents")).unwrap();
        fs::create_dir_all(claude_root.join("skills/research")).unwrap();
        fs::write(claude_root.join("agents/reviewer.md"), "# reviewer").unwrap();
        fs::write(claude_root.join("skills/research/SKILL.md"), "# research").unwrap();

        let items = discover_manifestless_source(&claude_root, Some("foreign")).unwrap();
        assert_eq!(items.len(), 2);
        assert!(
            items
                .iter()
                .any(|item| item.source_path == Path::new("agents/reviewer.md"))
        );
        assert!(
            items
                .iter()
                .any(|item| item.source_path == Path::new("skills/research"))
        );
    }

    #[test]
    fn conventional_root_skill_does_not_override_conventional_items() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("mars.toml"),
            "[package]\nname='demo'\nversion='0.1.0'\n",
        )
        .unwrap();
        fs::write(dir.path().join("SKILL.md"), "# root").unwrap();
        fs::create_dir_all(dir.path().join("skills/planning")).unwrap();
        fs::write(dir.path().join("skills/planning/SKILL.md"), "# planning").unwrap();

        let items = discover_resolved_source(dir.path(), Some("demo")).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].source_path, PathBuf::from("skills/planning"));
    }

    #[test]
    fn conventional_root_skill_survives_bootstrap_only_discovery() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("mars.toml"),
            "[package]\nname='demo'\nversion='0.1.0'\n",
        )
        .unwrap();
        fs::write(dir.path().join("SKILL.md"), "# root").unwrap();
        fs::create_dir_all(dir.path().join("bootstrap/global-auth")).unwrap();
        fs::write(
            dir.path().join("bootstrap/global-auth/BOOTSTRAP.md"),
            "# auth",
        )
        .unwrap();

        let items = discover_resolved_source(dir.path(), Some("demo")).unwrap();

        assert_eq!(items.len(), 2);
        assert!(items.iter().any(|item| {
            item.id.kind == ItemKind::Skill
                && item.id.name.as_str() == "demo"
                && item.source_path == Path::new(".")
        }));
        assert!(items.iter().any(|item| {
            item.id.kind == ItemKind::BootstrapDoc
                && item.id.name.as_str() == "global-auth"
                && item.source_path == Path::new("bootstrap/global-auth")
        }));
    }

    #[test]
    fn manifest_declared_skill_path_is_honored() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("top-level")).unwrap();
        fs::create_dir_all(dir.path().join("plugins/deep-skill")).unwrap();
        fs::write(dir.path().join("top-level/SKILL.md"), "# top").unwrap();
        fs::write(dir.path().join("plugins/deep-skill/SKILL.md"), "# deep").unwrap();
        fs::create_dir_all(dir.path().join(".claude-plugin")).unwrap();
        fs::write(
            dir.path().join(".claude-plugin/plugin.json"),
            r#"{"skills":[{"path":"./plugins/deep-skill"}]}"#,
        )
        .unwrap();

        let items = discover_manifestless_source(dir.path(), Some("demo")).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].source_path, PathBuf::from("plugins/deep-skill"));
    }

    #[test]
    fn fallback_dedupes_overlapping_manifest_and_container_paths() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("skills/planning")).unwrap();
        fs::write(dir.path().join("skills/planning/SKILL.md"), "# skill").unwrap();
        fs::create_dir_all(dir.path().join(".claude-plugin")).unwrap();
        fs::write(
            dir.path().join(".claude-plugin/plugin.json"),
            r#"{"skills":[{"path":"./skills/planning"}]}"#,
        )
        .unwrap();

        let items = discover_manifestless_source(dir.path(), Some("demo")).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].source_path, PathBuf::from("skills/planning"));
    }

    #[test]
    fn manifest_ignores_nested_metadata_agent_keys() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("agents")).unwrap();
        fs::write(dir.path().join("agents/reviewer.md"), "# reviewer").unwrap();
        fs::create_dir_all(dir.path().join(".claude-plugin")).unwrap();
        fs::write(
            dir.path().join(".claude-plugin/plugin.json"),
            r#"{"agents":[{"path":"./agents/reviewer.md"}],"metadata":{"agents":[{"path":"./ignore.md"}]}}"#,
        )
        .unwrap();

        let items = discover_manifestless_source(dir.path(), Some("demo")).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].source_path, PathBuf::from("agents/reviewer.md"));
    }

    #[test]
    fn fallback_manifest_declares_bootstrap_docs() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("docs/global-auth")).unwrap();
        fs::write(dir.path().join("docs/global-auth/BOOTSTRAP.md"), "# auth").unwrap();
        fs::create_dir_all(dir.path().join(".claude-plugin")).unwrap();
        fs::write(
            dir.path().join(".claude-plugin/plugin.json"),
            r#"{"bootstrapDocs":[{"path":"./docs/global-auth"}]}"#,
        )
        .unwrap();

        let items = discover_manifestless_source(dir.path(), Some("demo")).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id.kind, ItemKind::BootstrapDoc);
        assert_eq!(items[0].id.name.as_str(), "global-auth");
        assert_eq!(items[0].source_path, PathBuf::from("docs/global-auth"));
    }

    #[test]
    fn fallback_manifest_declares_bootstrap_container() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("bootstrap/setup")).unwrap();
        fs::write(dir.path().join("bootstrap/setup/BOOTSTRAP.md"), "# setup").unwrap();
        fs::create_dir_all(dir.path().join(".claude-plugin")).unwrap();
        fs::write(
            dir.path().join(".claude-plugin/plugin.json"),
            r#"{"bootstrap_docs":["./bootstrap"]}"#,
        )
        .unwrap();

        let items = discover_manifestless_source(dir.path(), Some("demo")).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id.kind, ItemKind::BootstrapDoc);
        assert_eq!(items[0].id.name.as_str(), "setup");
        assert_eq!(items[0].source_path, PathBuf::from("bootstrap/setup"));
    }

    #[test]
    fn nested_bootstrap_dir_is_discovered() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("nested/bootstrap/setup")).unwrap();
        fs::create_dir_all(dir.path().join("nested/bootstrap/.hidden")).unwrap();
        fs::write(
            dir.path().join("nested/bootstrap/setup/BOOTSTRAP.md"),
            "# setup",
        )
        .unwrap();
        fs::write(
            dir.path().join("nested/bootstrap/.hidden/BOOTSTRAP.md"),
            "# hidden",
        )
        .unwrap();

        let items = discover_manifestless_source(dir.path(), Some("demo")).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id.kind, ItemKind::BootstrapDoc);
        assert_eq!(items[0].id.name.as_str(), "setup");
        assert_eq!(
            items[0].source_path,
            PathBuf::from("nested/bootstrap/setup")
        );
    }

    #[test]
    fn fallback_manifest_declared_escape_is_rejected() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join(".claude-plugin")).unwrap();
        fs::write(
            dir.path().join(".claude-plugin/plugin.json"),
            r#"{"skills":[{"path":"./../escape"}]}"#,
        )
        .unwrap();

        let err = discover_manifestless_source(dir.path(), Some("demo")).unwrap_err();
        assert!(matches!(err, MarsError::ManifestDeclaredPathEscape { .. }));
    }

    #[test]
    fn discover_installed_reads_frontmatter() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("agents")).unwrap();
        fs::create_dir_all(dir.path().join("skills/planning")).unwrap();
        fs::write(
            dir.path().join("agents/coder.md"),
            "---\nname: coder\ndescription: test\nskills: [planning]\n---\n# Coder\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("skills/planning/SKILL.md"),
            "---\nname: planning\ndescription: test\n---\n# Planning\n",
        )
        .unwrap();

        let state = discover_installed(dir.path()).unwrap();
        assert_eq!(state.agents.len(), 1);
        assert_eq!(state.skills.len(), 1);
        assert_eq!(state.agents[0].skill_refs, vec!["planning"]);
        assert_eq!(
            state.skills[0].frontmatter_name.as_deref(),
            Some("planning")
        );
    }
}
