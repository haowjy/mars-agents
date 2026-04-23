//! Git CLI operations — ls-remote, clone, fetch, checkout.

use std::path::{Path, PathBuf};

use crate::error::MarsError;
use crate::platform::cache::git_cache_component;
use crate::platform::process::{display_command, run_git, run_git_with_ref};
use crate::source::{AvailableVersion, GlobalCache};

use super::git::parse_semver_tag;

pub(crate) fn ls_remote_ref(url: &str, reference: &str) -> Result<String, MarsError> {
    let command_display = display_command(&["ls-remote", url, reference]);
    let output = run_git_with_ref(
        &["ls-remote", url],
        reference,
        Path::new("."),
        "resolve remote git reference",
    )?;

    for line in output.lines() {
        if let Some((sha, _)) = line.split_once('\t')
            && !sha.trim().is_empty()
        {
            return Ok(sha.trim().to_string());
        }
    }

    Err(MarsError::GitCli {
        command: command_display,
        message: format!("reference `{reference}` not found"),
    })
}

/// Run `git ls-remote --tags <url>` and parse semver tags.
pub fn ls_remote_tags(url: &str) -> Result<Vec<AvailableVersion>, MarsError> {
    let output = run_git(
        &["ls-remote", "--tags", url],
        Path::new("."),
        "list remote git tags",
    )?;
    let mut versions = Vec::new();

    for line in output.lines() {
        let Some((sha, reference)) = line.split_once('\t') else {
            continue;
        };
        let Some(tag) = reference.strip_prefix("refs/tags/") else {
            continue;
        };

        // Annotated tags show up twice (`tag` and peeled `tag^{}`).
        // Keep only the non-peeled entry to avoid duplicates.
        if tag.ends_with("^{}") {
            continue;
        }

        let Some(version) = parse_semver_tag(tag) else {
            continue;
        };

        versions.push(AvailableVersion {
            tag: tag.to_string(),
            version,
            commit_id: sha.trim().to_string(),
        });
    }

    versions.sort_by(|a, b| a.version.cmp(&b.version));
    Ok(versions)
}

/// Run `git ls-remote <url> HEAD` and return the default-branch SHA.
pub fn ls_remote_head(url: &str) -> Result<String, MarsError> {
    ls_remote_ref(url, "HEAD")
}

pub(crate) fn fetch_git_clone(
    url: &str,
    tag: Option<&str>,
    sha: Option<&str>,
    cache: &GlobalCache,
) -> Result<PathBuf, MarsError> {
    let cache_name = git_cache_component(url)?;
    let cache_path = cache.git_dir().join(cache_name);

    // Acquire per-entry lock to prevent cross-repo races on the same cache entry.
    // Held through fetch + checkout, released when _lock drops at function return.
    let lock_path = cache_path.with_extension("lock");
    let _lock = crate::fs::FileLock::acquire(&lock_path)?;

    let cache_path_display = cache_path.to_string_lossy().to_string();
    let was_cached = cache_path.exists();

    if !was_cached {
        let mut args = vec!["clone", "--depth", "1"];
        if let Some(tag_name) = tag {
            args.push("--branch");
            args.push(tag_name);
        }
        args.push(url);
        args.push(&cache_path_display);

        run_git(&args, &cache.git_dir(), "clone git source into cache")?;
    } else {
        run_git(
            &["fetch", "--depth", "1", "origin"],
            &cache_path,
            "fetch cached git source",
        )?;
    }

    if was_cached {
        if let Some(tag_name) = tag {
            run_git(
                &["checkout", tag_name],
                &cache_path,
                "checkout cached git tag",
            )?;
        }

        if let Some(sha) = sha {
            run_git(
                &["checkout", sha],
                &cache_path,
                "checkout cached git commit",
            )?;
        } else if tag.is_none() {
            run_git(
                &["checkout", "origin/HEAD"],
                &cache_path,
                "checkout cached git default head",
            )?;
        }
    } else if let Some(sha) = sha {
        run_git(
            &["checkout", sha],
            &cache_path,
            "checkout cloned git commit",
        )?;
    }

    Ok(cache_path)
}
