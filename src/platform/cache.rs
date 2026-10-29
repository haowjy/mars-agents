//! Cache directory naming and root resolution.
//!
//! Generates filesystem-safe cache keys from URLs and external identifiers.

use sha2::{Digest, Sha256};
use std::path::PathBuf;

use crate::error::MarsError;

/// Characters invalid in Windows path components.
const INVALID_CHARS: &[char] = &['/', '\\', ':', '<', '>', '"', '|', '?', '*'];

/// Windows reserved device names (case-insensitive).
const RESERVED_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Generate a filesystem-safe single path component from an external identifier.
///
/// Rules (applied on all platforms for cross-platform determinism):
/// - Replace `/`, `\`, `:`, `<`, `>`, `"`, `|`, `?`, `*`, ASCII control chars, NUL with `_`
/// - Avoid trailing space or dot
/// - Avoid Windows reserved device names (CON, PRN, AUX, NUL, COM1-9, LPT1-9)
/// - Truncate to 200 bytes
pub fn safe_component(raw: &str) -> String {
    let mut result = String::with_capacity(raw.len());

    for c in raw.chars() {
        if INVALID_CHARS.contains(&c) || c.is_ascii_control() || c == '\0' {
            result.push('_');
        } else {
            result.push(c);
        }
    }

    // Avoid trailing space or dot
    while result.ends_with(' ') || result.ends_with('.') {
        result.pop();
    }

    // Handle reserved names by appending underscore
    let upper = result.to_ascii_uppercase();
    for reserved in RESERVED_NAMES {
        if upper == *reserved || upper.starts_with(&format!("{reserved}.")) {
            result.push('_');
            break;
        }
    }

    // Truncate to 200 bytes (UTF-8 aware)
    if result.len() > 200 {
        let mut end = 200;
        while end > 0 && !result.is_char_boundary(end) {
            end -= 1;
        }
        result.truncate(end);
    }

    // Empty result becomes underscore
    if result.is_empty() {
        result.push('_');
    }

    result
}

/// Generate a safe component with a hash suffix to prevent collisions.
///
/// Returns: `{safe_component(raw, prefix_chars=60)}_{hex8(sha256(raw))}`
pub fn safe_component_with_hash(raw: &str) -> String {
    let prefix = safe_component(raw);

    // Truncate prefix to 60 chars for readable portion
    let prefix_truncated = if prefix.len() > 60 {
        let mut end = 60;
        while end > 0 && !prefix.is_char_boundary(end) {
            end -= 1;
        }
        &prefix[..end]
    } else {
        &prefix
    };

    // Compute SHA-256 hash and take first 8 hex chars
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    let hash = hasher.finalize();
    let hash_hex: String = hash.iter().take(4).map(|b| format!("{b:02x}")).collect();

    format!("{prefix_truncated}_{hash_hex}")
}

/// Generate a cache directory component for a git clone URL.
pub fn git_cache_component(url: &str) -> Result<String, MarsError> {
    Ok(safe_component_with_hash(
        &mars_schema::git_url::canonicalize_git_url(url),
    ))
}

/// Generate a cache directory component for an archive URL + SHA.
pub fn archive_cache_component(url: &str, sha: &str) -> Result<String, MarsError> {
    let combined = format!("{url}@{sha}");
    Ok(safe_component_with_hash(&combined))
}

/// Resolve the global cache root directory.
///
/// Resolution order:
/// 1. `MARS_CACHE_DIR` env var
/// 2. OS cache directory + `mars/cache`
/// 3. `{cwd}/.mars/cache` fallback
pub fn global_cache_root() -> Result<PathBuf, MarsError> {
    if let Some(cache_dir) = std::env::var_os("MARS_CACHE_DIR") {
        return Ok(PathBuf::from(cache_dir));
    }

    if let Some(cache_dir) = dirs::cache_dir() {
        return Ok(cache_dir.join("mars").join("cache"));
    }

    Ok(std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".mars")
        .join("cache"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_component_replaces_invalid_chars() {
        for (input, expected) in [
            ("a/b\\c:d", "a_b_c_d"),
            ("file<>name", "file__name"),
            ("test|file", "test_file"),
            ("test ", "test"),
            ("test.", "test"),
            ("test. ", "test"),
            ("CON", "CON_"),
            ("con", "con_"),
            ("NUL", "NUL_"),
            ("COM1", "COM1_"),
            ("lpt9", "lpt9_"),
            ("", "_"),
            ("...", "_"),
        ] {
            assert_eq!(safe_component(input), expected, "{input:?}");
        }
    }

    #[test]
    fn safe_component_with_hash_prevents_collisions() {
        // These would collide without hash.
        let a = safe_component_with_hash("a:b");
        let b = safe_component_with_hash("a/b");
        let c = safe_component_with_hash("a_b");

        // All different due to hash suffix.
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
    }
}
