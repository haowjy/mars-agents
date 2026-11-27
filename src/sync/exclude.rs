//! The block Mars owns in a repository's `info/exclude`, which git reads for every work tree and
//! never commits: what `machine_local` decides to keep out of commits is listed there.
//!
//! Git reads only the common directory's `info/exclude`, so every linked work tree of a
//! repository shares the file, and git applies every pattern in it relative to each work tree's
//! root: one work tree's block hides the same paths in all the others. Each work tree has its own
//! block, keyed by its canonical path, and a sync rewrites only its own block, under a lock file
//! beside it, by tmp+rename. A block whose work tree is gone is dropped by the next sync of any
//! work tree. Lines outside the blocks are the user's and stay as they are. Because the blocks
//! are shared, [`settle`] reports a path this work tree commits that another work tree's block
//! hides; it cannot unhide it without unhiding the other work tree's item.
//!
//! A sync writes its block twice: before it writes any output, from its decision joined with the
//! paths the block already lists, so a sync killed midway leaves nothing unprotected; and after it
//! publishes the lock, from the lock, which removes what it no longer installs.
//!
//! This is a sync output, not how Mars finds a project: without git, or outside a work tree,
//! there is nothing to write. The work tree is found by walking up for `.git`, a directory or a
//! linked work tree's file.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::error::MarsError;
use crate::layout::Layout;

const BEGIN: &str = "# >>> mars: machine-local items of ";
const END: &str = "# <<< mars: machine-local items of ";

/// The git work tree holding `root`, and its repository's common directory.
pub(crate) struct WorkTree {
    pub root: PathBuf,
    common_dir: PathBuf,
}

impl WorkTree {
    fn exclude(&self) -> PathBuf {
        self.common_dir.join("info").join("exclude")
    }

    fn key(&self) -> String {
        self.root.to_string_lossy().into_owned()
    }
}

