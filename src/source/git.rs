//! Git source adapter primitives.

use crate::error::MarsError;
use crate::source::{AvailableVersion, GlobalCache, ResolvedRef};
use crate::types::CommitHash;

/// Options controlling git fetch behavior.
#[derive(Debug, Clone, Default)]
pub struct FetchOptions {
    /// Preferred commit SHA to checkout before resolving tags/versions.
    /// Used for lock replay to guarantee reproducible content.
    pub preferred_commit: Option<CommitHash>,
}

/// Normalize a git URL to a filesystem-safe directory name.
///
/// Strips protocol prefixes and replaces `/` and `:` with `_`.
/// Strips trailing `.git` suffix.
///
/// Examples:
/// - `https://github.com/foo/bar` -> `github.com_foo_bar`
/// - `github.com/foo/bar` -> `github.com_foo_bar`
/// - `git@github.com:foo/bar.git` -> `github.com_foo_bar`
/// - `ssh://git@github.com/foo/bar` -> `github.com_foo_bar`
pub fn url_to_dirname(url: &str) -> String {
    let mut s = url.to_string();

    // Strip common protocol prefixes
    for prefix in &["https://", "http://", "ssh://", "git://"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.to_string();
            break;
        }
    }

    // Handle SSH shorthand: git@github.com:foo/bar -> github.com/foo/bar
    if let Some(rest) = s.strip_prefix("git@") {
        s = rest.to_string();
        if let Some(colon_pos) = s.find(':') {
            let after_colon = &s[colon_pos + 1..];
            if !after_colon.starts_with("//") {
                s.replace_range(colon_pos..colon_pos + 1, "/");
            }
        }
    }

    // Strip trailing .git
    if let Some(rest) = s.strip_suffix(".git") {
        s = rest.to_string();
    }

    // Strip trailing slash
    if let Some(rest) = s.strip_suffix('/') {
        s = rest.to_string();
    }

    // Replace `/` with `_`
    s.replace('/', "_")
}

/// Parse a tag name as a semver version tag.
///
/// Accepts: `v1.0.0`, `v0.5.2`, `1.0.0`
/// Rejects: `latest`, `nightly-2024`, or any non-semver tag.
#[allow(dead_code)]
fn parse_semver_tag(tag: &str) -> Option<semver::Version> {
    let version_str = tag.strip_prefix('v').unwrap_or(tag);
    semver::Version::parse(version_str).ok()
}

pub fn list_versions(_url: &str, _cache: &GlobalCache) -> Result<Vec<AvailableVersion>, MarsError> {
    todo!("Phase 2: implement remote version listing");
}

pub fn fetch(
    _url: &str,
    _version_req: Option<&str>,
    _source_name: &str,
    _cache: &GlobalCache,
    _options: &FetchOptions,
) -> Result<ResolvedRef, MarsError> {
    todo!("Phase 2: implement source fetch");
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== url_to_dirname tests ====================

    #[test]
    fn url_to_dirname_https() {
        assert_eq!(
            url_to_dirname("https://github.com/foo/bar"),
            "github.com_foo_bar"
        );
    }

    #[test]
    fn url_to_dirname_bare_domain() {
        assert_eq!(
            url_to_dirname("github.com/haowjy/meridian-base"),
            "github.com_haowjy_meridian-base"
        );
    }

    #[test]
    fn url_to_dirname_ssh() {
        assert_eq!(
            url_to_dirname("git@github.com:foo/bar.git"),
            "github.com_foo_bar"
        );
    }

    #[test]
    fn url_to_dirname_https_with_git_suffix() {
        assert_eq!(
            url_to_dirname("https://github.com/foo/bar.git"),
            "github.com_foo_bar"
        );
    }

    #[test]
    fn url_to_dirname_ssh_protocol() {
        assert_eq!(
            url_to_dirname("ssh://git@github.com/foo/bar"),
            "github.com_foo_bar"
        );
    }

    #[test]
    fn url_to_dirname_http() {
        assert_eq!(
            url_to_dirname("http://gitlab.com/org/repo"),
            "gitlab.com_org_repo"
        );
    }

    #[test]
    fn url_to_dirname_trailing_slash() {
        assert_eq!(
            url_to_dirname("https://github.com/foo/bar/"),
            "github.com_foo_bar"
        );
    }

    // ==================== parse_semver_tag tests ====================

    #[test]
    fn parse_semver_v_prefixed() {
        let v = parse_semver_tag("v1.2.3").unwrap();
        assert_eq!(v, semver::Version::new(1, 2, 3));
    }

    #[test]
    fn parse_semver_no_prefix() {
        let v = parse_semver_tag("0.5.2").unwrap();
        assert_eq!(v, semver::Version::new(0, 5, 2));
    }

    #[test]
    fn parse_semver_prerelease() {
        let v = parse_semver_tag("v2.0.0-rc.1").unwrap();
        assert_eq!(v.major, 2);
        assert!(!v.pre.is_empty());
    }

    #[test]
    fn parse_semver_rejects_non_semver() {
        assert!(parse_semver_tag("latest").is_none());
        assert!(parse_semver_tag("nightly-2024").is_none());
        assert!(parse_semver_tag("release").is_none());
    }
}
