//! Filesystem discovery for package-provided agents, skills, and bootstrap docs.
//!
//! Discovery is intentionally convention-based: the bounded walk `mars-schema` owns finds
//! directories named `bootstrap` (and, for Agents and skills, `agents` and `skills`) instead of
//! carrying harness-specific blocklists. That walk skips hidden dot-directories, so generated
//! harness surfaces like `.claude/` and tool caches like `.git/` are not imported unless a
//! dependency explicitly roots discovery there with `subpath`. This module keeps no copy of its
//! skip list, depth, or generated-tree rule.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

use serde_json::Value;

use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::lock::ItemId;
use crate::package_prepare::DiskTreeSource;
use mars_schema::discover::{Excluded, excluded, walk_dirs};
use mars_schema::item::ItemKind;
use mars_schema::names::ItemName;

pub(crate) const SOURCE_MANIFEST_PATHS: &[&str] = &[
    ".claude-plugin/plugin.json",
    ".claude-plugin/marketplace.json",
];
const HOOKS_DIR_NAME: &str = "hooks";
const BOOTSTRAP_DIR_NAME: &str = "bootstrap";
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

/// The name selection knows a hook by: its `name`, else its directory's. A `hook.toml` that
/// does not parse is known by its directory.
pub(crate) fn hook_slug(hook_toml: &str, dir_name: &str) -> String {
    toml::from_str::<toml::Value>(hook_toml)
        .ok()
        .and_then(|value| value.get("name")?.as_str().map(str::to_owned))
        .unwrap_or_else(|| dir_name.to_owned())
}

fn scan_hook_dir(
    package_root: &Path,
    relative_root: &Path,
    items: &mut Vec<DiscoveredItem>,
    visited: &mut HashSet<PathBuf>,
    diag: &mut DiagnosticCollector,
) -> Result<(), MarsError> {
    if relative_root != Path::new(HOOKS_DIR_NAME) {
        return Ok(());
    }
    for path in discover_hook_directories(package_root, diag)? {
        let rel = relative_to(package_root, &path)?;
        if !visited.insert(rel.clone()) {
            continue;
        }
        let dir_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if dir_name.starts_with('.') {
            continue;
        }
        let raw = std::fs::read_to_string(path.join("hook.toml"))?;
        let name = hook_slug(&raw, dir_name);
        items.push(DiscoveredItem {
            id: ItemId {
                kind: ItemKind::Hook,
                name: ItemName::from(name),
            },
            source_path: rel,
        });
    }
    Ok(())
}

/// Return package-root hook directories in stable order.
///
/// Hooks are deliberately a package-root-only convention. Unlike agents and
/// skills, nested `hooks/` containers are not independent package layers.
pub(crate) fn discover_hook_directories(
    package_root: &Path,
    diag: &mut DiagnosticCollector,
) -> Result<Vec<PathBuf>, MarsError> {
    if generated_plugin_tree(package_root, diag) {
        return Ok(Vec::new());
    }
    let hooks_dir = package_root.join(HOOKS_DIR_NAME);
    if !hooks_dir.is_dir() {
        return Ok(Vec::new());
    }
    if generated_plugin_tree(&hooks_dir, diag) {
        return Ok(Vec::new());
    }
    let mut hooks = Vec::new();
    for path in read_dir_paths_sorted(&hooks_dir)? {
        if !path.is_dir()
            || !path.join("hook.toml").is_file()
            || path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with('.'))
            || generated_plugin_tree(&path, diag)
        {
            continue;
        }
        hooks.push(path);
    }
    Ok(hooks)
}

fn scan_bootstrap_dir(
    package_root: &Path,
    relative_root: &Path,
    items: &mut Vec<DiscoveredItem>,
    visited: &mut HashSet<PathBuf>,
    diag: &mut DiagnosticCollector,
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
        if generated_plugin_tree(&path, diag) {
            continue;
        }
        let rel = relative_to(package_root, &path)?;
        register_bootstrap_doc(package_root, &rel, items, visited)?;
    }

    Ok(())
}

