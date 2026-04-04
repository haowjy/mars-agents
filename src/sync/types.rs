//! Shared types for the sync pipeline.

/// Options controlling sync behavior.
#[derive(Debug, Clone, Default)]
pub struct SyncOptions {
    /// Force overwrite on conflicts (skip merge).
    pub force: bool,
    /// Compute plan but don't execute (dry run).
    pub dry_run: bool,
    /// Error if lock file would change (CI mode).
    pub frozen: bool,
}
