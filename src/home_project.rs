//! The home project: a `mars.toml` or `mars.lock` in the home directory, which is how people
//! installed for themselves before the user level. Its store is `~/.mars`, which is also the user
//! level's folder unless `MARS_HOME` moves it. While both share the folder, the files in it are
//! the home project's: every user-level write is refused (`cli::dispatch_result`) and a project
//! sync installs no user level (`sync::layers`), each naming the steps left to move it.
//!
//! Kept for one minor release, with `--root ~` opening it (`src/cli/.context/TODO`).

use std::path::{Path, PathBuf};

use crate::layout::Layout;
use crate::types::managed_cmd;

pub(crate) struct HomeProject {
    layout: Layout,
    user: Layout,
}

impl HomeProject {
    /// The home project, when home holds its manifest or its lock.
    pub(crate) fn find() -> Option<Self> {
        let layout = Layout::project(crate::platform::home::canonical_home()?);
        if !layout.manifest().exists() && !layout.lock().exists() {
            return None;
        }
        Some(Self {
            layout,
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

    /// The home project's own files, for messages: `~/mars.toml` and `~/mars.lock`, those that
    /// exist.
    pub(crate) fn files(&self) -> String {
        [self.layout.manifest(), self.layout.lock()]
            .iter()
            .filter(|path| path.exists())
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(" and ")
    }

    /// Why the user level cannot be written while this home project shares its folder.
    pub(crate) fn blocks_user_level(&self) -> String {
        format!(
            "{} is still the store of the home project ({}); to move it to the user level, {}",
            self.user.root().display(),
            self.files(),
            self.steps()
        )
    }

    /// The steps left to move the home project to the user level, in the order a shared folder
    /// needs: unlink its targets while the manifest still opens it, delete its lock and store,
    /// move the manifest and `.mars-src/` in, then sync the user level.
    pub(crate) fn steps(&self) -> String {
        let home = self.layout.root();
        let mut steps = Vec::new();
        if self.layout.manifest().exists() {
            let targets = crate::config::load(&self.layout).ok().map(|config| {
                let local = crate::config::load_local(&self.layout).unwrap_or_default();
                crate::config::targets::named_targets(&config, &local)
            });
            let unlink = |target: &str| {
                format!(
                    "`{}`",
                    managed_cmd(&format!("mars --root {} unlink {target}", home.display()))
                )
            };
            match targets {
                None => steps.push(format!(
                    "run {} for each target it names",
                    unlink("<target>")
                )),
                Some(targets) if targets.is_empty() => {}
                Some(targets) => {
                    let unlinks: Vec<_> = targets.iter().map(|target| unlink(target)).collect();
                    steps.push(format!("run {}", unlinks.join(" and ")));
                }
            }
        }
        let mut deletes = Vec::new();
        if self.layout.lock().exists() {
            deletes.push(self.layout.lock().display().to_string());
        }
        if self.shares_user_folder() {
            if self.old_store_left() {
                deletes.push(format!(
                    "the old store's files in {}/ (keep any config.toml, curated.toml, mars.toml and .mars-src/ you moved in)",
                    self.user.root().display(),
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
        steps.join(", then ")
    }

    /// Whether the shared folder holds anything but the user level's own files while the user
    /// level has never synced: that is the home project's generated store.
    fn old_store_left(&self) -> bool {
        if self.user.lock().exists() {
            return false;
        }
        let own = self.user_root_files();
        std::fs::read_dir(self.user.root()).is_ok_and(|entries| {
            entries.flatten().any(|entry| {
                !own.iter()
                    .any(|name| entry.file_name().to_str() == Some(name.as_str()))
            })
        })
    }

    /// The names of the files that are the user level's, not its store's: what a user writes or
    /// moves in.
    fn user_root_files(&self) -> Vec<String> {
        let user = &self.user;
        let file_name = |path: &Path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        };
        [
            user.manifest(),
            user.local_config(),
            user.local_source(),
            user.curated(),
            user.curated_local(),
            PathBuf::from("config.toml"),
            PathBuf::from("curated.toml"),
        ]
        .iter()
        .filter_map(|path| file_name(path))
        .collect()
    }
}