fn scan_declared_bootstrap_path(
    package_root: &Path,
    tree: &DiskTreeSource,
    declared_path: &Path,
    items: &mut Vec<DiscoveredItem>,
    diag: &mut DiagnosticCollector,
) -> Result<(), MarsError> {
    let mut visited = HashSet::new();
    let candidate = package_root.join(declared_path);
    // A declared file is judged by the directory it sits in.
    let judged = if candidate.is_dir() {
        declared_path
    } else {
        declared_path.parent().unwrap_or(Path::new(""))
    };
    match excluded(tree, &tree_path(judged)) {
        None => {}
        Some(Excluded::PluginDir) => return Ok(()),
        Some(Excluded::Generated(dir)) => {
            crate::fs::report_generated_tree(&package_root.join(dir), diag);
            return Ok(());
        }
    }
    if candidate.join("BOOTSTRAP.md").is_file() {
        register_bootstrap_doc(package_root, declared_path, items, &mut visited)?;
    } else if candidate
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "BOOTSTRAP.md")
        && candidate.is_file()
        && let Some(parent) = declared_path.parent()
    {
        register_bootstrap_doc(package_root, parent, items, &mut visited)?;
    } else if candidate.is_dir() {
        scan_bootstrap_dir(package_root, declared_path, items, &mut visited, diag)?;
    }
    Ok(())
}

/// A relative path as the `/`-separated name `mars-schema` trees use.
fn tree_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
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

/// Bootstrap docs a plugin manifest declares with `bootstrapDocs`.
fn declared_bootstrap_items(
    package_root: &Path,
    tree: &DiskTreeSource,
    source_name: &str,
    diag: &mut DiagnosticCollector,
) -> Result<Vec<DiscoveredItem>, MarsError> {
    let mut items = Vec::new();
    for declared_path in declared_bootstrap_paths(package_root, source_name)? {
        scan_declared_bootstrap_path(package_root, tree, &declared_path, &mut items, diag)?;
    }
    Ok(dedupe_items_by_path(items))
}

/// Discover only binary-owned bootstrap document surfaces from an authored tree: the
/// `bootstrap/` containers the schema crate's bounded walk reaches, grounded to the shallowest
/// layer, plus the documents a plugin manifest declares.
pub(crate) fn discover_bootstrap_surfaces(
    package_root: &Path,
    diag: &mut DiagnosticCollector,
) -> Result<Vec<DiscoveredItem>, MarsError> {
    if !package_root.is_dir() || generated_plugin_tree(package_root, diag) {
        return Ok(Vec::new());
    }
    let tree = DiskTreeSource::new(package_root)?;
    let mut containers = Vec::new();
    let generated = walk_dirs(&tree, |dir| {
        if dir.rsplit('/').next() == Some(BOOTSTRAP_DIR_NAME) {
            containers.push(PathBuf::from(dir));
        }
    });
    if let Some(error) = tree.take_error() {
        return Err(error);
    }
    for dir in generated {
        crate::fs::report_generated_tree(&package_root.join(dir), diag);
    }
    let mut layered = Vec::new();
    let mut scratch = Vec::new();
    let mut visited = HashSet::new();
    for container in containers {
        scan_bootstrap_dir(package_root, &container, &mut scratch, &mut visited, diag)?;
        push_layered_items(&mut layered, &mut scratch, convention_layer(&container));
    }
    let mut items: Vec<_> = ground_items_to_shallowest_layer(layered)
        .into_iter()
        .map(|item| item.item)
        .collect();
    items.extend(declared_bootstrap_items(
        package_root,
        &tree,
        "unknown-source",
        diag,
    )?);
    finalize_items("unknown-source", items)
}

/// Discover the item kinds `mars-schema` does not own: hooks and bootstrap documents. Agents and
/// skills come from `Prepared` (see [`package_items`]); nothing here selects them. `parsed_hooks`
/// are hook definitions the compiler already read strictly.
pub(crate) fn discover_binary_owned_items(
    package_root: &Path,
    source_name: Option<&str>,
    parsed_hooks: Option<&[DiscoveredItem]>,
    diag: &mut DiagnosticCollector,
) -> Result<Vec<DiscoveredItem>, MarsError> {
    if generated_plugin_tree(package_root, diag) {
        return Ok(Vec::new());
    }
    let mut items = discover_bootstrap_surfaces(package_root, diag)?;
    match parsed_hooks {
        Some(parsed) => items.extend_from_slice(parsed),
        None => {
            let mut visited_hooks = HashSet::new();
            scan_hook_dir(
                package_root,
                Path::new(HOOKS_DIR_NAME),
                &mut items,
                &mut visited_hooks,
                diag,
            )?;
        }
    }
    finalize_items(source_name.unwrap_or("unknown-source"), items)
}

