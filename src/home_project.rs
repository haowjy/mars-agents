//! The home project: a `mars.toml` or `mars.lock` in the home directory, which is how people
//! installed for themselves before the user level. Its store is `~/.mars`, which is also the user
//! level's folder unless `MARS_HOME` moves it. While both share the folder, the files in it are
//! the home project's: every user-level write is refused (`cli::dispatch_result`) and a project
//! sync installs no user level (`sync::layers`). `--root ~` opens it, for one minor release, only
//! to read it and to `unlink` its targets (`src/cli/.context/TODO`): another write could adopt
//! the user level's files and later delete them as its own orphans.
//!
//! Either file makes it the home project, everywhere: `unlink` works from the lock's records once
//! the manifest is gone, so the steps printed always run.

use std::path::Path;

use crate::diagnostic::Diagnostic;
use crate::layout::Layout;
use crate::types::managed_cmd;

pub(crate) struct HomeProject {
    layout: Layout,
    user: Layout,
}

/// The names a project store writes and a user level's never does: at the user level they are
/// what the home project's store left.
const PROJECT_STORE_ONLY: &[&str] = &["machine.lock", "provided.lock", "provided", "cache"];

/// The store folders that hold installed items, which the user lock records one by one.
const ITEM_FOLDERS: &[&str] = &["agents", "skills", "hooks", "mcp"];

/// How many leftover names a step lists before it counts the rest.
const LISTED_LEFTOVERS: usize = 5;

/// Whether `dir` holds a home project's files, when `dir` is the home directory.
pub(crate) fn holds_project(dir: &Path) -> bool {
    let layout = Layout::project(dir);
    layout.manifest().exists() || layout.lock().exists()
}

/// Raise the home-project notice for this run: shown when the run is done, so the steps it lists
/// are those still left after the command (an `unlink` drops its own).
pub(crate) fn raise_notice() {
    crate::diagnostic::raise_deferred_notice("home-project", || {
        HomeProject::find().map(|home| Diagnostic::warning("home-project", home.notice()))
    });
}

impl HomeProject {
    /// The home project, when home holds its manifest or its lock.
    pub(crate) fn find() -> Option<Self> {
        let home = crate::platform::home::canonical_home()?;
        if !holds_project(&home) {
            return None;
        }
        Some(Self {
            layout: Layout::project(home),
            user: Layout::user_level()?,
        })
    }

    /// The home project, when its store is the user level's folder.
    pub(crate) fn in_user_folder() -> Option<Self> {
        Self::find().filter(Self::shares_user_folder)
    }

    fn shares_user_folder(&self) -> bool {
        let store = self.layout.store();
        dunce::canonicalize(store).unwrap_or_else(|_| store.to_path_buf()) == self.user.root()
    }

    /// `~/mars.toml` and `~/mars.lock`, those that exist.
    fn files(&self) -> String {
        [self.layout.manifest(), self.layout.lock()]
            .iter()
            .filter(|path| path.exists())
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(" and ")
    }

    /// The warning a run that met the home project ends with: what it blocks and the steps
    /// left. Refusals name it instead of repeating the steps.
    fn notice(&self) -> String {
        let what = if self.shares_user_folder() {
            format!(
                "its store, {}, is now the user level's folder, so `--user` writes are refused and projects install no user level until you move it",
                self.user.root().display()
            )
        } else {
            "Mars no longer uses it".to_string()
        };
        format!(
            "your home directory holds a Mars 0.40 project ({}); {what}. Steps left:{}",
            self.files(),
            self.steps()
        )
    }

    /// Why the user level cannot be written while this home project shares its folder.
    pub(crate) fn blocks_user_level(&self) -> String {
        raise_notice();
        format!(
            "the user level cannot be written: {} is still the store of the project in your home directory; the home-project warning lists the steps to move it",
            self.user.root().display(),
        )
    }

    /// Why `--root ~` refuses a command that writes: the home project is opened only to unlink.
    pub(crate) fn refuses_writes(&self) -> String {
        raise_notice();
        format!(
            "your home directory is no longer a Mars project: `--root {}` opens it only to read it and to `unlink` its targets; the home-project warning lists the steps to move it to the user level",
            self.layout.root().display(),
        )
    }

