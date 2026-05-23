//! Shared probe cache refresh semantics for pi / opencode / cursor caches.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProbeRefreshMode {
    /// Stale usable cache → return stale and spawn detached `__refresh-probe`.
    #[default]
    Background,
    /// Stale or miss → run probe in-process; never spawn background refresh.
    Synchronous,
    /// No probe subprocess; use disk only (stale if present, else unavailable).
    Skip,
}

impl ProbeRefreshMode {
    pub fn should_spawn_background_on_stale(self, mars_offline: bool) -> bool {
        !mars_offline && self == Self::Background
    }

    pub fn should_sync_probe_on_stale(self, mars_offline: bool) -> bool {
        !mars_offline && self == Self::Synchronous
    }

    pub fn blocks_cold_probe(self, mars_offline: bool) -> bool {
        mars_offline || self == Self::Skip
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeCacheBranch<T> {
    Hit(T),
    Stale(T),
    Unavailable,
    SynchronousProbe,
}

pub fn resolve_probe_cache_branch<Entry, Result, CachedResult, IsFresh, TriggerBackground>(
    cached: Option<Entry>,
    mars_offline: bool,
    probe_refresh: ProbeRefreshMode,
    cached_result: CachedResult,
    is_fresh: IsFresh,
    trigger_background: TriggerBackground,
) -> ProbeCacheBranch<Result>
where
    Result: Clone,
    CachedResult: Fn(&Entry) -> Option<&Result>,
    IsFresh: Fn(&Entry) -> bool,
    TriggerBackground: FnOnce(),
{
    if let Some(entry) = cached
        && let Some(result) = cached_result(&entry)
    {
        if is_fresh(&entry) {
            return ProbeCacheBranch::Hit(result.clone());
        }
        if probe_refresh.should_sync_probe_on_stale(mars_offline) {
            return ProbeCacheBranch::SynchronousProbe;
        }
        let result = result.clone();
        if probe_refresh.should_spawn_background_on_stale(mars_offline) {
            trigger_background();
        }
        return ProbeCacheBranch::Stale(result);
    }

    if probe_refresh.blocks_cold_probe(mars_offline) {
        ProbeCacheBranch::Unavailable
    } else {
        ProbeCacheBranch::SynchronousProbe
    }
}
