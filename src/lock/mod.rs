use std::collections::BTreeMap;
use std::collections::{HashMap, HashSet};
use std::path::Path;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::diagnostic::Diagnostic;
use crate::error::{LockError, MarsError};
use crate::models::ModelAlias;
use crate::types::{
    CommitHash, ContentHash, DestPath, SourceId, SourceName, SourceOrigin, SourceSubpath, SourceUrl,
};

/// The complete lock file — ownership registry for all managed items.
///
/// Schema version 3: items are keyed by logical identity ("kind/name"), and each item
/// carries a list of per-output records (one per target root materialization).
///
/// TOML format, deterministically ordered (sorted keys) for clean git diffs.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LockFile {
    /// Schema version. Current version is 3.
    pub version: u32,
    #[serde(default)]
    pub dependencies: IndexMap<SourceName, LockedSource>,
    /// Logical items keyed by "kind/name" identity string.
    #[serde(default)]
    pub items: IndexMap<String, LockedItemV2>,
    /// Config entries installed by mars sync, keyed by target root and entry key.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub config_entries: BTreeMap<String, BTreeMap<String, ConfigEntryRecord>>,
    /// Dependency model alias winners (declaration-order merged, dependency-only).
    #[serde(default)]
    pub dependency_model_aliases: IndexMap<String, ModelAlias>,
}

/// Custom `Deserialize` for `LockFile`, delegated to the current wire type.
/// Version validation is performed by [`load`]; direct deserialization expects the
/// current schema shape.
impl<'de> serde::Deserialize<'de> for LockFile {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = LockFileWire::deserialize(deserializer)?;
        Ok(LockFile {
            version: wire.version,
            dependencies: wire.dependencies,
            items: wire.items,
            config_entries: wire.config_entries,
            dependency_model_aliases: wire.dependency_model_aliases,
        })
    }
}

impl LockFile {
    /// Create a new empty lock file with the current schema version.
    pub fn empty() -> Self {
        LockFile {
            version: LOCK_VERSION,
            dependencies: IndexMap::new(),
            items: IndexMap::new(),
            config_entries: BTreeMap::new(),
            dependency_model_aliases: IndexMap::new(),
        }
    }

    /// Look up a locked item by its output dest_path, returning a flat [`LockedItem`] view.
    ///
    /// Searches across all items and their output records. Returns the first match.
    pub fn find_by_dest_path(&self, dest_path: &DestPath) -> Option<LockedItem> {
        for item_v2 in self.items.values() {
            for output in &item_v2.outputs {
                if crate::target::dest_paths_equivalent(
                    output.dest_path.as_str(),
                    dest_path.as_str(),
                ) && let Some(installed_checksum) = output.installed_checksum()
                {
                    return Some(LockedItem {
                        source: item_v2.source.clone(),
                        kind: item_v2.kind,
                        version: item_v2.version.clone(),
                        source_checksum: item_v2.source_checksum.clone(),
                        installed_checksum: installed_checksum.clone(),
                        dest_path: output.dest_path.clone(),
                    });
                }
            }
        }
        None
    }

    /// Check if any output record has the given dest_path.
    pub fn contains_dest_path(&self, dest_path: &DestPath) -> bool {
        self.items.values().any(|item| {
            item.outputs.iter().any(|o| {
                crate::target::dest_paths_equivalent(o.dest_path.as_str(), dest_path.as_str())
            })
        })
    }

    /// Iterate all output dest_paths across all items.
    pub fn all_output_dest_paths(&self) -> impl Iterator<Item = &DestPath> {
        self.items
            .values()
            .flat_map(|item| item.outputs.iter().map(|o| &o.dest_path))
    }

    /// Dest paths previously managed under a specific target root.
    pub fn output_dest_paths_for_target(&self, target_root: &str) -> HashSet<String> {
        self.items
            .values()
            .flat_map(|item| item.outputs.iter())
            .filter(|output| output.target_root == target_root)
            .map(|output| output.dest_path.to_string())
            .collect()
    }

    /// Whether the lock records ownership of `dest_path` under `target_root`.
    pub fn contains_output(&self, target_root: &str, dest_path: &str) -> bool {
        self.items.values().any(|item| {
            item.outputs.iter().any(|output| {
                output.target_root == target_root
                    && crate::target::dest_paths_equivalent(output.dest_path.as_str(), dest_path)
            })
        })
    }

    /// The installed checksum claimed for `dest_path` under `target_root`.
    ///
    /// Pending-deletion records intentionally return `None`: they authorize a
    /// removal retry, but do not authorize treating whatever is currently at
    /// the path as Mars-installed content.
    pub(crate) fn installed_checksum_for_output(
        &self,
        target_root: &str,
        dest_path: &str,
    ) -> Option<&ContentHash> {
        self.items.values().find_map(|item| {
            item.outputs.iter().find_map(|output| {
                (output.target_root == target_root
                    && crate::target::dest_paths_equivalent(output.dest_path.as_str(), dest_path))
                .then(|| output.installed_checksum())
                .flatten()
            })
        })
    }

    /// Flat view of canonical `.mars` outputs only.
    pub fn canonical_flat_items(&self) -> Vec<(DestPath, LockedItem)> {
        self.flat_items_for_target(CANONICAL_TARGET_ROOT)
    }

    /// Flat view of outputs materialized under `target_root`.
    pub fn flat_items_for_target(&self, target_root: &str) -> Vec<(DestPath, LockedItem)> {
        self.items
            .values()
            .flat_map(|item_v2| {
                item_v2.outputs.iter().filter_map(|output| {
                    if output.target_root != target_root {
                        return None;
                    }
                    let installed_checksum = output.installed_checksum()?;
                    Some((
                        output.dest_path.clone(),
                        LockedItem {
                            source: item_v2.source.clone(),
                            kind: item_v2.kind,
                            version: item_v2.version.clone(),
                            source_checksum: item_v2.source_checksum.clone(),
                            installed_checksum: installed_checksum.clone(),
                            dest_path: output.dest_path.clone(),
                        },
                    ))
                })
            })
            .collect()
    }
}

/// Ephemeral lookup index for lock files.
///
/// `LockFile` preserves the persisted v2 shape. Build this short-lived index
/// at hot call sites that need repeated output-path lookups.
pub struct LockIndex<'a> {
    lock: &'a LockFile,
    by_output: HashMap<(String, String), (&'a str, usize)>,
}

impl<'a> LockIndex<'a> {
    pub fn new(lock: &'a LockFile) -> Self {
        let mut by_output = HashMap::new();
        for (key, item) in &lock.items {
            for (idx, output) in item.outputs.iter().enumerate() {
                let normalized_dest = normalize_dest_path(output.dest_path.as_str());
                by_output.insert(
                    (output.target_root.clone(), normalized_dest),
                    (key.as_str(), idx),
                );
            }
        }

        Self { lock, by_output }
    }

    /// Look up a locked output by target root + dest_path, returning a flat [`LockedItem`] view.
    pub fn find_output(&self, target_root: &str, dest_path: &DestPath) -> Option<LockedItem> {
        let (item_key, output_idx) = *self.by_output.get(&(
            target_root.to_string(),
            normalize_dest_path(dest_path.as_str()),
        ))?;
        self.locked_item_for(item_key, output_idx)
    }

    fn item_for_output(
        &self,
        target_root: &str,
        dest_path: &DestPath,
    ) -> Option<(&'a str, &'a LockedItemV2, &'a OutputRecord)> {
        let (item_key, output_idx) = *self.by_output.get(&(
            target_root.to_string(),
            normalize_dest_path(dest_path.as_str()),
        ))?;
        let item = self.lock.items.get(item_key)?;
        let output = item.outputs.get(output_idx)?;
        Some((item_key, item, output))
    }

    /// Whether any output is recorded for `target_root + dest_path`.
    pub fn contains_output(&self, target_root: &str, dest_path: &DestPath) -> bool {
        self.by_output.contains_key(&(
            target_root.to_string(),
            normalize_dest_path(dest_path.as_str()),
        ))
    }

    /// Whether an installed (not pending-deletion) output is recorded for this path.
    pub(crate) fn contains_installed_output(
        &self,
        target_root: &str,
        dest_path: &DestPath,
    ) -> bool {
        self.item_for_output(target_root, dest_path)
            .is_some_and(|(_, _, output)| output.installed_checksum().is_some())
    }

    fn locked_item_for(&self, item_key: &str, output_idx: usize) -> Option<LockedItem> {
        let item_v2 = self.lock.items.get(item_key)?;
        let output = item_v2.outputs.get(output_idx)?;
        let installed_checksum = output.installed_checksum()?;
        Some(LockedItem {
            source: item_v2.source.clone(),
            kind: item_v2.kind,
            version: item_v2.version.clone(),
            source_checksum: item_v2.source_checksum.clone(),
            installed_checksum: installed_checksum.clone(),
            dest_path: output.dest_path.clone(),
        })
    }
}

fn normalize_dest_path(s: &str) -> String {
    if cfg!(windows) {
        s.replace('\\', "/")
    } else {
        s.to_string()
    }
}

/// One resolved source in the lock.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LockedSource {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<SourceUrl>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subpath: Option<SourceSubpath>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<CommitHash>,
}

/// V2 locked item: one logical item with per-output records.
///
/// `source_checksum` is shared across all outputs (same source content).
/// Each `OutputRecord` has its own `installed_checksum` for divergence detection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LockedItemV2 {
    pub source: SourceName,
    pub kind: ItemKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub source_checksum: ContentHash,
    /// Per-output records: one per target root this item was materialized to.
    pub outputs: Vec<OutputRecord>,
}

/// A single path owned by Mars, with its lifecycle claim made explicit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OutputRecord {
    /// Target root this output belongs to (e.g., ".mars", ".claude").
    pub target_root: String,
    /// Relative path under the target root (e.g., "agents/coder.md").
    pub dest_path: DestPath,
    /// What authority this record currently asserts for the path.
    #[serde(flatten)]
    pub state: OutputState,
}

/// The lifecycle claim carried by an [`OutputRecord`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum OutputState {
    /// Mars confirms its installed bytes are present at the path.
    Installed { installed_checksum: ContentHash },
    /// Removal was not confirmed; retain path ownership solely to retry deletion.
    PendingDeletion,
}

impl OutputRecord {
    pub fn installed(
        target_root: String,
        dest_path: DestPath,
        installed_checksum: ContentHash,
    ) -> Self {
        Self {
            target_root,
            dest_path,
            state: OutputState::Installed { installed_checksum },
        }
    }

    pub fn pending_deletion(
        target_root: impl Into<String>,
        dest_path: impl Into<DestPath>,
    ) -> Self {
        Self {
            target_root: target_root.into(),
            dest_path: dest_path.into(),
            state: OutputState::PendingDeletion,
        }
    }

    pub fn installed_checksum(&self) -> Option<&ContentHash> {
        match &self.state {
            OutputState::Installed { installed_checksum } => Some(installed_checksum),
            OutputState::PendingDeletion => None,
        }
    }

