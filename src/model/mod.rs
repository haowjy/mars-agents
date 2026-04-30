/// Shared IR types — the boundary between the reader and compiler stages.
///
/// `ReaderIr` is the single handoff value from `reader::read()` to
/// `compiler::compile()`.  It contains source-level facts only; no
/// destination paths, rendered bytes, lock records, or sync-plan state may
/// appear here.
use indexmap::IndexMap;

use crate::config::{Config, EffectiveConfig, LocalConfig};
use crate::fs::FileLock;
use crate::local_source::LocalDiscoveredItem;
use crate::lock::LockFile;
use crate::models::ModelAlias;
use crate::resolve::ResolvedGraph;
use crate::sync::mutation::DependencyUpsertChange;

/// The single boundary type between reader and compiler.
///
/// Invariants (enforced by construction in `reader::read`):
/// - No `DestPath` fields.
/// - No lowered output bytes or rendered content.
/// - No lock item records (only the *old* lock for diff purposes).
/// - No sync-plan or apply state.
pub struct ReaderIr {
    /// Merged effective config (direct + local overrides).
    pub config: EffectiveConfig,
    /// Raw local-override config (needed for mutation persistence).
    pub local_config: LocalConfig,
    /// Raw base config (needed for mutation persistence and bump entries).
    pub raw_config: Config,
    /// Existing lock file on disk before this sync run.
    pub old_lock: LockFile,
    /// Fully resolved dependency graph.
    pub graph: ResolvedGraph,
    /// Merged model aliases (consumer + all dependency manifests).
    pub model_aliases: IndexMap<String, ModelAlias>,
    /// Configured managed-target root names (e.g. `[".agents"]`).
    pub target_registry: Vec<String>,
    /// Dependency mutations to record in the sync report.
    pub dependency_changes: Vec<DependencyUpsertChange>,
    /// Local package items discovered from the project root.
    /// Source paths only — dest-path assignment happens in the compiler.
    pub local_items: Vec<LocalDiscoveredItem>,
    /// Holds the sync file-lock for the lifetime of the pipeline.
    pub(crate) _sync_lock: FileLock,
}
