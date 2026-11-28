//! The user's Mars folder: `MARS_HOME`, else `<home>/.mars`, on every platform, as each harness
//! keeps one folder in home. The user level's manifest, lock, own items, store and the user
//! files (`config.toml`, `curated.toml`) all live in it.
//!
//! A relative `HOME` is ignored: it would name a different folder from every working directory.
//! A relative `MARS_HOME` is taken from the working directory, since the user typed it.

use std::path::PathBuf;

/// The variable that names the user folder.
pub const HOME_VARIABLE: &str = "MARS_HOME";

/// Mars 0.40 located `config.toml` with this variable. For one minor release it is read as
/// `MARS_HOME` when that is unset (`src/config/.context/CONTEXT.md`).
pub const RENAMED_VARIABLE: &str = "MARS_CONFIG_DIR";

/// The user folder, absolute, or `None` with no absolute home and no variable set.
pub fn mars_home() -> Option<PathBuf> {
    if let Some(dir) = env_dir(HOME_VARIABLE).or_else(|| env_dir(RENAMED_VARIABLE)) {
        return Some(std::path::absolute(&dir).unwrap_or(dir));
    }
    canonical_home().map(|home| home.join(".mars"))
}

/// What the user should hear about `MARS_CONFIG_DIR`: `Some(true)` when it names the user folder
/// (`MARS_HOME` is unset), `Some(false)` when `MARS_HOME` overrides it, `None` when it is unset.
pub fn renamed_variable_in_use() -> Option<bool> {
    env_dir(RENAMED_VARIABLE).map(|_| env_dir(HOME_VARIABLE).is_none())
}

/// A user file (`config.toml`, `curated.toml`) in the user folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserFile {
    /// The file to read.
    pub path: PathBuf,
    /// A copy where Mars 0.40 read it, when there is one.
    pub old: Option<OldCopy>,
}

/// The copy of a user file at the Mars 0.40 location (`dirs::config_dir()/mars`). It is read for
/// one minor release while the user folder has none (`src/config/.context/CONTEXT.md`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OldCopy {
    /// `UserFile::path` is the old copy; it belongs at `to`.
    Read { to: PathBuf },
    /// The user folder's copy is read; this one, at the old location, is not.
    Ignored { old: PathBuf },
}

/// The user file `name`, or `None` when the user folder cannot be located. Only the default
/// folder has an old copy: a variable names the whole folder, and `MARS_CONFIG_DIR` is still read
/// as the folder where 0.40 found the file.
pub fn user_file(name: &str) -> Option<UserFile> {
    let path = mars_home()?.join(name);
    let old = (env_dir(HOME_VARIABLE).is_none() && env_dir(RENAMED_VARIABLE).is_none())
        .then(|| dirs::config_dir().map(|dir| dir.join("mars").join(name)))
        .flatten()
        .filter(|old| old.is_file());
    Some(match old {
        None => UserFile { path, old: None },
        Some(old) if path.exists() => UserFile {
            path,
            old: Some(OldCopy::Ignored { old }),
        },
        Some(old) => UserFile {
            path: old,
            old: Some(OldCopy::Read { to: path }),
        },
    })
}

/// The home directory, canonical so a symlinked `/home` still compares equal.
pub(crate) fn canonical_home() -> Option<PathBuf> {
    home_dir().map(|home| dunce::canonicalize(&home).unwrap_or(home))
}

/// The user's home directory, when it is known as an absolute path.
fn home_dir() -> Option<PathBuf> {
    dirs::home_dir().filter(|home| home.is_absolute())
}

/// An empty value counts as unset.
fn env_dir(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::ffi::OsString;
    use std::path::Path;

    /// Sets the variables for the test and puts the old values back.
    struct Env(Vec<(&'static str, Option<OsString>)>);

    impl Env {
        fn set(vars: &[(&'static str, Option<&str>)]) -> Self {
            let saved = vars
                .iter()
                .map(|(key, value)| {
                    let old = std::env::var_os(key);
                    #[allow(unused_unsafe)]
                    unsafe {
                        match value {
                            Some(value) => std::env::set_var(key, value),
                            None => std::env::remove_var(key),
                        }
                    }
                    (*key, old)
                })
                .collect();
            Self(saved)
        }
    }

    impl Drop for Env {
        fn drop(&mut self) {
            for (key, old) in &self.0 {
                #[allow(unused_unsafe)]
                unsafe {
                    match old {
                        Some(old) => std::env::set_var(key, old),
                        None => std::env::remove_var(key),
                    }
                }
            }
        }
    }

    #[test]
    #[serial]
    fn folder_is_mars_home_then_the_renamed_variable_then_home() {
        let cases = [
            ("MARS_HOME wins", Some("/own"), Some("/old"), "/own"),
            ("empty counts as unset", Some(""), Some("/old"), "/old"),
            ("renamed variable", None, Some("/old"), "/old"),
            ("home", None, None, "/h/.mars"),
        ];
        for (case, home_var, renamed, expected) in cases {
            let _env = Env::set(&[
                ("HOME", Some("/h")),
                ("MARS_HOME", home_var),
                ("MARS_CONFIG_DIR", renamed),
                ("XDG_CONFIG_HOME", Some("/xdg")),
            ]);
            assert_eq!(mars_home().as_deref(), Some(Path::new(expected)), "{case}");
        }
    }
}