    pub fn mark_installed(&mut self, installed_checksum: ContentHash) {
        self.state = OutputState::Installed { installed_checksum };
    }

    pub fn mark_pending_deletion(&mut self) {
        self.state = OutputState::PendingDeletion;
    }
}

/// Ownership record for one target-native config entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConfigEntryRecord {
    /// Canonical JSON for the exact post-substitution hook entry array.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emitted_json: Option<String>,
}

/// Flat view of a single installed item — used by diff, plan, and apply stages.
///
/// Constructed from [`LockedItemV2`] + one [`OutputRecord`]; preserves backward
/// compat with code that operates on per-dest-path records.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LockedItem {
    pub source: SourceName,
    pub kind: ItemKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub source_checksum: ContentHash,
    pub installed_checksum: ContentHash,
    pub dest_path: DestPath,
}

// Re-export ItemKind and ItemId from types — they're shared vocabulary,
// not lock-specific. This preserves `use crate::lock::ItemKind` compatibility.
pub use crate::types::{ItemId, ItemKind};

const LOCK_FILE: &str = "mars.lock";
/// Current lock file schema version.
const LOCK_VERSION: u32 = 3;
/// Canonical materialization root for `.mars/` apply outcomes.
pub const CANONICAL_TARGET_ROOT: &str = ".mars";

// ---------------------------------------------------------------------------
// Persisted wire formats.
// ---------------------------------------------------------------------------

/// Current wire format for Deserialize (mirrors `LockFile` but derives `Deserialize`).
#[derive(Deserialize)]
struct LockFileWire {
    version: u32,
    #[serde(default)]
    dependencies: IndexMap<SourceName, LockedSource>,
    #[serde(default)]
    items: IndexMap<String, LockedItemV2>,
    #[serde(default)]
    config_entries: BTreeMap<String, BTreeMap<String, ConfigEntryRecord>>,
    #[serde(default)]
    dependency_model_aliases: IndexMap<String, ModelAlias>,
}

/// Version 2 output records did not distinguish installed content from retry tombstones.
#[derive(Deserialize)]
struct OutputRecordV2 {
    target_root: String,
    dest_path: DestPath,
    installed_checksum: ContentHash,
}

#[derive(Deserialize)]
struct LockedItemV2Wire {
    source: SourceName,
    kind: ItemKind,
    #[serde(default)]
    version: Option<String>,
    source_checksum: ContentHash,
    outputs: Vec<OutputRecordV2>,
}

/// One-release v2 wire format. Delete this promotion after the release following
/// lock v3, alongside the #130 legacy-hook sweeps that depend on these records.
#[derive(Deserialize)]
struct LockFileV2Wire {
    #[allow(dead_code)]
    version: u32,
    #[serde(default)]
    dependencies: IndexMap<SourceName, LockedSource>,
    #[serde(default)]
    items: IndexMap<String, LockedItemV2Wire>,
    #[serde(default)]
    config_entries: BTreeMap<String, BTreeMap<String, ConfigEntryRecord>>,
    #[serde(default)]
    dependency_model_aliases: IndexMap<String, ModelAlias>,
}

// ---------------------------------------------------------------------------
// Load / write
// ---------------------------------------------------------------------------

/// Load the lock file from the given root directory.
///
/// Returns an empty current-version lock if the file is absent.
/// Version 2 is promoted in memory for one release; other older schemas fail
/// with actionable re-sync guidance.
pub fn load(root: &Path) -> Result<LockFile, MarsError> {
    let (lock, _) = load_with_diagnostics(root)?;
    Ok(lock)
}

/// Load lock for runtime alias commands (`models list/resolve`, launch bundle routing).
///
/// Legacy v2 lock files created before dependency aliases were moved into `mars.lock`
/// may omit `dependency_model_aliases` entirely. When dependency entries exist, runtime
/// alias consumers must fail closed so dependency alias authority is not silently treated
/// as empty.
pub fn load_for_runtime_aliases(root: &Path) -> Result<LockFile, MarsError> {
    let path = root.join(LOCK_FILE);
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(LockFile::empty()),
        Err(e) => return Err(LockError::Io(e).into()),
    };

    let value: toml::Value = toml::from_str(&content).map_err(|e| LockError::Corrupt {
        message: format!("failed to parse {}: {e}", path.display()),
    })?;

    let has_dependency_alias_field = value
        .as_table()
        .map(|table| table.contains_key("dependency_model_aliases"))
        .unwrap_or(false);

    let (lock, _) = load_with_diagnostics(root)?;

    if !has_dependency_alias_field && !lock.dependencies.is_empty() {
        return Err(LockError::Corrupt {
            message: format!(
                "legacy {} is missing `dependency_model_aliases` for dependency alias authority; run `{}` to update it",
                LOCK_FILE,
                crate::types::managed_cmd("mars sync")
            ),
        }
        .into());
    }

    Ok(lock)
}

/// Load the lock file and return any diagnostics produced while reading it.
///
/// Version 2 locks are promoted in memory so one-release cleanup bridges can
/// inspect their ownership records. Other version and schema failures are
/// returned as actionable lock errors.
pub fn load_with_diagnostics(root: &Path) -> Result<(LockFile, Vec<Diagnostic>), MarsError> {
    let path = root.join(LOCK_FILE);
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok((LockFile::empty(), Vec::new()));
        }
        Err(e) => return Err(LockError::Io(e).into()),
    };

    let value: toml::Value = toml::from_str(&content).map_err(|e| LockError::Corrupt {
        message: format!("failed to parse {}: {e}", path.display()),
    })?;
    let version = value
        .get("version")
        .and_then(toml::Value::as_integer)
        .ok_or_else(|| LockError::Corrupt {
            message: format!("{} has no integer lock version", path.display()),
        })?;
    match version {
        3 => {
            let wire: LockFileWire = value.try_into().map_err(|error| LockError::Corrupt {
                message: format!(
                    "failed to parse {} lock version {LOCK_VERSION}: {error}",
                    path.display()
                ),
            })?;
            Ok((
                LockFile {
                    version: wire.version,
                    dependencies: wire.dependencies,
                    items: wire.items,
                    config_entries: wire.config_entries,
                    dependency_model_aliases: wire.dependency_model_aliases,
                },
                Vec::new(),
            ))
        }
        2 => {
            let wire: LockFileV2Wire =
                value.try_into().map_err(|error| LockError::Corrupt {
                    message: format!("failed to parse {} lock version 2: {error}", path.display()),
                })?;
            Ok((promote_v2_lock(root, wire), Vec::new()))
        }
        older if older < i64::from(LOCK_VERSION) => Err(LockError::Corrupt {
            message: format!(
                "{} uses unsupported lock version {older}; remove it and run `{}` (only version 2 can be promoted to version {LOCK_VERSION})",
                path.display(),
                crate::types::managed_cmd("mars sync")
            ),
        }
        .into()),
        newer => Err(LockError::Corrupt {
            message: format!(
                "{} uses unsupported lock version {newer}; this Mars supports version {LOCK_VERSION}",
                path.display()
            ),
        }
        .into()),
    }
}

/// Cross the untyped v2 output boundary exactly once.
///
/// A v2 checksum could describe either installed content or a retry tombstone left
/// after failed removal. The output's actual disk shape selects the canonical file
/// or directory hash. Only matching regular content is promoted as installed;
/// every other path retains deletion authority without asserting ghost content.
fn promote_v2_lock(root: &Path, wire: LockFileV2Wire) -> LockFile {
    let items = wire
        .items
        .into_iter()
        .map(|(key, item)| {
            let outputs = item
                .outputs
                .into_iter()
                .map(|output| {
                    let path = root
                        .join(&output.target_root)
                        .join(output.dest_path.as_str());
                    let matches_disk = v2_output_checksum(&path)
                        .is_some_and(|checksum| checksum == output.installed_checksum.as_ref());
                    if matches_disk {
                        OutputRecord::installed(
                            output.target_root,
                            output.dest_path,
                            output.installed_checksum,
                        )
                    } else {
                        OutputRecord::pending_deletion(output.target_root, output.dest_path)
                    }
                })
                .collect();
            (
                key,
                LockedItemV2 {
                    source: item.source,
                    kind: item.kind,
                    version: item.version,
                    source_checksum: item.source_checksum,
                    outputs,
                },
            )
        })
        .collect();

    LockFile {
        version: LOCK_VERSION,
        dependencies: wire.dependencies,
        items,
        config_entries: wire.config_entries,
        dependency_model_aliases: wire.dependency_model_aliases,
    }
}

fn v2_output_checksum(path: &Path) -> Option<String> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        return None;
    }
    if file_type.is_file() {
        return std::fs::read(path)
            .ok()
            .map(|bytes| crate::hash::hash_bytes(&bytes));
    }
    if file_type.is_dir() {
        if !has_only_regular_file_entries(path) {
            return None;
        }
        return crate::hash::compute_dir_hash(path).ok();
    }
    None
}

/// Validate a directory without following links or opening entry contents.
fn has_only_regular_file_entries(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in entries {
        let Ok(entry) = entry else {
            return false;
        };
        let path = entry.path();
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            return false;
        };
        let file_type = metadata.file_type();
        if file_type.is_dir() {
            if !has_only_regular_file_entries(&path) {
                return false;
            }
        } else if !file_type.is_file() {
            return false;
        }
    }
    true
}

/// Write the lock file atomically to the given root directory (always current format).
pub fn write(root: &Path, lock: &LockFile) -> Result<(), MarsError> {
    let path = root.join(LOCK_FILE);
    let mut normalized = lock.clone();
    normalized.version = LOCK_VERSION;
    normalized.dependencies.sort_keys();
    normalized.items.sort_keys();
    normalized.dependency_model_aliases.sort_keys();

    let content = toml::to_string_pretty(&normalized).map_err(|e| LockError::Corrupt {
        message: format!("failed to serialize lock file: {e}"),
    })?;
    crate::fs::atomic_write_if_changed(&path, content.as_bytes()).map(|_| ())
}

// ---------------------------------------------------------------------------
// Build
// ---------------------------------------------------------------------------