    /// The steps left to move the home project to the user level, one per line, in the order a
    /// shared folder needs: unlink its targets while its lock still records them, delete its lock
    /// and store, move the manifest and `.mars-src/` in, then sync the user level.
    fn steps(&self) -> String {
        let home = self.layout.root();
        let mut steps = Vec::new();
        let unlink = |target: &str| {
            format!(
                "`{}`",
                managed_cmd(&format!("mars --root {} unlink {target}", home.display()))
            )
        };
        if self.layout.lock().exists() {
            match crate::lock::load(&self.layout) {
                Err(_) => steps.push(format!("run {} for each target", unlink("<target>"))),
                Ok(lock) => {
                    let targets = lock.recorded_targets();
                    if !targets.is_empty() {
                        let unlinks: Vec<_> = targets.iter().map(|target| unlink(target)).collect();
                        steps.push(format!("run {}", unlinks.join(", ")));
                    }
                }
            }
        }
        let mut deletes = Vec::new();
        if self.layout.lock().exists() {
            deletes.push(self.layout.lock().display().to_string());
        }
        if self.shares_user_folder() {
            let leftovers = store_leftovers(&self.user);
            if !leftovers.is_empty() {
                deletes.push(format!(
                    "the old store's {} in {}/",
                    name_leftovers(&leftovers),
                    self.user.root().display()
                ));
            }
        } else if self.layout.store().exists() {
            deletes.push(format!("{}/", self.layout.store().display()));
        }
        if !deletes.is_empty() {
            steps.push(format!("delete {}", deletes.join(" and ")));
        }
        if self.layout.manifest().exists() {
            steps.push(if self.user.manifest().exists() {
                format!(
                    "merge {} into {} without its `targets` line, and delete it",
                    self.layout.manifest().display(),
                    self.user.manifest().display()
                )
            } else {
                format!(
                    "move {} to {} and delete its `targets` line",
                    self.layout.manifest().display(),
                    self.user.manifest().display()
                )
            });
        }
        if self.layout.local_source().is_dir() {
            steps.push(format!(
                "move what is in {}/ to {}/",
                self.layout.local_source().display(),
                self.user.local_source().display()
            ));
        }
        steps.push(format!("run `{}`", managed_cmd("mars --user sync")));
        steps
            .iter()
            .enumerate()
            .map(|(index, step)| format!("\n    {}. {step}", index + 1))
            .collect()
    }
}

/// What a project store left in the user level's folder: the files only a project store writes,
/// and installed items the user lock does not record. Names are relative to the folder. Empty
/// when the user lock cannot be read, since then every item would look left over.
pub(crate) fn store_leftovers(user: &Layout) -> Vec<String> {
    let Ok(lock) = crate::lock::load(user) else {
        return Vec::new();
    };
    let recorded: Vec<String> = lock
        .canonical_flat_items()
        .into_iter()
        .map(|(dest, _)| dest.to_string())
        .collect();
    let covered = |name: &str| {
        recorded
            .iter()
            .any(|path| path == name || path.starts_with(&format!("{name}/")))
    };
    let root = user.store();
    let project_only = PROJECT_STORE_ONLY
        .iter()
        .filter(|name| root.join(name).symlink_metadata().is_ok())
        .map(|name| name.to_string());
    let items = ITEM_FOLDERS.iter().flat_map(|folder| {
        let mut names: Vec<String> = std::fs::read_dir(root.join(folder))
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| format!("{folder}/{}", entry.file_name().to_string_lossy()))
            .filter(|name| !covered(name))
            .collect();
        names.sort();
        names
    });
    project_only.chain(items).collect()
}

/// `leftovers` for a message: the first few by name, the rest counted.
pub(crate) fn name_leftovers(leftovers: &[String]) -> String {
    let listed = leftovers
        .iter()
        .take(LISTED_LEFTOVERS)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    match leftovers.len().saturating_sub(LISTED_LEFTOVERS) {
        0 => listed,
        rest => format!("{listed} and {rest} more"),
    }
}

/// Whether `layout` is the home project, which only `--root ~` opens.
pub(crate) fn is_home_project(layout: &Layout) -> bool {
    layout.kind() == crate::layout::RootKind::Project
        && Some(layout.root()) == crate::platform::home::canonical_home().as_deref()
}
