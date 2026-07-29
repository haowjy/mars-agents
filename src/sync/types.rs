//! Shared types for the sync pipeline.

/// Options controlling sync behavior.
#[derive(Debug, Clone, Default)]
pub struct SyncOptions {
    /// Force overwrite local modifications and unmanaged collisions.
    pub force: bool,
    /// Compute plan but don't execute (dry run).
    pub dry_run: bool,
    /// Error if lock file would change (CI mode).
    pub frozen: bool,
    /// Force synchronous models cache + probe refresh.
    pub refresh_models: bool,
    /// Skip automatic models cache refresh.
    pub no_refresh_models: bool,
    /// Fetch version metadata so sync can report available upgrades.
    pub check_upgrades: bool,
    /// Skip package `requires-mars` compatibility checks.
    pub ignore_requires_mars: bool,
    /// Skip package `requires-meridian` compatibility checks.
    pub ignore_requires_meridian: bool,
}

#[cfg(test)]
mod tests {
    use super::SyncOptions;

    #[test]
    fn default_no_refresh_models_is_false() {
        let options = SyncOptions::default();
        assert!(!options.no_refresh_models);
        assert!(!options.refresh_models);
    }
}