/// Build a new lock file from resolved graph + apply results.
///
/// Constructs the lock file from the graph (source provenance) and
/// the apply outcomes (checksums). Items that were skipped, kept, or
/// merged retain their provenance from the graph. Removed items are excluded.
pub fn build(
    graph: &crate::resolve::ResolvedGraph,
    applied: &crate::sync::apply::ApplyResult,
    old_lock: &LockFile,
    config_entries: BTreeMap<String, BTreeMap<String, ConfigEntryRecord>>,
) -> Result<LockFile, MarsError> {
    use crate::sync::apply::ActionTaken;

    let mut dependencies = IndexMap::new();
    let mut items: IndexMap<String, LockedItemV2> = IndexMap::new();
    let old_lock_index = LockIndex::new(old_lock);

    for outcome in &applied.outcomes {
        match outcome.action {
            ActionTaken::Installed | ActionTaken::Updated => {
                let installed =
                    outcome
                        .installed_checksum
                        .as_ref()
                        .ok_or_else(|| LockError::Corrupt {
                            message: format!(
                                "missing checksum for write-producing action on {}",
                                outcome.dest_path
                            ),
                        })?;
                if checksum_is_empty(installed) {
                    return Err(LockError::Corrupt {
                        message: format!("empty installed_checksum for {}", outcome.dest_path),
                    }
                    .into());
                }

                let source =
                    outcome
                        .source_checksum
                        .as_ref()
                        .ok_or_else(|| LockError::Corrupt {
                            message: format!(
                                "missing source checksum for write-producing action on {}",
                                outcome.dest_path
                            ),
                        })?;
                if checksum_is_empty(source) {
                    return Err(LockError::Corrupt {
                        message: format!("empty source_checksum for {}", outcome.dest_path),
                    }
                    .into());
                }
            }
            ActionTaken::Removed | ActionTaken::Skipped | ActionTaken::Kept => {}
        }
    }

    // Build dependency entries directly from resolved graph provenance.
    for (name, node) in &graph.nodes {
        dependencies.insert(name.clone(), to_locked_source(node));
    }

    // Build item entries from apply outcomes.
    for outcome in &applied.outcomes {
        match &outcome.action {
            ActionTaken::Removed | ActionTaken::Skipped => {
                // For skipped items, carry forward from old lock
                if matches!(outcome.action, ActionTaken::Skipped) {
                    let item_key = item_key(&outcome.item_id);
                    if let Some(old_item) = old_lock.items.get(&item_key) {
                        items.insert(item_key, old_item.clone());
                    } else {
                        // Fall back: search old lock by dest_path when the logical item key differs
                        if let Some((_, old_item, old_output)) = old_lock_index
                            .item_for_output(CANONICAL_TARGET_ROOT, &outcome.dest_path)
                        {
                            let key = format!(
                                "{}/{}",
                                old_item.kind,
                                outcome.dest_path.item_name(old_item.kind)
                            );
                            items.entry(key).or_insert_with(|| LockedItemV2 {
                                source: old_item.source.clone(),
                                kind: old_item.kind,
                                version: old_item.version.clone(),
                                source_checksum: old_item.source_checksum.clone(),
                                outputs: outputs_with_carried_non_canonical(
                                    Some(old_item),
                                    OutputRecord::installed(
                                        CANONICAL_TARGET_ROOT.to_string(),
                                        old_output.dest_path.clone(),
                                        old_output
                                            .installed_checksum()
                                            .expect("canonical output is installed")
                                            .clone(),
                                    ),
                                ),
                            });
                        }
                    }
                }
                // Removed items are excluded from the new lock.
            }
            ActionTaken::Kept => {
                // Keep local: carry forward old lock entry.
                let item_key = item_key(&outcome.item_id);
                if let Some(old_item) = old_lock.items.get(&item_key) {
                    items.insert(item_key, old_item.clone());
                } else if let Some((_, old_item, old_output)) =
                    old_lock_index.item_for_output(CANONICAL_TARGET_ROOT, &outcome.dest_path)
                {
                    let key = format!(
                        "{}/{}",
                        old_item.kind,
                        outcome.dest_path.item_name(old_item.kind)
                    );
                    items.entry(key).or_insert_with(|| LockedItemV2 {
                        source: old_item.source.clone(),
                        kind: old_item.kind,
                        version: old_item.version.clone(),
                        source_checksum: old_item.source_checksum.clone(),
                        outputs: outputs_with_carried_non_canonical(
                            Some(old_item),
                            OutputRecord::installed(
                                CANONICAL_TARGET_ROOT.to_string(),
                                old_output.dest_path.clone(),
                                old_output
                                    .installed_checksum()
                                    .expect("canonical output is installed")
                                    .clone(),
                            ),
                        ),
                    });
                }
            }
            ActionTaken::Installed | ActionTaken::Updated => {
                let dest_path = outcome.dest_path.clone();
                if dest_path.as_str().is_empty() {
                    continue;
                }

                // Use source_name from outcome (propagated from TargetItem)
                let source_name = if outcome.source_name.as_ref().is_empty() {
                    None
                } else {
                    Some(outcome.source_name.clone())
                };

                // Determine version from graph
                let version = source_name.as_ref().and_then(|sn| {
                    graph
                        .nodes
                        .get(sn)
                        .and_then(|n| n.resolved_ref.version_tag.clone())
                });

                let source_checksum = outcome
                    .source_checksum
                    .clone()
                    .expect("validated above: source_checksum exists for write actions");
                let installed_checksum = outcome
                    .installed_checksum
                    .clone()
                    .expect("validated above: installed_checksum exists for write actions");

                let key = item_key(&outcome.item_id);
                let old_item = old_lock.items.get(&key).or_else(|| {
                    old_lock_index
                        .item_for_output(CANONICAL_TARGET_ROOT, &outcome.dest_path)
                        .map(|(_, old_item, _)| old_item)
                });
                let outputs = outputs_with_carried_non_canonical(
                    old_item,
                    OutputRecord::installed(
                        CANONICAL_TARGET_ROOT.to_string(),
                        dest_path,
                        installed_checksum,
                    ),
                );
                items.insert(
                    key,
                    LockedItemV2 {
                        source: source_name.unwrap_or_else(|| SourceName::from("")),
                        kind: outcome.item_id.kind,
                        version,
                        source_checksum,
                        outputs,
                    },
                );
            }
        }
    }

    // Add synthetic _self source if any local package items exist.
    let local_source_name: SourceName = SourceOrigin::LocalPackage.to_string().into();
    let has_self_items = items.values().any(|item| item.source == local_source_name);
    if has_self_items {
        dependencies.insert(
            local_source_name,
            LockedSource {
                url: None,
                path: Some(".".into()),
                subpath: None,
                version: None,
                commit: None,
            },
        );
    }

    // Validate checksums.
    for item in items.values() {
        if checksum_is_empty(&item.source_checksum) {
            let dest = item
                .outputs
                .first()
                .map(|o| o.dest_path.to_string())
                .unwrap_or_default();
            return Err(LockError::Corrupt {
                message: format!("empty source_checksum for {dest}"),
            }
            .into());
        }
        for output in &item.outputs {
            if output.installed_checksum().is_some_and(checksum_is_empty) {
                return Err(LockError::Corrupt {
                    message: format!("empty installed_checksum for {}", output.dest_path),
                }
                .into());
            }
        }
    }

    // Sort keys for deterministic output.
    dependencies.sort_keys();
    items.sort_keys();

    Ok(LockFile {
        version: LOCK_VERSION,
        dependencies,
        items,
        config_entries,
        dependency_model_aliases: IndexMap::new(),
    })
}

fn outputs_with_carried_non_canonical(
    old_item: Option<&LockedItemV2>,
    canonical_output: OutputRecord,
) -> Vec<OutputRecord> {
    let mut outputs = vec![canonical_output];
    if let Some(old_item) = old_item {
        for old_output in &old_item.outputs {
            if old_output.target_root != CANONICAL_TARGET_ROOT {
                outputs.push(old_output.clone());
            }
        }
    }
    outputs
}

/// Lock view for native emission immediately after apply + target sync.
///
/// Seeds canonical `.mars` items from the current apply pass, then layers
/// per-target sync outputs so `copy_decision` treats freshly synced paths as
/// managed. Full lock rebuild happens in `finalize()`; this path avoids a
/// graph walk while still covering first-sync agents absent from `old_lock`.
pub fn ownership_lock_for_native_emission(
    old_lock: &LockFile,
    apply_outcomes: &[crate::sync::apply::ActionOutcome],
    target_outcomes: &[crate::target_sync::TargetSyncOutcome],
) -> LockFile {
    let mut lock = old_lock.clone();
    apply_apply_outcomes_to_lock(&mut lock, old_lock, apply_outcomes);
    apply_target_sync_outputs(&mut lock, target_outcomes);
    lock
}

/// Lock view for native emission after `mars link` target sync.
///
/// The persisted lock already reflects canonical items; only target-sync outputs
/// from the link pass need to be layered on for ownership checks.
pub fn ownership_lock_after_target_sync(
    old_lock: &LockFile,
    target_outcomes: &[crate::target_sync::TargetSyncOutcome],
) -> LockFile {
    let mut lock = old_lock.clone();
    apply_target_sync_outputs(&mut lock, target_outcomes);
    lock
}

/// Merge current apply outcomes into a lock view for ownership checks.
///
/// Write actions upsert canonical `.mars` outputs; removals drop the item;
/// skipped/kept entries carry forward from `old_lock` when the clone lacks them.
pub fn apply_apply_outcomes_to_lock(
    lock: &mut LockFile,
    old_lock: &LockFile,
    outcomes: &[crate::sync::apply::ActionOutcome],
) {
    use crate::sync::apply::ActionTaken;

    let old_lock_index = LockIndex::new(old_lock);
    for outcome in outcomes {
        match outcome.action {
            ActionTaken::Removed => {
                lock.items.shift_remove(&item_key(&outcome.item_id));
            }
            ActionTaken::Skipped => {
                let key = item_key(&outcome.item_id);
                if lock.items.contains_key(&key) {
                    continue;
                }
                if let Some(old_item) = old_lock.items.get(&key) {
                    lock.items.insert(key, old_item.clone());
                } else if let Some(flat) =
                    old_lock_index.find_output(CANONICAL_TARGET_ROOT, &outcome.dest_path)
                {
                    let key = format!("{}/{}", flat.kind, outcome.dest_path.item_name(flat.kind));
                    lock.items.entry(key).or_insert_with(|| LockedItemV2 {
                        source: flat.source,
                        kind: flat.kind,
                        version: flat.version,
                        source_checksum: flat.source_checksum,
                        outputs: vec![OutputRecord::installed(
                            CANONICAL_TARGET_ROOT.to_string(),
                            flat.dest_path,
                            flat.installed_checksum,
                        )],
                    });
                }
            }
            ActionTaken::Kept => {
                let key = item_key(&outcome.item_id);
                if let Some(old_item) = old_lock.items.get(&key) {
                    lock.items.insert(key, old_item.clone());
                } else if let Some(flat) =
                    old_lock_index.find_output(CANONICAL_TARGET_ROOT, &outcome.dest_path)
                {
                    let key = format!("{}/{}", flat.kind, outcome.dest_path.item_name(flat.kind));
                    lock.items.entry(key).or_insert_with(|| LockedItemV2 {
                        source: flat.source,
                        kind: flat.kind,
                        version: flat.version,
                        source_checksum: flat.source_checksum,
                        outputs: vec![OutputRecord::installed(
                            CANONICAL_TARGET_ROOT.to_string(),
                            flat.dest_path,
                            flat.installed_checksum,
                        )],
                    });
                }
            }
            ActionTaken::Installed | ActionTaken::Updated => {
                if outcome.dest_path.as_str().is_empty() {
                    continue;
                }
                let Some(source_checksum) = outcome
                    .source_checksum
                    .as_ref()
                    .filter(|checksum| !checksum_is_empty(checksum))
                else {
                    continue;
                };
                let Some(installed_checksum) = outcome
                    .installed_checksum
                    .as_ref()
                    .filter(|checksum| !checksum_is_empty(checksum))
                else {
                    continue;
                };

                let source_name = if outcome.source_name.as_ref().is_empty() {
                    SourceName::from("")
                } else {
                    outcome.source_name.clone()
                };

                let key = item_key(&outcome.item_id);
                let old_entry = old_lock
                    .items
                    .get(&key)
                    .map(|old_item| (key.as_str(), old_item))
                    .or_else(|| {
                        old_lock_index
                            .item_for_output(CANONICAL_TARGET_ROOT, &outcome.dest_path)
                            .map(|(old_key, old_item, _)| (old_key, old_item))
                    });
                let old_key = old_entry.map(|(old_key, _)| old_key.to_string());
                let outputs = outputs_with_carried_non_canonical(
                    old_entry.map(|(_, old_item)| old_item),
                    OutputRecord::installed(
                        CANONICAL_TARGET_ROOT.to_string(),
                        outcome.dest_path.clone(),
                        installed_checksum.clone(),
                    ),
                );
                if let Some(old_key) = old_key
                    && old_key != key
                {
                    lock.items.shift_remove(&old_key);
                }
                lock.items.insert(
                    key,
                    LockedItemV2 {
                        source: source_name,
                        kind: outcome.item_id.kind,
                        version: None,
                        source_checksum: source_checksum.clone(),
                        outputs,
                    },
                );
            }
        }
    }
}