/// The item for a prepared Agent or skill, located at `path`: the definition's output path in
/// the materialized layout, or its source path in the authored one.
pub(crate) fn definition_item(
    definition: &mars_schema::prepare::PreparedDefinition,
    path: &str,
) -> DiscoveredItem {
    let (kind, source_path) = match definition.kind {
        mars_schema::item::DefinitionKind::Agent => (ItemKind::Agent, PathBuf::from(path)),
        mars_schema::item::DefinitionKind::Skill => (
            ItemKind::Skill,
            match Path::new(path).parent() {
                Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
                _ => PathBuf::from("."),
            },
        ),
    };
    DiscoveredItem {
        id: ItemId {
            kind,
            name: definition.slug.clone().into(),
        },
        source_path,
    }
}

/// Every item of a materialized package: Agents and skills from its prepared result, and the
/// binary-owned kinds from the tree.
pub(crate) fn package_items(
    package_root: &Path,
    source_name: Option<&str>,
    prepared: &mars_schema::prepare::Prepared,
    parsed_hooks: Option<&[DiscoveredItem]>,
    diag: &mut DiagnosticCollector,
) -> Result<Vec<DiscoveredItem>, MarsError> {
    let mut items = discover_binary_owned_items(package_root, source_name, parsed_hooks, diag)?;
    items.extend(
        prepared
            .definitions
            .iter()
            .map(|definition| definition_item(definition, &definition.output_path)),
    );
    finalize_items(source_name.unwrap_or("unknown-source"), items)
}

fn generated_plugin_tree(path: &Path, diag: &mut DiagnosticCollector) -> bool {
    let generated = crate::plugin_metadata::has_valid_plugin_sentinel(path);
    if generated {
        crate::fs::report_generated_tree(path, diag);
    }
    generated
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

fn declared_bootstrap_paths(
    package_root: &Path,
    source_name: &str,
) -> Result<Vec<PathBuf>, MarsError> {
    let mut declared = Vec::new();
    for manifest in SOURCE_MANIFEST_PATHS {
        let path = package_root.join(manifest);
        if !path.is_file() {
            continue;
        }
        let content = std::fs::read_to_string(&path)?;
        let json: Value = serde_json::from_str(&content).map_err(|e| MarsError::Source {
            source_name: source_name.to_string(),
            message: format!("failed to parse plugin manifest `{}`: {e}", path.display()),
        })?;
        declared.extend(parse_declared_bootstrap_paths(&json));
    }

    let mut resolved: Vec<PathBuf> = Vec::new();
    for raw in declared {
        if !raw.starts_with("./") {
            continue;
        }
        let normalized = normalize_manifest_declared_path(&raw).ok_or_else(|| {
            MarsError::ManifestDeclaredPathEscape {
                source_name: source_name.to_string(),
                manifest_path: raw.display().to_string(),
                package_root: package_root.to_path_buf(),
            }
        })?;
        let candidate = package_root.join(&normalized);
        if !candidate.exists() {
            return Err(MarsError::ManifestDeclaredPathMissing {
                source_name: source_name.to_string(),
                manifest_path: raw.display().to_string(),
                package_root: package_root.to_path_buf(),
            });
        }
        let canonical = dunce::canonicalize(&candidate).map_err(|_| {
            MarsError::ManifestDeclaredPathMissing {
                source_name: source_name.to_string(),
                manifest_path: raw.display().to_string(),
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
                manifest_path: raw.display().to_string(),
                package_root: package_root.to_path_buf(),
            });
        }
        let rel = relative_to(package_root, &candidate)?;
        if !resolved.contains(&rel) {
            resolved.push(rel);
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

fn parse_declared_bootstrap_paths(json: &Value) -> Vec<PathBuf> {
    let Some(map) = json.as_object() else {
        return Vec::new();
    };
    let mut declared = Vec::new();
    for key in MANIFEST_BOOTSTRAP_KEYS {
        if let Some(value) = map.get(*key) {
            collect_declared_paths_from_value(value, &mut declared);
        }
    }
    declared
}

fn collect_declared_paths_from_value(value: &Value, declared: &mut Vec<PathBuf>) {
    match value {
        Value::String(path) => declared.push(PathBuf::from(path)),
        Value::Array(values) => {
            for child in values {
                collect_declared_paths_from_value(child, declared);
            }
        }
        Value::Object(map) => {
            if let Some(path) = map.get("path").and_then(|value| value.as_str()) {
                declared.push(PathBuf::from(path));
            }
        }
        _ => {}
    }
}

fn sort_items(items: &mut [DiscoveredItem]) {
    items.sort_by(|a, b| {
        a.id.cmp(&b.id)
            .then_with(|| a.source_path.cmp(&b.source_path))
    });
}

#[cfg(test)]
mod tests;
