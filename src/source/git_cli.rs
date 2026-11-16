//! Git CLI operations — ls-remote, clone, fetch, checkout.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::error::MarsError;
use crate::platform::cache::{archive_cache_component, git_cache_component};
use crate::platform::fs::{publish_cache_dir_if_absent, safe_remove};
use crate::platform::process::{display_command, git_has_commit, run_git, run_git_with_index};
use crate::source::GlobalCache;
use mars_schema::resolve::Tag;

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

/// The commit a ref names: a ref to an annotated tag is peeled, as the tag listing does, so a new
/// pin records the commit and not the tag object.
pub(crate) fn ls_remote_ref(url: &str, reference: &str) -> Result<String, MarsError> {
    not_an_option("URL", url)?;
    not_an_option("ref", reference)?;
    let peeled_pattern = format!("{reference}^{{}}");
    let args = ["ls-remote", "--", url, reference, peeled_pattern.as_str()];
    let output = run_git(&args, Path::new("."), "resolve remote git reference")?;

    // The first ref listed is the answer; git lists an annotated tag's commit on its own
    // `<ref>^{}` line.
    let lines: Vec<(&str, &str)> = output
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(sha, name)| (sha.trim(), name))
        .filter(|(sha, _)| !sha.is_empty())
        .collect();
    if let Some((sha, name)) = lines.iter().find(|(_, name)| !name.ends_with("^{}")) {
        let peeled = lines
            .iter()
            .find(|(_, other)| other.strip_suffix("^{}") == Some(name));
        return Ok(peeled.map_or(*sha, |(commit, _)| *commit).to_string());
    }

    Err(MarsError::GitCli {
        command: display_command(&["ls-remote", "--", url, reference]),
        message: format!("reference `{reference}` not found"),
    })
}

/// Every tag of the repository and the commit it points at: an annotated tag's own line names the
/// tag object, and its peeled `^{}` line the commit, which is the one that is recorded.
pub fn ls_remote_tags(url: &str) -> Result<Vec<Tag>, MarsError> {
    not_an_option("URL", url)?;
    let output = run_git(
        &["ls-remote", "--tags", "--", url],
        Path::new("."),
        "list remote git tags",
    )?;
    let mut tags: BTreeMap<String, (String, Option<String>)> = BTreeMap::new();
    for line in output.lines() {
        let Some((sha, reference)) = line.split_once('\t') else {
            continue;
        };
        let Some(tag) = reference.strip_prefix("refs/tags/") else {
            continue;
        };
        let sha = sha.trim().to_string();
        match tag.strip_suffix("^{}") {
            Some(tag) => tags.entry(tag.to_string()).or_default().1 = Some(sha),
            None => tags.entry(tag.to_string()).or_default().0 = sha,
        }
    }
    Ok(tags
        .into_iter()
        .map(|(tag, (object, peeled))| Tag {
            tag,
            commit: peeled.unwrap_or(object),
        })
        .collect())
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
        // `--force`: the cache is a mirror of what the remote says now, so a tag the remote moved
        // is taken, not rejected. A lock that wants the old commit is kept by the resolver.
        run_git(
            &[
                "fetch",
                "--depth",
                "1",
                "--force",
                "--tags",
                "--prune-tags",
                "origin",
            ],
            &cache_path,
            "fetch cached git source",
        )?;
    }

    // Cold and warm caches reach a pinned commit the same way: a commit no advertised ref reaches
    // is still asked of the remote by hash before it is called missing.
    if let Some(sha) = sha
        && !git_has_commit(&cache_path, sha)?
    {
        fetch_commit(&cache_path, sha)?;
    }

    // A pinned commit that survived the fetches above and is still not in the clone is gone from
    // the repository. Whatever else fails exporting a commit that is there (a smudge filter, a
    // full disk) says nothing about the commit and keeps its own error.
    if pinned && !git_has_commit(&cache_path, commit)? {
        return Err(MarsError::CommitUnreachable {
            commit: commit.to_string(),
            url: url.to_string(),
        });
    }
    export_commit(&cache_path, commit, &content_path)?;
    Ok(content_path)
}

/// Fetch `sha` by hash; a server that won't serve a bare hash gets a full fetch instead. An error
/// from every attempt is returned as it is: it says the remote could not be asked, not that the
/// commit is gone.
fn fetch_commit(cache_path: &Path, sha: &str) -> Result<(), MarsError> {
    if run_git(
        &["fetch", "--depth", "1", "--", "origin", sha],
        cache_path,
        "fetch cached git commit",
    )
    .is_ok()
    {
        return Ok(());
    }
    run_git(
        &["fetch", "--unshallow", "origin"],
        cache_path,
        "unshallow cached git source for locked commit",
    )
    .or_else(|_| {
        run_git(
            &["fetch", "origin"],
            cache_path,
            "fetch cached git source for locked commit",
        )
    })
    .map(|_| ())
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
    // A directory, so that the `index.lock` git leaves when it is killed is cleared with it.
    let index_dir = scratch.with_extension("index");
    safe_remove(&scratch)?;
    safe_remove(&index_dir)?;
    std::fs::create_dir_all(&scratch)?;
    std::fs::create_dir_all(&index_dir)?;
    // A private index and the scratch directory as the work tree: git then takes `.gitattributes`
    // and filters from `commit` alone. `checkout-index` in the clone would also read the clone's
    // working tree, which a clone made by an older mars still has, at some other commit.
    let scratch_arg = scratch.to_string_lossy().into_owned();
    let exported = run_git_with_index(
        &[
            "--work-tree",
            &scratch_arg,
            "read-tree",
            "-u",
            "--reset",
            commit,
        ],
        clone,
        "export cached git commit",
        Some(&index_dir.join("index")),
    );
    let _ = safe_remove(&index_dir);
    if let Err(error) = exported {
        let _ = safe_remove(&scratch);
        return Err(error);
    }
    publish_cache_dir_if_absent(&scratch, content_path)?;
    Ok(())
}