/// Merge per-target sync results into a built lock file.
pub fn apply_target_sync_outputs(
    lock: &mut LockFile,
    target_outcomes: &[crate::target_sync::TargetSyncOutcome],
) {
    for outcome in target_outcomes {
        for dest_path in &outcome.removed_dest_paths {
            remove_target_output(lock, &outcome.target, dest_path);
        }
        for synced in &outcome.synced_outputs {
            upsert_target_output(
                lock,
                &outcome.target,
                &synced.dest_path,
                &synced.installed_checksum,
            );
        }
    }
}

/// Native harness output recorded in the lock for a canonical `.mars` agent item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledNativeOutput {
    /// Canonical `.mars` dest path for the owning agent (e.g. `agents/coder.md`).
    pub owner_canonical_dest_path: String,
    pub target_root: String,
    pub dest_path: String,
    pub installed_checksum: ContentHash,
}

/// Whether a freshly compiled native output is new or content-changed vs the
/// previous lock at the same `(target_root, dest_path)`. Lets the sync summary
/// count only real emissions — steady-state re-emits don't inflate the count.
pub fn native_output_is_new_or_changed(old: &LockFile, out: &CompiledNativeOutput) -> bool {
    for item in old.items.values() {
        for output in &item.outputs {
            if output.target_root == out.target_root
                && crate::target::dest_paths_equivalent(output.dest_path.as_str(), &out.dest_path)
            {
                return output.installed_checksum() != Some(&out.installed_checksum);
            }
        }
    }
    true
}

/// Drop native harness output records removed by native agent reconcile.
pub fn apply_removed_native_outputs(lock: &mut LockFile, records: &[(String, String)]) {
    for (target_root, dest_path) in records {
        remove_target_output(lock, target_root, dest_path);
    }
}

/// Record native harness outputs produced by dual-surface compile.
pub fn apply_compiled_native_outputs(
    lock: &mut LockFile,
    records: &[CompiledNativeOutput],
) -> Result<(), LockError> {
    for record in records {
        if !upsert_native_output_on_owner(
            lock,
            &record.owner_canonical_dest_path,
            &record.target_root,
            &record.dest_path,
            &record.installed_checksum,
        ) {
            return Err(LockError::Corrupt {
                message: format!(
                    "native output `{}/{}` has no canonical owner `{}`",
                    record.target_root, record.dest_path, record.owner_canonical_dest_path
                ),
            });
        }
    }
    Ok(())
}

/// Preserve unresolved noncanonical removal authority as a retry tombstone.
///
/// A rebuilt lock omits canonical items removed from the source graph. Their
/// linked-target artifacts can outlive that removal when filesystem deletion
/// fails, so the unresolved linked outputs must remain owned until removal
/// succeeds.
pub fn retain_unremoved_noncanonical_outputs(
    lock: &mut LockFile,
    old_lock: &LockFile,
    removed: &[(String, String)],
) {
    for (old_key, old_item) in &old_lock.items {
        let unresolved: Vec<_> = old_item
            .outputs
            .iter()
            .filter(|output| output.target_root != CANONICAL_TARGET_ROOT)
            .filter(|output| {
                !removed.iter().any(|(target_root, dest_path)| {
                    output.target_root == *target_root
                        && crate::target::dest_paths_equivalent(
                            output.dest_path.as_str(),
                            dest_path,
                        )
                })
            })
            .filter(|output| !lock.contains_output(&output.target_root, output.dest_path.as_str()))
            .map(|output| {
                OutputRecord::pending_deletion(output.target_root.clone(), output.dest_path.clone())
            })
            .collect();
        if unresolved.is_empty() {
            continue;
        }

        let item = lock
            .items
            .entry(old_key.clone())
            .or_insert_with(|| LockedItemV2 {
                source: old_item.source.clone(),
                kind: old_item.kind,
                version: old_item.version.clone(),
                source_checksum: old_item.source_checksum.clone(),
                // A retry tombstone may carry only unresolved noncanonical outputs.
                // It must never resurrect an old canonical output: only the current
                // apply pass can grant canonical ownership and deletion authority.
                outputs: Vec::new(),
            });
        item.outputs.extend(unresolved);
        item.outputs.sort_by(|a, b| {
            a.target_root
                .cmp(&b.target_root)
                .then_with(|| a.dest_path.as_str().cmp(b.dest_path.as_str()))
        });
        item.outputs.dedup_by(|a, b| {
            a.target_root == b.target_root
                && crate::target::dest_paths_equivalent(a.dest_path.as_str(), b.dest_path.as_str())
        });
    }
}

fn upsert_target_output(
    lock: &mut LockFile,
    target_root: &str,
    dest_path: &str,
    installed_checksum: &ContentHash,
) {
    let dest = DestPath::from(dest_path);
    let scoped_hook_owner = dest_path.strip_prefix("hooks/").map(|hook_name| {
        format!(
            "hooks/{}/{}",
            target_root.trim_start_matches('.'),
            hook_name
        )
    });
    for item in lock.items.values_mut() {
        let owns_output = if item.kind == ItemKind::Hook {
            item.outputs.iter().any(|output| {
                scoped_hook_owner.as_deref().is_some_and(|owner| {
                    output.target_root == CANONICAL_TARGET_ROOT
                        && crate::target::dest_paths_equivalent(output.dest_path.as_str(), owner)
                })
            })
        } else {
            item.outputs.iter().any(|output| {
                (output.target_root == target_root || output.target_root == CANONICAL_TARGET_ROOT)
                    && crate::target::dest_paths_equivalent(output.dest_path.as_str(), dest_path)
            })
        };
        if !owns_output {
            continue;
        }

        if let Some(output) = item.outputs.iter_mut().find(|output| {
            output.target_root == target_root
                && crate::target::dest_paths_equivalent(output.dest_path.as_str(), dest_path)
        }) {
            output.mark_installed(installed_checksum.clone());
            return;
        }

        item.outputs.push(OutputRecord::installed(
            target_root.to_string(),
            dest,
            installed_checksum.clone(),
        ));
        item.outputs.sort_by(|a, b| {
            a.target_root
                .cmp(&b.target_root)
                .then_with(|| a.dest_path.as_str().cmp(b.dest_path.as_str()))
        });
        return;
    }
}

fn upsert_native_output_on_owner(
    lock: &mut LockFile,
    owner_canonical_dest_path: &str,
    target_root: &str,
    native_dest_path: &str,
    installed_checksum: &ContentHash,
) -> bool {
    let native_dest = DestPath::from(native_dest_path);
    for item in lock.items.values_mut() {
        let owns_canonical = item.outputs.iter().any(|output| {
            output.target_root == CANONICAL_TARGET_ROOT
                && crate::target::dest_paths_equivalent(
                    output.dest_path.as_str(),
                    owner_canonical_dest_path,
                )
        });
        if !owns_canonical {
            continue;
        }

        if let Some(output) = item.outputs.iter_mut().find(|output| {
            output.target_root == target_root
                && crate::target::dest_paths_equivalent(output.dest_path.as_str(), native_dest_path)
        }) {
            output.mark_installed(installed_checksum.clone());
            return true;
        }

        item.outputs.push(OutputRecord::installed(
            target_root.to_string(),
            native_dest,
            installed_checksum.clone(),
        ));
        item.outputs.sort_by(|a, b| {
            a.target_root
                .cmp(&b.target_root)
                .then_with(|| a.dest_path.as_str().cmp(b.dest_path.as_str()))
        });
        return true;
    }
    false
}

