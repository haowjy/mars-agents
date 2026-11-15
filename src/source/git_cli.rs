//! Git CLI operations — ls-remote, clone, fetch, checkout.

use std::path::{Path, PathBuf};

use crate::error::MarsError;
use crate::platform::cache::{archive_cache_component, git_cache_component};
use crate::platform::fs::{publish_cache_dir_if_absent, safe_remove};
use crate::platform::process::{display_command, run_git, run_git_with_ref};
use crate::source::{AvailableVersion, GlobalCache};

/// Dependency data (a URL, a tag or branch, a locked commit) reaches git as an argument, and one
/// starting with `-` would be an option: `--upload-pack=<cmd>` runs `<cmd>`. Every function here
/// refuses such a value before spawning git, and puts `--` before the repository where git has one.
fn not_an_option(what: &str, value: &str) -> Result<(), MarsError> {
    if value.starts_with('-') {
        return Err(MarsError::Source {
            source_name: value.to_string(),
            message: format!("{what} `{value}` starts with `-`, which git would read as an option"),
        });
    }
    Ok(())
}

pub(crate) fn ls_remote_ref(url: &str, reference: &str) -> Result<String, MarsError> {
    not_an_option("URL", url)?;
    not_an_option("ref", reference)?;
    let command_display = display_command(&["ls-remote", "--", url, reference]);
    let output = run_git_with_ref(
        &["ls-remote", "--", url],
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

pub fn ls_remote_tags(url: &str) -> Result<Vec<AvailableVersion>, MarsError> {
    not_an_option("URL", url)?;
    let output = run_git(
        &["ls-remote", "--tags", "--", url],
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

        let Some(version) = mars_schema::version::parse_tag(tag) else {
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

pub fn ls_remote_head(url: &str) -> Result<String, MarsError> {
    ls_remote_ref(url, "HEAD")
}

/// Bring `commit` of `url` into the cache and return its content directory, `archives/<url>@<commit>`:
/// the commit's files and nothing else, shared by every dependency on that commit and never
/// rewritten once published.
///
/// The clone under `git/` is one per URL and only holds objects; no one reads its working tree,
/// because the next fetch of another commit would change it under a reader. `tag` and `pinned`
/// shape the git commands: a tag is cloned or fetched by name, and a pinned commit (a lock replay,
/// or a ref with no tag) is also fetched by hash. `commit` is the hash the lock records.
pub(crate) fn fetch_git_content(
    url: &str,
    tag: Option<&str>,
    pinned: bool,
    commit: &str,
    cache: &GlobalCache,
) -> Result<PathBuf, MarsError> {
    not_an_option("URL", url)?;
    tag.map(|tag| not_an_option("tag", tag)).transpose()?;
    not_an_option("commit", commit)?;
    let content_path = cache
        .archives_dir()
        .join(archive_cache_component(url, commit)?);
    if content_path.exists() {
        return Ok(content_path);
    }
    let sha = pinned.then_some(commit);
    let cache_name = git_cache_component(url)?;
    let cache_path = cache.git_dir().join(cache_name);
    std::fs::create_dir_all(cache.git_dir())?;

    // Serialize fetches into a cache entry. `with_extension` swaps the text after the last `.` in
    // the entry name, so entries on one host (`github.com_...`) can share one lock file.
    // Held through fetch + export, released when _lock drops at function return.
    let lock_path = cache_path.with_extension("lock");
    let _lock = crate::fs::FileLock::acquire(&lock_path)?;
    if content_path.exists() {
        return Ok(content_path);
    }

    let cache_path_display = cache_path.to_string_lossy().to_string();
    let was_cached = cache_path.exists();

    if !was_cached {
        // No checkout: the files come from `export_commit`.
        let mut args = vec!["clone", "--no-checkout"];
        if sha.is_none() {
            args.push("--depth");
            args.push("1");
        }
        if let Some(tag_name) = tag {
            // `--branch <tag>` implies `--single-branch`, which pins the refspec to that one tag:
            // once upstream deletes it, every later `fetch --prune-tags` fails.
            args.push("--no-single-branch");
            args.push("--branch");
            args.push(tag_name);
        }
        args.push("--");
        args.push(url);
        args.push(&cache_path_display);

        run_git(&args, &cache.git_dir(), "clone git source into cache")?;
    } else {
        run_git(
            &["fetch", "--depth", "1", "--tags", "--prune-tags", "origin"],
            &cache_path,
            "fetch cached git source",
        )?;
        if let Some(sha) = sha {
            match run_git(
                &["fetch", "--depth", "1", "--", "origin", sha],
                &cache_path,
                "fetch cached git commit",
            ) {
                Ok(_) => {}
                Err(_) => {
                    run_git(
                        &["fetch", "--unshallow", "origin"],
                        &cache_path,
                        "unshallow cached git source for locked commit",
                    )
                    .or_else(|_| {
                        run_git(
                            &["fetch", "origin"],
                            &cache_path,
                            "fetch cached git source for locked commit",
                        )
                    })?;
                }
            }
        }
    }

    export_commit(&cache_path, commit, &content_path)?;
    Ok(content_path)
}

/// Write the files of `commit` into `content_path`, whole or not at all. The lock held by the
/// caller makes the scratch name safe to reuse: a killed run leaves one directory that the next
/// run clears, not one per process id.
fn export_commit(clone: &Path, commit: &str, content_path: &Path) -> Result<(), MarsError> {
    let scratch = content_path.with_file_name(format!(
        "{}.tmp",
        content_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
    ));
    safe_remove(&scratch)?;
    std::fs::create_dir_all(&scratch)?;
    let exported =
        run_git(&["read-tree", commit], clone, "read cached git commit").and_then(|_| {
            // checkout-index applies the same filters as a checkout and, unlike archive, ignores
            // `export-ignore`; the prefix must end in a separator.
            let mut prefix = scratch.to_string_lossy().into_owned();
            prefix.push(std::path::MAIN_SEPARATOR);
            run_git(
                &["checkout-index", "--all", "--prefix", &prefix],
                clone,
                "export cached git commit",
            )
        });
    if let Err(error) = exported {
        let _ = safe_remove(&scratch);
        return Err(error);
    }
    publish_cache_dir_if_absent(&scratch, content_path)?;
    Ok(())
}
