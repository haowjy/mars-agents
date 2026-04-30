/// Shared IR types — the boundary between the reader and compiler stages.
///
/// `ReaderIr` is the single handoff value from `reader::read()` to
/// `compiler::compile()`.  It contains source-level facts only; no
/// destination paths, rendered bytes, lock records, or sync-plan state may
/// appear here.
use crate::local_source::LocalDiscoveredItem;
use crate::sync::ResolvedState;

/// The single boundary type between reader and compiler.
///
/// Invariants (enforced by construction in `reader::read`):
/// - No `DestPath` fields.
/// - No lowered output bytes or rendered content.
/// - No lock item records (only the *old* lock for diff purposes).
/// - No sync-plan or apply state.
pub struct ReaderIr {
    /// Fully resolved pipeline state (config, graph, model aliases, lock, sync lock).
    pub resolved: ResolvedState,
    /// Local package items discovered from the project root.
    /// Source paths only — dest-path assignment happens in the compiler.
    pub local_items: Vec<LocalDiscoveredItem>,
}