fn remove_target_output(lock: &mut LockFile, target_root: &str, dest_path: &str) {
    for item in lock.items.values_mut() {
        item.outputs.retain(|output| {
            !(output.target_root == target_root
                && crate::target::dest_paths_equivalent(output.dest_path.as_str(), dest_path))
        });
    }
    lock.items.retain(|_, item| !item.outputs.is_empty());
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn checksum_is_empty(checksum: &ContentHash) -> bool {
    checksum.as_ref().trim().is_empty()
}

fn to_locked_source(node: &crate::resolve::ResolvedNode) -> LockedSource {
    let (url, path, subpath) = match &node.source_id {
        SourceId::Git { url, subpath } => (Some(url.clone()), None, subpath.clone()),
        SourceId::Path { canonical, subpath } => (
            None,
            Some(canonical.to_string_lossy().to_string()),
            subpath.clone(),
        ),
    };

    LockedSource {
        url,
        path,
        subpath,
        version: node.resolved_ref.version_tag.clone(),
        commit: node.resolved_ref.commit.clone(),
    }
}

/// Canonical item key for v2 lock: `"kind/name"`.
pub fn item_key(id: &ItemId) -> String {
    format!("{}/{}", id.kind, id.name)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    use crate::resolve::{ResolvedGraph, ResolvedNode};
    use crate::source::ResolvedRef;
    use crate::sync::apply::{ActionOutcome, ActionTaken, ApplyResult};
    use crate::types::{ItemName, SourceId, SourceUrl};
    use tempfile::TempDir;

    fn sample_lock() -> LockFile {
        let mut dependencies = IndexMap::new();
        dependencies.insert(
            "base".into(),
            LockedSource {
                url: Some("https://github.com/org/base.git".into()),
                path: None,
                subpath: None,
                version: Some("v1.0.0".into()),
                commit: Some("abc123".into()),
            },
        );

        let mut items = IndexMap::new();
        items.insert(
            "agent/coder".to_string(),
            LockedItemV2 {
                source: "base".into(),
                kind: ItemKind::Agent,
                version: Some("v1.0.0".into()),
                source_checksum: "sha256:aaa".into(),
                outputs: vec![OutputRecord::installed(
                    ".mars".to_string(),
                    "agents/coder.md".into(),
                    "sha256:bbb".into(),
                )],
            },
        );
        items.insert(
            "skill/review".to_string(),
            LockedItemV2 {
                source: "base".into(),
                kind: ItemKind::Skill,
                version: Some("v1.0.0".into()),
                source_checksum: "sha256:ccc".into(),
                outputs: vec![OutputRecord::installed(
                    ".mars".to_string(),
                    "skills/review".into(),
                    "sha256:ddd".into(),
                )],
            },
        );

        LockFile {
            version: LOCK_VERSION,
            dependencies,
            items,
            config_entries: BTreeMap::new(),
            dependency_model_aliases: IndexMap::new(),
        }
    }

    #[test]
    fn v1_lock_version_has_actionable_error() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("mars.lock"), "version = 1\n").unwrap();

        let error = load(dir.path()).unwrap_err().to_string();

        assert!(error.contains("unsupported lock version 1"));
        assert!(error.contains("only version 2 can be promoted"));
        assert!(error.contains(&format!(
            "remove it and run `{}`",
            crate::types::managed_cmd("mars sync")
        )));
    }

    #[test]
    fn v2_output_matching_regular_file_promotes_to_installed() {
        let dir = TempDir::new().unwrap();
        let output = dir.path().join(".mars/agents/coder.md");
        std::fs::create_dir_all(output.parent().unwrap()).unwrap();
        std::fs::write(&output, "managed").unwrap();
        let checksum = crate::hash::hash_bytes(b"managed");
        std::fs::write(
            dir.path().join("mars.lock"),
            format!(
                r#"
version = 2

[items."agent/coder"]
source = "_self"
kind = "agent"
source_checksum = "{checksum}"

[[items."agent/coder".outputs]]
target_root = ".mars"
dest_path = "agents/coder.md"
installed_checksum = "{checksum}"
"#
            ),
        )
        .unwrap();

        let lock = load(dir.path()).unwrap();
        let output = &lock.items["agent/coder"].outputs[0];

        assert_eq!(lock.version, LOCK_VERSION);
        assert!(matches!(output.state, OutputState::Installed { .. }));
    }

    #[test]
    fn v2_output_absent_on_disk_promotes_to_pending_deletion() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("mars.lock"),
            r#"
version = 2

[items."hook/audit"]
source = "_self"
kind = "hook"
source_checksum = "sha256:source"