pub(crate) fn work_tree(root: &Path) -> Option<WorkTree> {
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

/// The lines outside every block, and each block's patterns by owner, in file order.
struct Blocks {
    kept: Vec<String>,
    owners: BTreeMap<String, Vec<String>>,
}

fn read_blocks(exclude: &Path) -> Result<(String, Blocks), MarsError> {
    let content = match std::fs::read_to_string(exclude) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    let mut blocks = Blocks {
        kept: Vec::new(),
        owners: BTreeMap::new(),
    };
    let mut lines = content.lines();
    while let Some(line) = lines.next() {
        let Some(owner) = line.strip_prefix(BEGIN) else {
            blocks.kept.push(line.to_string());
            continue;
        };
        let block: Vec<String> = lines
            .by_ref()
            .take_while(|line| line.strip_prefix(END) != Some(owner))
            .map(str::to_string)
            .collect();
        blocks
            .owners
            .entry(owner.to_string())
            .or_default()
            .extend(block);
    }
    Ok((content, blocks))
}

/// The paths this work tree's block lists, as absolute paths. Empty without a work tree or an
/// unreadable file: a path that is not listed is only one Mars does not take for its own.
pub(crate) fn listed(layout: &Layout) -> BTreeSet<PathBuf> {
    let Some(tree) = work_tree(layout.root()) else {
        return BTreeSet::new();
    };
    let Ok((_, blocks)) = read_blocks(&tree.exclude()) else {
        return BTreeSet::new();
    };
    blocks
        .owners
        .get(&tree.key())
        .into_iter()
        .flatten()
        .filter_map(|pattern| unpattern(pattern))
        .map(|relative| tree.root.join(relative))
        .collect()
}

/// Before a sync writes: add `planned` to this work tree's block, keeping what it lists.
pub(crate) fn protect(layout: &Layout, planned: BTreeSet<PathBuf>) -> Result<(), MarsError> {
    rewrite(layout, Update::Protect(planned)).map(|_| ())
}

/// What a published lock says about the block: see [`settle`].
pub(crate) struct Settled {
    /// Machine-local outputs: listed.
    pub machine: BTreeSet<PathBuf>,
    /// Shared files only machine-local entries are in: listed while the block lists them, since
    /// whether Mars created the file was decided before it wrote (`machine_local::decide`).
    pub files: BTreeSet<PathBuf>,
    /// What this work tree should commit, checked against the other work trees' blocks.
    pub project: BTreeSet<PathBuf>,
}

/// After a sync published its lock: this work tree's block lists exactly what `settled` says.
/// Returns one warning per other work tree whose block hides a path of `settled.project` that it
/// did not hide at the last settle: the block notes what it reported, so each is said once.
pub(crate) fn settle(layout: &Layout, settled: Settled) -> Result<Vec<String>, MarsError> {
    rewrite(layout, Update::Settle(settled))
}

enum Update {
    Protect(BTreeSet<PathBuf>),
    Settle(Settled),
}

/// The line in a work tree's block that says a path of its own is hidden by `owner`'s block.
const HIDDEN: &str = "# hidden by ";

fn rewrite(layout: &Layout, update: Update) -> Result<Vec<String>, MarsError> {
    let Some(tree) = work_tree(layout.root()) else {
        return Ok(Vec::new());
    };
    let exclude = tree.exclude();
    let key = tree.key();
    // With no file there are no blocks, so nothing is listed or hidden: an empty block needs none.
    let empty = match &update {
        Update::Protect(planned) => planned.is_empty(),
        Update::Settle(settled) => settled.machine.is_empty(),
    };
    if empty && !exclude.is_file() {
        return Ok(Vec::new());
    }
    std::fs::create_dir_all(exclude.parent().expect("info/exclude has a parent"))?;
    let _lock = crate::fs::FileLock::acquire(&tree.common_dir.join("mars-exclude.lock"))?;
    let (content, blocks) = read_blocks(&exclude)?;
    let Blocks { mut kept, owners } = blocks;

    let mut own: Vec<String> = Vec::new();
    let mut others: Vec<(String, Vec<String>)> = Vec::new();
    for (owner, lines) in owners {
        if owner == key {
            own = lines;
        } else if Path::new(&owner).join(".git").exists() {
            kept.push(format!("{BEGIN}{owner}"));
            kept.extend(lines.iter().cloned());
            kept.push(format!("{END}{owner}"));
            others.push((owner, lines.iter().filter_map(|p| unpattern(p)).collect()));
        }
    }
    let listed: BTreeSet<PathBuf> = own
        .iter()
        .filter_map(|line| unpattern(line))
        .map(|relative| tree.root.join(relative))
        .collect();
    let noted: BTreeSet<&str> = own
        .iter()
        .filter(|line| line.starts_with(HIDDEN))
        .map(String::as_str)
        .collect();
    let relative = |paths: &BTreeSet<PathBuf>| -> Vec<String> {
        paths
            .iter()
            .filter_map(|path| path.strip_prefix(&tree.root).ok())
            .map(slash)
            .collect()
    };

    let mut lines: BTreeSet<String> = BTreeSet::new();
    let mut warnings = Vec::new();
    match update {
        Update::Protect(planned) => {
            lines.extend(own.iter().cloned());
            lines.extend(
                relative(&planned)
                    .iter()
                    .map(|path| pattern(Path::new(path))),
            );
        }
        Update::Settle(Settled {
            mut machine,
            files,
            project,
        }) => {
            machine.extend(files.into_iter().filter(|file| listed.contains(file)));
            lines.extend(
                relative(&machine)
                    .iter()
                    .map(|path| pattern(Path::new(path))),
            );
            let project = relative(&project);
            for (owner, patterns) in &others {
                let hidden: Vec<&String> = project
                    .iter()
                    .filter(|path| {
                        patterns.iter().any(|pattern| {
                            *path == pattern
                                || path
                                    .strip_prefix(pattern.as_str())
                                    .is_some_and(|rest| rest.starts_with('/'))
                        })
                    })
                    .collect();
                let mut news = Vec::new();
                for path in hidden {
                    let note = format!("{HIDDEN}{owner}: {path}");
                    if !noted.contains(note.as_str()) {
                        news.push(format!("`{path}`"));
                    }
                    lines.insert(note);
                }
                if !news.is_empty() {
                    let one = news.len() == 1;
                    warnings.push(format!(
                        "{} of this project {} hidden from `git add` here: work tree `{owner}` lists {} in the repository's shared info/exclude as its machine-local items, and git applies that file in every work tree. Add {} with `git add -f`",
                        news.join(", "),
                        if one { "is" } else { "are" },
                        if one { "it" } else { "them" },
                        if one { "it" } else { "them" },
                    ));
                }
            }
        }
    }
    if !lines.is_empty() {
        kept.push(format!("{BEGIN}{key}"));
        kept.extend(lines);
        kept.push(format!("{END}{key}"));
    }
    let mut out = kept.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    if out != content {
        crate::fs::atomic_write(&exclude, out.as_bytes())?;
    }
    Ok(warnings)
}

fn slash(relative: &Path) -> String {
    relative.to_string_lossy().replace('\\', "/")
}

/// `relative` as an anchored gitignore pattern matching that one path.
fn pattern(relative: &Path) -> String {
    let mut out = String::from("/");
    for ch in slash(relative).chars() {
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

/// The path a [`pattern`] matches, relative and with `/`; `None` for a line Mars did not write.
fn unpattern(pattern: &str) -> Option<String> {
    let mut chars = pattern.strip_prefix('/')?.chars();
    let mut out = String::new();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            out.push(chars.next()?);
        } else {
            out.push(ch);
        }
    }
    Some(out)
}
