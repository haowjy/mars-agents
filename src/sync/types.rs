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
    /// Skip automatic models cache refresh.
    pub no_refresh_models: bool,
}

#[cfg(test)]
mod tests {
    use super::SyncOptions;

    #[test]
    fn default_no_refresh_models_is_false() {
        assert!(!SyncOptions::default().no_refresh_models);
    }
}
