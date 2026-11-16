//! Git source adapter: the repository's tags and refs, and a commit's content directory.
//!
//! Delegates to `git_cli` for git CLI operations and `archive` for GitHub archive
//! download/extraction.

use mars_schema::resolve::Tag;

use crate::error::MarsError;
use crate::source::GlobalCache;
use crate::source::parse::extract_hostname;
use std::path::PathBuf;

use super::archive;
use super::git_cli;

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

pub fn ls_remote_ref(url: &str, reference: &str) -> Result<String, MarsError> {
    git_cli::ls_remote_ref(url, reference)
}

pub(crate) fn ls_remote_head(url: &str) -> Result<String, MarsError> {
    git_cli::ls_remote_head(url)
}

pub fn ls_remote_tags(url: &str) -> Result<Vec<Tag>, MarsError> {
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

/// The content directory of `commit` (see `git_cli::fetch_git_content`). `tag` is the tag the
/// commit was listed under, which git fetches by name; any other commit is fetched by hash. A
/// commit the repository does not have is `LockedCommitUnreachable`; every other error says
/// nothing about the commit.
pub fn fetch_content(
    url: &str,
    commit: &str,
    tag: Option<&str>,
    cache: &GlobalCache,
) -> Result<PathBuf, MarsError> {
    if should_use_github_archive(url) {
        return match archive::fetch_archive(url, commit, cache) {
            Err(MarsError::Http { status: 404, .. }) => Err(MarsError::LockedCommitUnreachable {
                commit: commit.to_string(),
                url: url.to_string(),
            }),
            other => other,
        };
    }
    git_cli::fetch_git_content(url, tag, tag.is_none(), commit, cache)
}
