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
    Ok(safe_component_with_hash(normalize_git_url(url)))
}

/// Generate a cache directory component for an archive URL + SHA.
pub fn archive_cache_component(url: &str, sha: &str) -> Result<String, MarsError> {
    let combined = format!("{url}@{sha}");
    Ok(safe_component_with_hash(&combined))
}

/// Normalize a git URL for cache key generation.
///
/// Strips protocol prefixes, handles SSH shorthand, strips .git suffix.
fn normalize_git_url(url: &str) -> &str {
    let mut s = url;

    // Strip common protocol prefixes
    for prefix in &["https://", "http://", "ssh://", "git://"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest;
            break;
        }
    }

    // Handle SSH shorthand: git@github.com:foo/bar -> github.com:foo/bar
    // Keep the colon for now, safe_component will convert it.
    if let Some(rest) = s.strip_prefix("git@") {
        s = rest;
    }

    // Strip trailing .git
    if let Some(rest) = s.strip_suffix(".git") {
        s = rest;
    }

    // Strip trailing slash
    s.strip_suffix('/').unwrap_or(s)
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
    use serial_test::serial;
    use std::ffi::OsString;
    use std::path::Path;

    #[allow(unused_unsafe)]
    fn env_set(key: &str, value: &std::path::Path) {
        unsafe {
            std::env::set_var(key, value);
        }
    }

    #[allow(unused_unsafe)]
    fn env_remove(key: &str) {
        unsafe {
            std::env::remove_var(key);
        }
    }

    struct EnvVarGuard {
        key: String,
        prev: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set_path(key: &str, value: &std::path::Path) -> Self {
            let prev = std::env::var_os(key);
            env_set(key, value);
            Self {
                key: key.to_string(),
                prev,
            }
        }

        fn remove(key: &str) -> Self {
            let prev = std::env::var_os(key);
            env_remove(key);
            Self {
                key: key.to_string(),
                prev,
            }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(prev) = &self.prev {
                #[allow(unused_unsafe)]
                unsafe {
                    std::env::set_var(&self.key, prev);
                }
            } else {
                env_remove(&self.key);
            }
        }
    }

    #[test]
    fn safe_component_replaces_invalid_chars() {
        assert_eq!(safe_component("a/b\\c:d"), "a_b_c_d");
        assert_eq!(safe_component("file<>name"), "file__name");
        assert_eq!(safe_component("test|file"), "test_file");
    }

    #[test]
    fn safe_component_handles_trailing_space_dot() {
        assert_eq!(safe_component("test "), "test");
        assert_eq!(safe_component("test."), "test");
        assert_eq!(safe_component("test. "), "test");
    }

    #[test]
    fn safe_component_handles_reserved_names() {
        assert_eq!(safe_component("CON"), "CON_");
        assert_eq!(safe_component("con"), "con_");
        assert_eq!(safe_component("NUL"), "NUL_");
        assert_eq!(safe_component("COM1"), "COM1_");
        assert_eq!(safe_component("lpt9"), "lpt9_");
    }

    #[test]
    fn safe_component_handles_empty() {
        assert_eq!(safe_component(""), "_");
        assert_eq!(safe_component("..."), "_");
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

    #[test]
    fn git_cache_component_no_colon() {
        // Explicit port URL must not have colon in result.
        let result = git_cache_component("git://gitlab.localtest.me:19424/group/pkg.git").unwrap();
        assert!(
            !result.contains(':'),
            "cache component should not contain colon: {result}"
        );
    }

    #[test]
    fn git_cache_component_various_urls() {
        // All should produce valid components without colons.
        let urls = [
            "https://github.com/foo/bar",
            "git@github.com:foo/bar.git",
            "ssh://git@github.com/foo/bar",
            "git://host:1234/path.git",
        ];
        for url in urls {
            let result = git_cache_component(url).unwrap();
            assert!(
                !result.contains(':'),
                "URL {url} produced component with colon: {result}"
            );
        }
    }

    #[test]
    fn archive_cache_component_no_colon() {
        let result = archive_cache_component("https://host:8080/archive.tar.gz", "abc123").unwrap();
        assert!(
            !result.contains(':'),
            "archive component should not contain colon: {result}"
        );
    }

    #[test]
    #[serial]
    fn global_cache_root_respects_env_var() {
        let temp = std::env::temp_dir().join("mars-test-cache");
        let _guard = EnvVarGuard::set_path("MARS_CACHE_DIR", &temp);

        let root = global_cache_root().unwrap();
        assert_eq!(root, temp);
    }

    #[test]
    #[serial]
    fn global_cache_root_uses_os_cache_when_no_env() {
        let _guard = EnvVarGuard::remove("MARS_CACHE_DIR");

        let root = global_cache_root().unwrap();

        if let Some(cache_dir) = dirs::cache_dir() {
            assert_eq!(root, cache_dir.join("mars").join("cache"));
        } else {
            assert!(
                root.ends_with(Path::new(".mars").join("cache")),
                "fallback root should end with .mars/cache: {root:?}"
            );
        }
    }
}
