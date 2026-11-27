//! Keeps machine-local items out of commits: in a git work tree, the paths a sync creates for
//! the records `.mars/machine.lock` holds (the user level's, a host's, a target only
//! `mars.local.toml` names) are listed in a block Mars owns in the repository's `info/exclude`,
//! which git reads for every work tree and never commits.
//!
//! Git reads only the common directory's `info/exclude`, so every linked work tree of a
//! repository shares the file. Each work tree has its own block, keyed by its canonical path, and
//! a sync rewrites only its own block, under a lock file beside it, by tmp+rename. A block whose
//! work tree is gone is dropped by the next sync of any work tree. Lines outside the blocks are
//! the user's and stay as they are.
//!
//! A sync writes its block twice: before it writes any output, from its plan joined with the
//! paths the block already lists, so a sync killed midway leaves nothing unprotected; and after
//! it publishes the lock, from the lock, which removes what it no longer installs.
//!
//! This is a sync output, not how Mars finds a project: without git, or outside a work tree,
//! there is nothing to write. The work tree is found by walking up for `.git`, a directory or a
//! linked work tree's file.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::error::MarsError;
use crate::layout::Layout;
use crate::lock::LockFile;

const BEGIN: &str = "# >>> mars: machine-local items of ";
const END: &str = "# <<< mars: machine-local items of ";

/// The git work tree holding `root`, and its repository's common directory.
struct WorkTree {
    root: PathBuf,
    common_dir: PathBuf,
}

fn work_tree(root: &Path) -> Option<WorkTree> {
    let root = dunce::canonicalize(root).ok()?;
    let top = root.ancestors().find(|dir| dir.join(".git").exists())?;
    let dot_git = top.join(".git");
    let git_dir = if dot_git.is_dir() {
        dot_git
    } else {
        let content = std::fs::read_to_string(&dot_git).ok()?;
        let target = content.trim().strip_prefix("gitdir:")?.trim();
        top.join(target)
    };
    let common_dir = match std::fs::read_to_string(git_dir.join("commondir")) {
        Ok(common) => git_dir.join(common.trim()),
        Err(_) => git_dir,
    };
    Some(WorkTree {
        root: top.to_path_buf(),
        common_dir: dunce::canonicalize(&common_dir).unwrap_or(common_dir),
    })
}

/// Every path the machine-local half of `lock` records an output at.
pub(crate) fn locked_paths(
    layout: &Layout,
    lock: &LockFile,
    project_targets: &BTreeSet<String>,
) -> BTreeSet<PathBuf> {
    let (_, local) = crate::lock::machine::split(lock.clone(), project_targets);
    local
        .items
        .values()
        .flat_map(|item| &item.outputs)
        .map(|output| {
            output
                .dest_path
                .resolve(&layout.target(&output.target_root))
        })
        .collect()
}

/// The paths a sync will write for its plan's machine-local items, in the store and in each
/// managed target that takes them, an agent's native file included.
pub(crate) fn planned_paths(
    layout: &Layout,
    target: &crate::sync::target::TargetState,
    managed_targets: &[String],
) -> BTreeSet<PathBuf> {
    let mut paths = BTreeSet::new();
    for item in target.items.values() {
        let source = item.source_name.as_str();
        if !crate::layer::is_machine_local(source) {
            continue;
        }
        paths.insert(item.dest_path.resolve(layout.store()));
        for target_root in managed_targets {
            if !crate::layer::offered_to(source, target_root) {
                continue;
            }
            let relative = match crate::sync::package_dir::PackageDir::parse_canonical(
                item.id.kind,
                &item.dest_path,
            ) {
                Some(dir) if dir.belongs_to(target_root) => dir.native(),
                Some(_) => continue,
                None => item.dest_path.to_string(),
            };
            paths.insert(layout.in_target(target_root, &relative));
            if item.id.kind == mars_schema::item::ItemKind::Agent
                && let Some(harness) = crate::harness::registry::harness_for_target(target_root)
            {
                let extension = crate::compiler::lowering::lowering(harness)
                    .agent
                    .extension();
                paths.insert(
                    layout.in_target(target_root, format!("agents/{}.{extension}", item.id.name)),
                );
            }
        }
    }
    paths
}

/// Before a sync writes: add `planned` to this work tree's block, keeping what it lists.
pub(crate) fn protect(layout: &Layout, planned: BTreeSet<PathBuf>) -> Result<(), MarsError> {
    rewrite(layout, planned, true)
}

/// After a sync published its lock: this work tree's block lists exactly `paths`.
pub(crate) fn settle(layout: &Layout, paths: BTreeSet<PathBuf>) -> Result<(), MarsError> {
    rewrite(layout, paths, false)
}

fn rewrite(layout: &Layout, paths: BTreeSet<PathBuf>, keep: bool) -> Result<(), MarsError> {
    let Some(tree) = work_tree(layout.root()) else {
        return Ok(());
    };
    let info = tree.common_dir.join("info");
    let exclude = info.join("exclude");
    let key = tree.root.to_string_lossy().into_owned();
    if paths.is_empty() && !keep && !exclude.is_file() {
        return Ok(());
    }
    std::fs::create_dir_all(&info)?;
    let _lock = crate::fs::FileLock::acquire(&tree.common_dir.join("mars-exclude.lock"))?;
    let content = match std::fs::read_to_string(&exclude) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };

    let mut kept = Vec::new();
    let mut own: BTreeSet<String> = BTreeSet::new();
    let mut lines = content.lines();
    while let Some(line) = lines.next() {
        let Some(owner) = line.strip_prefix(BEGIN) else {
            kept.push(line.to_string());
            continue;
        };
        let block: Vec<&str> = lines
            .by_ref()
            .take_while(|line| line.strip_prefix(END) != Some(owner))
            .collect();
        if owner == key {
            own.extend(block.iter().map(|line| line.to_string()));
        } else if Path::new(owner).join(".git").exists() {
            kept.push(line.to_string());
            kept.extend(block.iter().map(|line| line.to_string()));
            kept.push(format!("{END}{owner}"));
        }
    }

    let mut patterns: BTreeSet<String> = paths
        .iter()
        .filter_map(|path| path.strip_prefix(&tree.root).ok())
        .map(pattern)
        .collect();
    if keep {
        patterns.extend(own);
    }
    if !patterns.is_empty() {
        kept.push(format!("{BEGIN}{key}"));
        kept.extend(patterns);
        kept.push(format!("{END}{key}"));
    }
    let mut out = kept.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    if out != content {
        crate::fs::atomic_write(&exclude, out.as_bytes())?;
    }
    Ok(())
}

/// `relative` as an anchored gitignore pattern matching that one path.
fn pattern(relative: &Path) -> String {
    let mut out = String::from("/");
    let text = relative.to_string_lossy().replace('\\', "/");
    for ch in text.chars() {
        if matches!(ch, '*' | '?' | '[' | '\\' | '!' | '#') {
            out.push('\\');
        }
        out.push(ch);
    }
    if out.ends_with(' ') {
        out.insert(out.len() - 1, '\\');
    }
    out
}