[[items."hook/audit".outputs]]
target_root = ".opencode"
dest_path = "plugins/mars-audit.ts"
installed_checksum = "sha256:old"
"#,
        )
        .unwrap();

        let lock = load(dir.path()).unwrap();

        assert!(matches!(
            lock.items["hook/audit"].outputs[0].state,
            OutputState::PendingDeletion
        ));
    }

    #[cfg(unix)]
    #[test]
    fn v2_directory_output_with_nested_dangling_symlink_has_no_checksum() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let output = dir.path().join("skill");
        std::fs::create_dir(&output).unwrap();
        std::fs::write(output.join("SKILL.md"), "# Skill").unwrap();
        symlink("missing.md", output.join("reference.md")).unwrap();

        assert_eq!(v2_output_checksum(&output), None);
    }

    #[cfg(unix)]
    #[test]
    fn v2_directory_output_with_nested_directory_symlink_has_no_checksum() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let output = dir.path().join("skill");
        let external = dir.path().join("external");
        std::fs::create_dir(&output).unwrap();
        std::fs::create_dir(&external).unwrap();
        std::fs::write(output.join("SKILL.md"), "# Skill").unwrap();
        std::fs::write(external.join("reference.md"), "# Reference").unwrap();
        symlink(&external, output.join("references")).unwrap();

        assert_eq!(v2_output_checksum(&output), None);
    }

    #[cfg(unix)]
    #[test]
    fn v2_directory_output_with_nested_fifo_returns_without_opening_it() {
        let dir = TempDir::new().unwrap();
        let output = dir.path().join("skill");
        std::fs::create_dir(&output).unwrap();
        std::fs::write(output.join("SKILL.md"), "# Skill").unwrap();
        let status = std::process::Command::new("mkfifo")
            .arg(output.join("events"))
            .status()
            .unwrap();
        assert!(status.success());

        let started = std::time::Instant::now();
        assert_eq!(v2_output_checksum(&output), None);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "shape validation must not open and block on the FIFO"
        );
    }

    #[cfg(unix)]
    #[test]
    fn v2_promotion_classifies_outputs_by_disk_shape_and_checksum() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let outputs = root.join(".mars/outputs");
        std::fs::create_dir_all(&outputs).unwrap();

        std::fs::write(outputs.join("file-match"), "managed").unwrap();
        std::fs::write(outputs.join("file-mismatch"), "changed").unwrap();
        std::fs::create_dir(outputs.join("dir-match")).unwrap();
        std::fs::write(outputs.join("dir-match/SKILL.md"), "# Managed").unwrap();
        std::fs::create_dir(outputs.join("dir-mismatch")).unwrap();
        std::fs::write(outputs.join("dir-mismatch/SKILL.md"), "# Changed").unwrap();
        std::fs::write(outputs.join("symlink-target"), "managed").unwrap();
        symlink("symlink-target", outputs.join("symlink")).unwrap();
        std::fs::write(outputs.join("unreadable"), "managed").unwrap();
        std::fs::set_permissions(
            outputs.join("unreadable"),
            std::fs::Permissions::from_mode(0o000),
        )
        .unwrap();

        let file_checksum = crate::hash::hash_bytes(b"managed");
        let directory_checksum =
            crate::hash::compute_hash(&outputs.join("dir-match"), ItemKind::Skill).unwrap();
        let cases = [
            ("file-match", &file_checksum),
            ("file-mismatch", &file_checksum),
            ("dir-match", &directory_checksum),
            ("dir-mismatch", &directory_checksum),
            ("absent", &file_checksum),
            ("symlink", &file_checksum),
            ("unreadable", &file_checksum),
        ];
        let mut lock = String::from("version = 2\n");
        for (name, checksum) in cases {
            lock.push_str(&format!(
                r#"
[items."agent/{name}"]
source = "_self"
kind = "agent"
source_checksum = "{checksum}"

[[items."agent/{name}".outputs]]
target_root = ".mars"
dest_path = "outputs/{name}"
installed_checksum = "{checksum}"
"#
            ));
        }
        std::fs::write(root.join("mars.lock"), lock).unwrap();

        let promoted = load(root).unwrap();
        std::fs::set_permissions(
            outputs.join("unreadable"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();

        for name in ["file-match", "dir-match"] {
            assert!(
                matches!(
                    promoted.items[&format!("agent/{name}")].outputs[0].state,
                    OutputState::Installed { .. }
                ),
                "{name} should retain installed-content authority"
            );
        }
        for name in [
            "file-mismatch",
            "dir-mismatch",
            "absent",
            "symlink",
            "unreadable",
        ] {
            assert!(
                matches!(
                    promoted.items[&format!("agent/{name}")].outputs[0].state,
                    OutputState::PendingDeletion
                ),
                "{name} should retain deletion authority only"
            );
        }
    }

    #[test]
    fn load_for_runtime_aliases_rejects_legacy_v2_without_dependency_alias_authority() {
        let toml_str = r#"
version = 3

[dependencies.base]
url = "https://github.com/org/base.git"
version = "v1.0.0"
commit = "abc123"

[items."agent/coder"]
source = "base"
kind = "agent"
source_checksum = "sha256:aaa"

[[items."agent/coder".outputs]]
target_root = ".mars"
dest_path = "agents/coder.md"
state = "installed"
installed_checksum = "sha256:bbb"
"#;
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("mars.lock"), toml_str).unwrap();

        let err = load_for_runtime_aliases(dir.path()).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("missing `dependency_model_aliases`"));
        assert!(message.contains(&format!("run `{}`", crate::types::managed_cmd("mars sync"))));
    }

    #[test]
    fn load_for_runtime_aliases_allows_missing_dependency_aliases_when_no_dependencies() {
        let toml_str = r#"
version = 3

[items."agent/coder"]
source = "_self"
kind = "agent"
source_checksum = "sha256:aaa"

[[items."agent/coder".outputs]]
target_root = ".mars"
dest_path = "agents/coder.md"
state = "installed"
installed_checksum = "sha256:bbb"
"#;
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("mars.lock"), toml_str).unwrap();

        let lock = load_for_runtime_aliases(dir.path()).unwrap();
        assert!(lock.dependencies.is_empty());
        assert!(lock.dependency_model_aliases.is_empty());
    }

    #[test]
    fn roundtrip_lock_file() {
        let lock = sample_lock();
        let dir = TempDir::new().unwrap();
        write(dir.path(), &lock).unwrap();
        let reloaded = load(dir.path()).unwrap();
        assert_eq!(lock, reloaded);
    }

    #[test]
    fn roundtrip_lock_file_with_config_entries() {
        let mut lock = sample_lock();
        lock.config_entries.insert(
            ".claude".to_string(),
            BTreeMap::from([(
                "mcp:context7".to_string(),
                ConfigEntryRecord { emitted_json: None },
            )]),
        );

        let dir = TempDir::new().unwrap();
        write(dir.path(), &lock).unwrap();
        let reloaded = load(dir.path()).unwrap();

        assert_eq!(lock, reloaded);
        assert_eq!(
            reloaded.config_entries[".claude"]["mcp:context7"].emitted_json,
            None
        );
    }

    #[test]
    fn write_emits_dependency_model_aliases_table_even_when_empty() {
        let lock = sample_lock();
        let dir = TempDir::new().unwrap();
        write(dir.path(), &lock).unwrap();

        let content = std::fs::read_to_string(dir.path().join("mars.lock")).unwrap();
        assert!(
            content.contains("dependency_model_aliases"),
            "serialized lock should include dependency_model_aliases authority table"
        );
    }

    #[test]
    fn deterministic_serialization() {
        let lock = sample_lock();
        let s1 = toml::to_string_pretty(&lock).unwrap();
        let s2 = toml::to_string_pretty(&lock).unwrap();
        assert_eq!(s1, s2);

        // V2: keys are "agent/coder" and "skill/review" — agent comes before skill alphabetically.
        let coder_pos = s1.find("agent/coder").unwrap();
        let review_pos = s1.find("skill/review").unwrap();
        assert!(
            coder_pos < review_pos,
            "agent/coder should appear before skill/review"
        );
    }

    #[test]
    fn write_sorts_dependency_model_aliases_keys() {
        let toml_str = r#"
version = 3

[dependency_model_aliases.zeta]
model = "openai/gpt-z"

[dependency_model_aliases.alpha]
model = "openai/gpt-a"
"#;
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("mars.lock"), toml_str).unwrap();

        let lock = load(dir.path()).unwrap();
        write(dir.path(), &lock).unwrap();

        let written = std::fs::read_to_string(dir.path().join("mars.lock")).unwrap();
        let alpha = written
            .find("[dependency_model_aliases.alpha]")
            .expect("alpha alias should be serialized");
        let zeta = written
            .find("[dependency_model_aliases.zeta]")
            .expect("zeta alias should be serialized");
        assert!(alpha < zeta, "aliases should serialize in sorted key order");
    }

    #[test]
    fn empty_lock_file() {
        let lock = LockFile::empty();
        assert_eq!(lock.version, LOCK_VERSION);
        assert!(lock.dependencies.is_empty());
        assert!(lock.items.is_empty());
    }

    #[test]
    fn load_absent_returns_empty() {
        let dir = TempDir::new().unwrap();
        let lock = load(dir.path()).unwrap();
        assert_eq!(lock.version, LOCK_VERSION);
        assert!(lock.dependencies.is_empty());
        assert!(lock.items.is_empty());
    }

    #[test]
    fn write_and_reload() {
        let dir = TempDir::new().unwrap();
        let lock = sample_lock();
        write(dir.path(), &lock).unwrap();
        let reloaded = load(dir.path()).unwrap();
        assert_eq!(lock, reloaded);
    }

    #[test]
    fn dual_checksums_present() {
        let lock = sample_lock();
        let item = &lock.items["agent/coder"];
        assert_ne!(
            &item.source_checksum,
            item.outputs[0]
                .installed_checksum()
                .expect("installed output")
        );
        assert!(item.source_checksum.starts_with("sha256:"));
        assert!(
            item.outputs[0]
                .installed_checksum()
                .expect("installed output")
                .starts_with("sha256:")
        );
    }

    #[test]
    fn path_source_in_lock() {
        let toml_str = r#"
version = 3

[dependencies.local]
path = "/home/dev/agents"

[items."agent/helper"]
source = "local"
kind = "agent"
source_checksum = "sha256:111"

[[items."agent/helper".outputs]]
target_root = ".mars"
dest_path = "agents/helper.md"
state = "installed"
installed_checksum = "sha256:222"
"#;
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("mars.lock"), toml_str).unwrap();
        let lock = load(dir.path()).unwrap();
        let source = &lock.dependencies["local"];
        assert!(source.url.is_none());
        assert_eq!(source.path.as_deref(), Some("/home/dev/agents"));
        assert!(source.commit.is_none());
    }

    #[test]
    fn item_kind_serializes_lowercase() {
        let item = LockedItemV2 {
            source: "base".into(),
            kind: ItemKind::Skill,
            version: None,
            source_checksum: "sha256:aaa".into(),
            outputs: vec![OutputRecord::installed(
                ".mars".to_string(),
                "skills/review".into(),
                "sha256:bbb".into(),
            )],
        };
        let serialized = toml::to_string(&item).unwrap();
        assert!(serialized.contains("kind = \"skill\""));
    }

    #[test]
    fn item_id_display() {
        let id = ItemId {
            kind: ItemKind::Agent,
            name: "coder".into(),
        };
        assert_eq!(id.to_string(), "agent/coder");
    }

    #[test]
    fn item_kind_display() {
        assert_eq!(ItemKind::Agent.to_string(), "agent");
        assert_eq!(ItemKind::Skill.to_string(), "skill");
    }

    #[test]
    fn find_by_dest_path_returns_flat_view() {
        let lock = sample_lock();
        let found = lock
            .find_by_dest_path(&DestPath::from("agents/coder.md"))
            .unwrap();
        assert_eq!(found.source, "base");
        assert_eq!(found.kind, ItemKind::Agent);
        assert_eq!(found.source_checksum, "sha256:aaa");
        assert_eq!(found.installed_checksum, "sha256:bbb");
        assert_eq!(found.dest_path.as_str(), "agents/coder.md");
    }

    #[test]
    fn find_by_dest_path_missing_returns_none() {
        let lock = sample_lock();
        assert!(
            lock.find_by_dest_path(&DestPath::from("agents/missing.md"))
                .is_none()
        );
    }

    #[test]
    fn contains_dest_path_hit_and_miss() {
        let lock = sample_lock();
        assert!(lock.contains_dest_path(&DestPath::from("agents/coder.md")));
        assert!(!lock.contains_dest_path(&DestPath::from("agents/nobody.md")));
    }

    #[test]
    fn lock_index_target_scoped_lookup_distinguishes_same_dest_path() {
        let mut lock = sample_lock();
        lock.items
            .get_mut("agent/coder")
            .unwrap()
            .outputs
            .push(OutputRecord::installed(
                ".pi".to_string(),
                "agents/coder.md".into(),
                "sha256:pi".into(),
            ));

        let index = LockIndex::new(&lock);
        let dest = DestPath::from("agents/coder.md");

        let mars = index
            .find_output(".mars", &dest)
            .expect("expected canonical .mars output");
        let pi = index
            .find_output(".pi", &dest)
            .expect("expected .pi output");

        assert_eq!(mars.installed_checksum, "sha256:bbb");
        assert_eq!(pi.installed_checksum, "sha256:pi");
        assert!(index.contains_output(".mars", &dest));
        assert!(index.contains_output(".pi", &dest));
        assert!(!index.contains_output(".cursor", &dest));
    }

    #[test]
    fn output_dest_paths_for_target_filters_by_target_root() {
        let mut lock = sample_lock();
        lock.items
            .get_mut("agent/coder")
            .unwrap()
            .outputs
            .push(OutputRecord::installed(
                ".cursor".to_string(),
                "agents/coder.md".into(),
                "sha256:cursor".into(),
            ));

        let mars_paths = lock.output_dest_paths_for_target(".mars");
        assert!(mars_paths.contains("agents/coder.md"));
        assert!(mars_paths.contains("skills/review"));

        let cursor_paths = lock.output_dest_paths_for_target(".cursor");
        assert_eq!(cursor_paths.len(), 1);
        assert!(cursor_paths.contains("agents/coder.md"));
        assert!(lock.output_dest_paths_for_target(".claude").is_empty());
    }

    #[test]
    fn contains_output_matches_target_root_and_dest_path() {
        let mut lock = sample_lock();
        assert!(lock.contains_output(".mars", "agents/coder.md"));
        assert!(!lock.contains_output(".cursor", "agents/coder.md"));

        lock.items
            .get_mut("agent/coder")
            .unwrap()
            .outputs
            .push(OutputRecord::installed(
                ".cursor".to_string(),
                "agents/coder.md".into(),
                "sha256:cursor".into(),
            ));
        assert!(lock.contains_output(".cursor", "agents/coder.md"));
        assert!(!lock.contains_output(".cursor", "agents/missing.md"));
    }

    #[test]
    fn apply_compiled_native_outputs_upserts_codex_native_by_canonical_owner() {
        let mut lock = sample_lock();
        apply_compiled_native_outputs(
            &mut lock,
            &[CompiledNativeOutput {
                owner_canonical_dest_path: "agents/coder.md".to_string(),
                target_root: ".codex".to_string(),
                dest_path: "agents/coder.toml".to_string(),
                installed_checksum: "sha256:codex".into(),
            }],
        )
        .unwrap();
        assert!(lock.contains_output(".codex", "agents/coder.toml"));
        assert!(lock.contains_output(".mars", "agents/coder.md"));
    }

    #[test]
    fn apply_compiled_native_outputs_upserts_when_frontmatter_name_differs_from_filename() {
        let mut lock = sample_lock();
        lock.items.insert(
            "agent/alias-name".to_string(),
            LockedItemV2 {
                source: "base".into(),
                kind: ItemKind::Agent,
                version: Some("v1.0.0".into()),
                source_checksum: "sha256:alias-src".into(),
                outputs: vec![OutputRecord::installed(
                    ".mars".to_string(),
                    "agents/on-disk-stem.md".into(),
                    "sha256:alias-mars".into(),
                )],
            },
        );
        apply_compiled_native_outputs(
            &mut lock,
            &[CompiledNativeOutput {
                owner_canonical_dest_path: "agents/on-disk-stem.md".to_string(),
                target_root: ".claude".to_string(),
                dest_path: "agents/alias-name.md".to_string(),
                installed_checksum: "sha256:claude-native".into(),
            }],
        )
        .unwrap();
        assert!(lock.contains_output(".claude", "agents/alias-name.md"));
    }

    #[test]
    fn build_updated_carries_non_canonical_outputs() {
        let mut old_lock = sample_lock();
        old_lock
            .items
            .get_mut("agent/coder")
            .unwrap()
            .outputs
            .push(OutputRecord::installed(
                ".claude".to_string(),
                "agents/coder.md".into(),
                "sha256:claude-old".into(),
            ));

        let graph = ResolvedGraph {
            nodes: IndexMap::new(),
            order: Vec::new(),
            filters: HashMap::new(),
            version_constraints: std::collections::HashMap::new(),
        };
        let applied = ApplyResult {
            outcomes: vec![ActionOutcome {
                item_id: ItemId {
                    kind: ItemKind::Agent,
                    name: "coder".into(),
                },
                action: ActionTaken::Updated,
                dest_path: "agents/coder.md".into(),
                source_name: "base".into(),
                source_checksum: Some("sha256:new-src".into()),
                installed_checksum: Some("sha256:new-mars".into()),
            }],
        };

        let new_lock = build(
            &graph,
            &applied,
            &old_lock,
            std::collections::BTreeMap::new(),
        )
        .unwrap();

        assert!(new_lock.contains_output(".mars", "agents/coder.md"));
        assert!(
            new_lock.contains_output(".claude", "agents/coder.md"),
            ".claude record should survive compile failure"
        );
        let item = &new_lock.items["agent/coder"];
        assert_eq!(item.outputs.len(), 2);
        assert_eq!(item.source_checksum, "sha256:new-src");
        let mars = item
            .outputs
            .iter()
            .find(|o| o.target_root == ".mars")
            .unwrap();
        assert_eq!(
            mars.installed_checksum().expect("installed output"),
            "sha256:new-mars"
        );
        let claude = item
            .outputs
            .iter()
            .find(|o| o.target_root == ".claude")
            .unwrap();
        assert_eq!(
            claude.installed_checksum().expect("installed output"),
            "sha256:claude-old"
        );
    }

    #[test]
    fn build_fallback_carries_non_canonical_outputs_for_skipped_and_kept() {
        let old_lock = LockFile {
            version: LOCK_VERSION,
            dependencies: IndexMap::new(),
            items: IndexMap::from([
                (
                    "agent/agents/coder.md".to_string(),
                    LockedItemV2 {
                        source: "base".into(),
                        kind: ItemKind::Agent,
                        version: None,
                        source_checksum: "sha256:coder-src".into(),
                        outputs: vec![
                            OutputRecord::installed(
                                ".mars".to_string(),
                                "agents/coder.md".into(),
                                "sha256:coder-mars".into(),
                            ),
                            OutputRecord::installed(
                                ".claude".to_string(),
                                "agents/coder.md".into(),
                                "sha256:coder-claude".into(),
                            ),
                        ],
                    },
                ),
                (
                    "skill/skills/review".to_string(),
                    LockedItemV2 {
                        source: "base".into(),
                        kind: ItemKind::Skill,
                        version: None,
                        source_checksum: "sha256:review-src".into(),
                        outputs: vec![
                            OutputRecord::installed(
                                ".mars".to_string(),
                                "skills/review".into(),
                                "sha256:review-mars".into(),
                            ),
                            OutputRecord::installed(
                                ".codex".to_string(),
                                "skills/review/SKILL.md".into(),
                                "sha256:review-codex".into(),
                            ),
                        ],
                    },
                ),
            ]),
            config_entries: BTreeMap::new(),
            dependency_model_aliases: IndexMap::new(),
        };
        let graph = ResolvedGraph {
            nodes: IndexMap::new(),
            order: Vec::new(),
            filters: HashMap::new(),
            version_constraints: std::collections::HashMap::new(),
        };
        let applied = ApplyResult {
            outcomes: vec![
                ActionOutcome {
                    item_id: ItemId {
                        kind: ItemKind::Agent,
                        name: "coder".into(),
                    },
                    action: ActionTaken::Skipped,
                    dest_path: "agents/coder.md".into(),
                    source_name: "base".into(),
                    source_checksum: None,
                    installed_checksum: None,
                },
                ActionOutcome {
                    item_id: ItemId {
                        kind: ItemKind::Skill,
                        name: "review".into(),
                    },
                    action: ActionTaken::Kept,
                    dest_path: "skills/review".into(),
                    source_name: "base".into(),
                    source_checksum: None,
                    installed_checksum: None,
                },
            ],
        };

        let new_lock = build(
            &graph,
            &applied,
            &old_lock,
            std::collections::BTreeMap::new(),
        )
        .unwrap();

        assert!(!new_lock.items.contains_key("agent/agents/coder.md"));
        assert!(new_lock.contains_output(".mars", "agents/coder.md"));
        assert!(new_lock.contains_output(".claude", "agents/coder.md"));

        assert!(!new_lock.items.contains_key("skill/skills/review"));
        assert!(new_lock.contains_output(".mars", "skills/review"));
        assert!(new_lock.contains_output(".codex", "skills/review/SKILL.md"));
    }

    #[test]
    fn build_write_fallback_carries_non_canonical_outputs() {
        let old_lock = LockFile {
            version: LOCK_VERSION,
            dependencies: IndexMap::new(),
            items: IndexMap::from([(
                "agent/agents/coder.md".to_string(),
                LockedItemV2 {
                    source: "base".into(),
                    kind: ItemKind::Agent,
                    version: None,
                    source_checksum: "sha256:old-src".into(),
                    outputs: vec![
                        OutputRecord::installed(
                            ".mars".to_string(),
                            "agents/coder.md".into(),
                            "sha256:old-mars".into(),
                        ),
                        OutputRecord::installed(
                            ".claude".to_string(),
                            "agents/coder.md".into(),
                            "sha256:old-claude".into(),
                        ),
                    ],
                },
            )]),
            config_entries: BTreeMap::new(),
            dependency_model_aliases: IndexMap::new(),
        };
        let graph = ResolvedGraph {
            nodes: IndexMap::new(),
            order: Vec::new(),
            filters: HashMap::new(),
            version_constraints: std::collections::HashMap::new(),
        };
        let applied = ApplyResult {
            outcomes: vec![ActionOutcome {
                item_id: ItemId {
                    kind: ItemKind::Agent,
                    name: "coder".into(),
                },
                action: ActionTaken::Updated,
                dest_path: "agents/coder.md".into(),
                source_name: "base".into(),
                source_checksum: Some("sha256:new-src".into()),
                installed_checksum: Some("sha256:new-mars".into()),
            }],
        };

        let new_lock = build(
            &graph,
            &applied,
            &old_lock,
            std::collections::BTreeMap::new(),
        )
        .unwrap();

        assert!(!new_lock.items.contains_key("agent/agents/coder.md"));
        assert!(new_lock.contains_output(".mars", "agents/coder.md"));
        assert!(new_lock.contains_output(".claude", "agents/coder.md"));
        let item = &new_lock.items["agent/coder"];
        assert_eq!(item.source_checksum, "sha256:new-src");
        let claude = item
            .outputs
            .iter()
            .find(|o| o.target_root == ".claude")
            .unwrap();
        assert_eq!(
            claude.installed_checksum().expect("installed output"),
            "sha256:old-claude"
        );
    }

    #[test]
    fn apply_apply_outcomes_write_fallback_carries_non_canonical_outputs() {
        let old_lock = LockFile {
            version: LOCK_VERSION,
            dependencies: IndexMap::new(),
            items: IndexMap::from([(
                "agent/agents/coder.md".to_string(),
                LockedItemV2 {
                    source: "base".into(),
                    kind: ItemKind::Agent,
                    version: None,
                    source_checksum: "sha256:old-src".into(),
                    outputs: vec![
                        OutputRecord::installed(
                            ".mars".to_string(),
                            "agents/coder.md".into(),
                            "sha256:old-mars".into(),
                        ),
                        OutputRecord::installed(
                            ".claude".to_string(),
                            "agents/coder.md".into(),
                            "sha256:old-claude".into(),
                        ),
                    ],
                },
            )]),
            config_entries: BTreeMap::new(),
            dependency_model_aliases: IndexMap::new(),
        };
        let mut lock = old_lock.clone();

        apply_apply_outcomes_to_lock(
            &mut lock,
            &old_lock,
            &[ActionOutcome {
                item_id: ItemId {
                    kind: ItemKind::Agent,
                    name: "coder".into(),
                },
                action: ActionTaken::Updated,
                dest_path: "agents/coder.md".into(),
                source_name: "base".into(),
                source_checksum: Some("sha256:new-src".into()),
                installed_checksum: Some("sha256:new-mars".into()),
            }],
        );

        assert!(!lock.items.contains_key("agent/agents/coder.md"));
        assert!(lock.contains_output(".mars", "agents/coder.md"));
        assert!(lock.contains_output(".claude", "agents/coder.md"));
        let item = &lock.items["agent/coder"];
        assert_eq!(item.source_checksum, "sha256:new-src");
    }

    #[test]
    fn apply_apply_outcomes_to_lock_updated_preserves_non_canonical_outputs() {
        let mut old_lock = sample_lock();
        old_lock
            .items
            .get_mut("agent/coder")
            .unwrap()
            .outputs
            .push(OutputRecord::installed(
                ".claude".to_string(),
                "agents/coder.md".into(),
                "sha256:claude".into(),
            ));

        let mut lock = old_lock.clone();
        apply_apply_outcomes_to_lock(
            &mut lock,
            &old_lock,
            &[ActionOutcome {
                item_id: ItemId {
                    kind: ItemKind::Agent,
                    name: ItemName::from("coder"),
                },
                action: ActionTaken::Updated,
                dest_path: "agents/coder.md".into(),
                source_name: "base".into(),
                source_checksum: Some("sha256:new-src".into()),
                installed_checksum: Some("sha256:new-mars".into()),
            }],
        );

        assert!(lock.contains_output(".mars", "agents/coder.md"));
        assert!(lock.contains_output(".claude", "agents/coder.md"));
        let item = &lock.items["agent/coder"];
        assert_eq!(item.source_checksum, "sha256:new-src");
        let mars = item
            .outputs
            .iter()
            .find(|o| o.target_root == ".mars")
            .unwrap();
        assert_eq!(
            mars.installed_checksum().expect("installed output"),
            "sha256:new-mars"
        );
        let claude = item
            .outputs
            .iter()
            .find(|o| o.target_root == ".claude")
            .unwrap();
        assert_eq!(
            claude.installed_checksum().expect("installed output"),
            "sha256:claude"
        );
    }

    #[test]
    fn ownership_lock_for_native_emission_seeds_new_apply_outcomes() {
        let old_lock = LockFile::empty();
        let apply_outcomes = vec![ActionOutcome {
            item_id: ItemId {
                kind: ItemKind::Agent,
                name: ItemName::from("coder"),
            },
            action: ActionTaken::Installed,
            dest_path: "agents/coder.md".into(),
            source_name: "base".into(),
            source_checksum: Some("sha256:src".into()),
            installed_checksum: Some("sha256:mars".into()),
        }];
        let view = ownership_lock_for_native_emission(
            &old_lock,
            &apply_outcomes,
            &[crate::target_sync::TargetSyncOutcome {
                target: ".cursor".to_string(),
                items_synced: 1,
                items_removed: 0,
                errors: Vec::new(),
                synced_outputs: vec![crate::target_sync::TargetSyncedOutput {
                    dest_path: "agents/coder.md".to_string(),
                    installed_checksum: "sha256:cursor".into(),
                }],
                removed_dest_paths: Vec::new(),
            }],
        );
        assert!(view.contains_output(".mars", "agents/coder.md"));
        assert!(view.contains_output(".cursor", "agents/coder.md"));
        assert!(!old_lock.contains_output(".mars", "agents/coder.md"));
    }

    #[test]
    fn ownership_lock_after_target_sync_layers_synced_outputs() {
        let lock = sample_lock();
        let view = ownership_lock_after_target_sync(
            &lock,
            &[crate::target_sync::TargetSyncOutcome {
                target: ".cursor".to_string(),
                items_synced: 1,
                items_removed: 0,
                errors: Vec::new(),
                synced_outputs: vec![crate::target_sync::TargetSyncedOutput {
                    dest_path: "agents/coder.md".to_string(),
                    installed_checksum: "sha256:cursor".into(),
                }],
                removed_dest_paths: Vec::new(),
            }],
        );
        assert!(view.contains_output(".cursor", "agents/coder.md"));
        assert!(!lock.contains_output(".cursor", "agents/coder.md"));
    }

    #[test]
    fn apply_target_sync_outputs_upserts_and_removes_target_records() {
        let mut lock = sample_lock();
        apply_target_sync_outputs(
            &mut lock,
            &[crate::target_sync::TargetSyncOutcome {
                target: ".cursor".to_string(),
                items_synced: 1,
                items_removed: 0,
                errors: Vec::new(),
                synced_outputs: vec![crate::target_sync::TargetSyncedOutput {
                    dest_path: "agents/coder.md".to_string(),
                    installed_checksum: "sha256:cursor".into(),
                }],
                removed_dest_paths: Vec::new(),
            }],
        );
        assert!(lock.contains_output(".cursor", "agents/coder.md"));

        apply_target_sync_outputs(
            &mut lock,
            &[crate::target_sync::TargetSyncOutcome {
                target: ".cursor".to_string(),
                items_synced: 0,
                items_removed: 1,
                errors: Vec::new(),
                synced_outputs: Vec::new(),
                removed_dest_paths: vec!["agents/coder.md".to_string()],
            }],
        );
        assert!(!lock.contains_output(".cursor", "agents/coder.md"));
        assert!(lock.contains_output(".mars", "agents/coder.md"));
    }

    #[test]
    fn canonical_flat_items_excludes_linked_target_outputs() {
        let mut lock = sample_lock();
        lock.items
            .get_mut("agent/coder")
            .unwrap()
            .outputs
            .push(OutputRecord::installed(
                ".cursor".to_string(),
                "agents/coder.md".into(),
                "sha256:cursor".into(),
            ));

        let canonical = lock.canonical_flat_items();
        assert_eq!(canonical.len(), 2);
        assert!(
            canonical
                .iter()
                .any(|(dp, _)| dp.as_str() == "agents/coder.md")
        );
        assert!(
            canonical
                .iter()
                .all(|(_, item)| { lock.contains_output(".mars", item.dest_path.as_str()) })
        );

        let cursor = lock.flat_items_for_target(".cursor");
        assert_eq!(cursor.len(), 1);
        assert_eq!(cursor[0].0.as_str(), "agents/coder.md");
    }

    #[test]
    fn build_uses_graph_provenance_for_sources() {
        let git_name: SourceName = "base".into();
        let path_name: SourceName = "local".into();
        let git_url: SourceUrl = "https://example.com/new.git".into();
        let path_canonical = PathBuf::from("/tmp/mars-agents-local-source");

        let mut nodes = IndexMap::new();
        nodes.insert(
            git_name.clone(),
            ResolvedNode {
                source_name: git_name.clone(),
                source_id: SourceId::git_with_subpath(
                    git_url.clone(),
                    Some(crate::types::SourceSubpath::new("plugins/base").unwrap()),
                ),
                rooted_ref: crate::resolve::RootedSourceRef {
                    checkout_root: PathBuf::from("/tmp/cache/base"),
                    package_root: PathBuf::from("/tmp/cache/base/plugins/base"),
                },
                resolved_ref: ResolvedRef {
                    source_name: git_name.clone(),
                    version: Some(semver::Version::new(1, 2, 3)),
                    version_tag: Some("v1.2.3".into()),
                    commit: Some("abc123".into()),
                    tree_path: PathBuf::from("/tmp/cache/base"),
                },
                manifest: None,
                deps: vec![],
            },
        );
        nodes.insert(
            path_name.clone(),
            ResolvedNode {
                source_name: path_name.clone(),
                source_id: SourceId::Path {
                    canonical: path_canonical.clone(),
                    subpath: Some(crate::types::SourceSubpath::new("plugins/local").unwrap()),
                },
                rooted_ref: crate::resolve::RootedSourceRef {
                    checkout_root: PathBuf::from("/tmp/cache/local"),
                    package_root: PathBuf::from("/tmp/cache/local/plugins/local"),
                },
                resolved_ref: ResolvedRef {
                    source_name: path_name.clone(),
                    version: None,
                    version_tag: None,
                    commit: None,
                    tree_path: PathBuf::from("/tmp/cache/local"),
                },
                manifest: None,
                deps: vec![],
            },
        );

        let graph = ResolvedGraph {
            nodes,
            order: vec![git_name.clone(), path_name.clone()],
            filters: HashMap::new(),
            version_constraints: std::collections::HashMap::new(),
        };
        let applied = ApplyResult { outcomes: vec![] };

        let mut old_sources = IndexMap::new();
        old_sources.insert(
            git_name.clone(),
            LockedSource {
                url: Some("https://example.com/old.git".into()),
                path: None,
                subpath: None,
                version: Some("v0.0.1".into()),
                commit: Some("deadbeef".into()),
            },
        );
        let old_lock = LockFile {
            version: LOCK_VERSION,
            dependencies: old_sources,
            items: IndexMap::new(),
            config_entries: std::collections::BTreeMap::new(),
            dependency_model_aliases: IndexMap::new(),
        };

        let new_lock = build(
            &graph,
            &applied,
            &old_lock,
            std::collections::BTreeMap::new(),
        )
        .unwrap();

        let base = &new_lock.dependencies["base"];
        assert_eq!(base.url.as_ref(), Some(&git_url));
        assert_eq!(
            base.subpath
                .as_ref()
                .map(crate::types::SourceSubpath::as_str),
            Some("plugins/base")
        );
        assert_eq!(base.version.as_deref(), Some("v1.2.3"));
        assert_eq!(base.commit.as_deref(), Some("abc123"));

        let local = &new_lock.dependencies["local"];
        assert!(local.url.is_none());
        assert_eq!(
            local
                .subpath
                .as_ref()
                .map(crate::types::SourceSubpath::as_str),
            Some("plugins/local")
        );
        assert_eq!(
            local.path.as_deref(),
            Some(path_canonical.to_string_lossy().as_ref())
        );
    }

    #[test]
    fn build_persists_ref_selector_in_locked_source_version() {
        let source_name: SourceName = "base".into();
        let mut nodes = IndexMap::new();
        nodes.insert(
            source_name.clone(),
            ResolvedNode {
                source_name: source_name.clone(),
                source_id: SourceId::git_with_subpath("https://example.com/base.git".into(), None),
                rooted_ref: crate::resolve::RootedSourceRef {
                    checkout_root: PathBuf::from("/tmp/cache/base"),
                    package_root: PathBuf::from("/tmp/cache/base"),
                },
                resolved_ref: ResolvedRef {
                    source_name: source_name.clone(),
                    version: None,
                    version_tag: Some("main".into()),
                    commit: Some("abc123".into()),
                    tree_path: PathBuf::from("/tmp/cache/base"),
                },
                manifest: None,
                deps: vec![],
            },
        );

        let graph = ResolvedGraph {
            nodes,
            order: vec![source_name.clone()],
            filters: HashMap::new(),
            version_constraints: std::collections::HashMap::new(),
        };
        let applied = ApplyResult { outcomes: vec![] };
        let new_lock = build(
            &graph,
            &applied,
            &LockFile::empty(),
            std::collections::BTreeMap::new(),
        )
        .unwrap();

        let source = &new_lock.dependencies["base"];
        assert_eq!(source.version.as_deref(), Some("main"));
        assert_eq!(source.commit.as_deref(), Some("abc123"));
    }

    #[test]
    fn build_keeps_self_items_from_old_lock_on_skipped_action() {
        let graph = ResolvedGraph {
            nodes: IndexMap::new(),
            order: Vec::new(),
            filters: HashMap::new(),
            version_constraints: std::collections::HashMap::new(),
        };
        let local_source_name: SourceName = SourceOrigin::LocalPackage.to_string().into();
        let old_lock = LockFile {
            version: LOCK_VERSION,
            dependencies: IndexMap::from([(
                local_source_name.clone(),
                LockedSource {
                    url: None,
                    path: Some(".".into()),
                    subpath: None,
                    version: None,
                    commit: None,
                },
            )]),
            items: IndexMap::from([(
                "skill/local-skill".to_string(),
                LockedItemV2 {
                    source: local_source_name.clone(),
                    kind: ItemKind::Skill,
                    version: None,
                    source_checksum: "sha256:self".into(),
                    outputs: vec![OutputRecord::installed(
                        ".mars".to_string(),
                        DestPath::from("skills/local-skill"),
                        "sha256:self".into(),
                    )],
                },
            )]),
            config_entries: std::collections::BTreeMap::new(),
            dependency_model_aliases: IndexMap::new(),
        };
        let applied = ApplyResult {
            outcomes: vec![ActionOutcome {
                item_id: ItemId {
                    kind: ItemKind::Skill,
                    name: "local-skill".into(),
                },
                action: ActionTaken::Skipped,
                dest_path: "skills/local-skill".into(),
                source_name: local_source_name.clone(),
                source_checksum: None,
                installed_checksum: None,
            }],
        };

        let new_lock = build(
            &graph,
            &applied,
            &old_lock,
            std::collections::BTreeMap::new(),
        )
        .unwrap();

        assert!(
            new_lock
                .dependencies
                .contains_key(local_source_name.as_str())
        );
        let item = &new_lock.items["skill/local-skill"];
        assert_eq!(item.source, local_source_name);
        assert_eq!(item.kind, ItemKind::Skill);
        assert_eq!(item.source_checksum, "sha256:self");
        assert_eq!(
            item.outputs[0]
                .installed_checksum()
                .expect("installed output"),
            "sha256:self"
        );
    }

    #[test]
    fn build_rejects_missing_installed_checksum_for_write_actions() {
        let graph = ResolvedGraph {
            nodes: IndexMap::new(),
            order: Vec::new(),
            filters: HashMap::new(),
            version_constraints: std::collections::HashMap::new(),
        };
        let old_lock = LockFile::empty();
        let applied = ApplyResult {
            outcomes: vec![ActionOutcome {
                item_id: ItemId {
                    kind: ItemKind::Agent,
                    name: "coder".into(),
                },
                action: ActionTaken::Installed,
                dest_path: "agents/coder.md".into(),
                source_name: "base".into(),
                source_checksum: Some("sha256:source".into()),
                installed_checksum: None,
            }],
        };

        let err = build(
            &graph,
            &applied,
            &old_lock,
            std::collections::BTreeMap::new(),
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("missing checksum for write-producing action"));
        assert!(msg.contains("agents/coder.md"));
    }

    #[test]
    fn build_rejects_empty_checksums_from_carried_items() {
        let graph = ResolvedGraph {
            nodes: IndexMap::new(),
            order: Vec::new(),
            filters: HashMap::new(),
            version_constraints: std::collections::HashMap::new(),
        };
        let old_lock = LockFile {
            version: LOCK_VERSION,
            dependencies: IndexMap::new(),
            items: IndexMap::from([(
                "agent/coder".to_string(),
                LockedItemV2 {
                    source: "base".into(),
                    kind: ItemKind::Agent,
                    version: None,
                    source_checksum: "".into(),
                    outputs: vec![OutputRecord::installed(
                        ".mars".to_string(),
                        DestPath::from("agents/coder.md"),
                        "sha256:installed".into(),
                    )],
                },
            )]),
            config_entries: std::collections::BTreeMap::new(),
            dependency_model_aliases: IndexMap::new(),
        };
        let applied = ApplyResult {
            outcomes: vec![ActionOutcome {
                item_id: ItemId {
                    kind: ItemKind::Agent,
                    name: "coder".into(),
                },
                action: ActionTaken::Skipped,
                dest_path: "agents/coder.md".into(),
                source_name: "base".into(),
                source_checksum: None,
                installed_checksum: None,
            }],
        };

        let err = build(
            &graph,
            &applied,
            &old_lock,
            std::collections::BTreeMap::new(),
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("empty source_checksum"));
    }
}

#[cfg(test)]
mod output_lifecycle_contract_tests {
    use super::{OutputRecord, OutputState};

    #[test]
    fn pending_deletion_record_carries_no_checksum() {
        let record = OutputRecord::pending_deletion(".opencode", "plugins/mars-audit.ts");

        assert!(matches!(record.state, OutputState::PendingDeletion));
        let encoded = toml::to_string(&record).expect("pending record serializes");
        assert!(!encoded.contains("installed_checksum"));
    }
}
