//! Cache directory naming and root resolution.
//!
//! Generates filesystem-safe cache keys from URLs and external identifiers.

use std::path::PathBuf;

use crate::error::MarsError;

/// Generate a filesystem-safe single path component from an external identifier.
///
/// Rules (applied on all platforms for cross-platform determinism):
/// - Replace `/`, `\`, `:`, `<`, `>`, `"`, `|`, `?`, `*`, ASCII control chars, NUL with `_`
/// - Avoid trailing space or dot
/// - Avoid Windows reserved device names (CON, PRN, AUX, NUL, COM1-9, LPT1-9)
/// - Truncate to 200 bytes
pub fn safe_component(raw: &str) -> String {
    // Placeholder - will be implemented in Slice 2
    // For now, delegate to existing url_to_dirname behavior
    raw.replace(['/', '\\', ':'], "_")
}

/// Generate a safe component with a hash suffix to prevent collisions.
///
/// Returns: `{safe_component(raw, prefix_chars=60)}_{hex8(sha256(raw))}`
pub fn safe_component_with_hash(raw: &str) -> String {
    // Placeholder - will be implemented in Slice 2
    safe_component(raw)
}

/// Generate a cache directory component for a git clone URL.
pub fn git_cache_component(url: &str) -> Result<String, MarsError> {
    // Placeholder - will be implemented in Slice 2
    // For now, use existing url_to_dirname
    Ok(crate::source::git::url_to_dirname(url))
}

/// Generate a cache directory component for an archive URL + SHA.
pub fn archive_cache_component(url: &str, sha: &str) -> Result<String, MarsError> {
    // Placeholder - will be implemented in Slice 2
    Ok(format!(
        "{}_{}",
        safe_component(url),
        &sha[..8.min(sha.len())]
    ))
}

/// Resolve the global cache root directory.
///
/// Resolution order:
/// 1. `MARS_CACHE_DIR` env var
/// 2. OS cache directory + `mars/cache`
/// 3. `{cwd}/.mars/cache` fallback
pub fn global_cache_root() -> Result<PathBuf, MarsError> {
    // Placeholder - will be implemented in Slice 3
    // For now, delegate to existing GlobalCache::new logic
    if let Some(cache_dir) = std::env::var_os("MARS_CACHE_DIR") {
        Ok(PathBuf::from(cache_dir))
    } else if let Some(home) = dirs::home_dir() {
        Ok(home.join(".mars").join("cache"))
    } else {
        Ok(std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(".mars")
            .join("cache"))
    }
}
