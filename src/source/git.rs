//! Git source adapter — strategy and public API.
//!
//! Delegates to `git_cli` for git CLI operations and `archive` for
//! GitHub archive download/extraction.

use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::source::parse::extract_hostname;
use crate::source::{AvailableVersion, GlobalCache, ResolvedRef};
use crate::types::CommitHash;

use super::archive;
use super::git_cli;

#[derive(Debug, Clone, Default)]
pub struct FetchOptions {
    /// Preferred commit SHA to checkout before resolving tags/versions.
    /// Used for lock replay to guarantee reproducible content.
    pub preferred_commit: Option<CommitHash>,
}
#[derive(Debug, Clone)]
pub(crate) struct ResolvedVersion {
    pub tag: Option<String>,
    pub version: Option<semver::Version>,
    pub sha: String,
}

fn resolve_version(
    url: &str,
    version_req: Option<&str>,
    diag: &mut DiagnosticCollector,
) -> Result<ResolvedVersion, MarsError> {
    if let Some(version_req) = version_req {
        if let Some(requested_version) = mars_schema::version::parse_tag(version_req) {
            let tags = ls_remote_tags(url)?;
            let selected = tags
                .into_iter()
                .find(|tag| tag.tag == version_req || tag.version == requested_version)
                .ok_or_else(|| MarsError::Source {
                    source_name: url.to_string(),
                    message: format!("version tag `{version_req}` not found"),
                })?;

            return Ok(ResolvedVersion {
                tag: Some(selected.tag),
                version: Some(selected.version),
                sha: selected.commit_id,
            });
        }

        let sha = ls_remote_ref(url, version_req)?;
        return Ok(ResolvedVersion {
            tag: None,
            version: None,
            sha,
        });
    }

    let tags = ls_remote_tags(url)?;
    if let Some(selected) = tags.last() {
        return Ok(ResolvedVersion {
            tag: Some(selected.tag.clone()),
            version: Some(selected.version.clone()),
            sha: selected.commit_id.clone(),
        });
    }

    diag.warn_advisory(
        "no-releases",
        format!("no releases found for {url}, using latest commit from default branch"),
    );
    let sha = ls_remote_head(url)?;
    Ok(ResolvedVersion {
        tag: None,
        version: None,
        sha,
    })
}

pub fn is_github_host(url: &str) -> bool {
    extract_hostname(url)
        .map(|host| host.eq_ignore_ascii_case("github.com"))
        .unwrap_or(false)
}

fn should_use_github_archive(url: &str) -> bool {
    let trimmed = url.trim();
    if trimmed.starts_with("git@") || trimmed.starts_with("ssh://") {
        return false;
    }

    trimmed.starts_with("https://") && is_github_host(trimmed)
}

pub fn list_versions(url: &str, _cache: &GlobalCache) -> Result<Vec<AvailableVersion>, MarsError> {
    ls_remote_tags(url)
}

fn ls_remote_ref(url: &str, reference: &str) -> Result<String, MarsError> {
    git_cli::ls_remote_ref(url, reference)
}

pub(crate) fn ls_remote_head(url: &str) -> Result<String, MarsError> {
    git_cli::ls_remote_head(url)
}

pub fn ls_remote_tags(url: &str) -> Result<Vec<AvailableVersion>, MarsError> {
    git_cli::ls_remote_tags(url).map_err(|error| with_alias_hint(url, error))
}

/// `host:path` with no user and a dotless host (`bitbucket:owner/repo`) is git's SSH form, but
/// reads like an alias Mars doesn't have; when SSH fails, say so.
fn with_alias_hint(url: &str, error: MarsError) -> MarsError {
    let MarsError::GitCli { command, message } = error else {
        return error;
    };
    let dotless_host = !url.contains('@')
        && mars_schema::git_url::is_ssh_shorthand(url)
        && extract_hostname(url).is_some_and(|host| !host.contains('.'));
    let message = if dotless_host {
        format!(
            "{message}\nhint: `{url}` is read as SSH to host `{}`; for a GitHub or GitLab repository use `github:owner/repo`, `gitlab:owner/repo` or `owner/repo`",
            extract_hostname(url).unwrap_or_default()
        )
    } else {
        message
    };
    MarsError::GitCli { command, message }
}

pub fn fetch(
    url: &str,
    version_req: Option<&str>,
    source_name: &str,
    cache: &GlobalCache,
    options: &FetchOptions,
    diag: &mut DiagnosticCollector,
) -> Result<ResolvedRef, MarsError> {
    let remote_url = url.to_string();
    let mut resolved = resolve_version(&remote_url, version_req, diag)?;
    if let Some(preferred_commit) = options.preferred_commit.as_ref() {
        resolved.sha = preferred_commit.to_string();
    }

    let tree_path = if should_use_github_archive(&remote_url) {
        match archive::fetch_archive(&remote_url, &resolved.sha, cache) {
            Ok(path) => path,
            Err(MarsError::Http { status: 404, .. }) if options.preferred_commit.is_some() => {
                return Err(MarsError::LockedCommitUnreachable {
                    commit: resolved.sha.clone(),
                    url: remote_url,
                });
            }
            Err(err) => return Err(err),
        }
    } else {
        // For git clone path, prefer exact SHA checkout when replaying a locked commit,
        // or when resolving branch/default-HEAD refs (non-tag fetches).
        let checkout_sha = if options.preferred_commit.is_some() || resolved.tag.is_none() {
            Some(resolved.sha.as_str())
        } else {
            None
        };

        match git_cli::fetch_git_clone(&remote_url, resolved.tag.as_deref(), checkout_sha, cache) {
            Ok(path) => path,
            Err(MarsError::GitCli { .. }) if options.preferred_commit.is_some() => {
                return Err(MarsError::LockedCommitUnreachable {
                    commit: resolved.sha.clone(),
                    url: remote_url,
                });
            }
            Err(err) => return Err(err),
        }
    };

    Ok(ResolvedRef {
        source_name: source_name.into(),
        version: resolved.version,
        version_tag: resolved.tag,
        commit: Some(CommitHash::from(resolved.sha)),
        tree_path,
    })
}

pub fn fetch_commit(
    url: &str,
    commit: &str,
    source_name: &str,
    cache: &GlobalCache,
    _diag: &mut DiagnosticCollector,
) -> Result<ResolvedRef, MarsError> {
    let remote_url = url.to_string();
    let tree_path = if should_use_github_archive(&remote_url) {
        match archive::fetch_archive(&remote_url, commit, cache) {
            Ok(path) => path,
            Err(MarsError::Http { status: 404, .. }) => {
                return Err(MarsError::LockedCommitUnreachable {
                    commit: commit.to_string(),
                    url: remote_url,
                });
            }
            Err(err) => return Err(err),
        }
    } else {
        match git_cli::fetch_git_clone(&remote_url, None, Some(commit), cache) {
            Ok(path) => path,
            Err(MarsError::GitCli { .. }) => {
                return Err(MarsError::LockedCommitUnreachable {
                    commit: commit.to_string(),
                    url: remote_url,
                });
            }
            Err(err) => return Err(err),
        }
    };

    Ok(ResolvedRef {
        source_name: source_name.into(),
        version: None,
        version_tag: None,
        commit: Some(CommitHash::from(commit)),
        tree_path,
    })
}
