//! The user's Mars folders. On Linux and macOS both follow the XDG rule, as uv, mise and gh do:
//! `dirs::config_dir()` is `~/Library/Application Support` on macOS, where nobody looks for a
//! CLI's config. Windows keeps `dirs`.
//!
//! A relative `HOME` or `XDG_*` is ignored, as the XDG spec says: it would name a different
//! folder from every working directory. A relative `MARS_*_DIR` is taken from the working
//! directory, since the user typed it.

use std::path::PathBuf;

/// User-authored Mars configuration: `MARS_CONFIG_DIR`, else `$XDG_CONFIG_HOME/mars`, else
/// `~/.config/mars`.
pub fn mars_config_dir() -> Option<PathBuf> {
    user_dir(
        "MARS_CONFIG_DIR",
        "XDG_CONFIG_HOME",
        ".config",
        dirs::config_dir,
    )
}

/// What Mars generates for the user: `MARS_DATA_DIR`, else `$XDG_DATA_HOME/mars`, else
/// `~/.local/share/mars`.
pub fn mars_data_dir() -> Option<PathBuf> {
    user_dir(
        "MARS_DATA_DIR",
        "XDG_DATA_HOME",
        ".local/share",
        dirs::data_dir,
    )
}

/// The user's file `name` (`config.toml`, `curated.toml`), and, when it is read from where Mars
/// 0.40 and earlier looked on macOS (`~/Library/Application Support/mars`), the path it should
/// move to. That read is a compatibility read for one minor release
/// (`src/config/.context/CONTEXT.md`).
pub fn user_file(name: &str) -> Option<(PathBuf, Option<PathBuf>)> {
    let path = mars_config_dir()?.join(name);
    if cfg!(target_os = "macos") && env_dir("MARS_CONFIG_DIR").is_none() && !path.exists() {
        let legacy = dirs::config_dir().map(|dir| dir.join("mars").join(name));
        if let Some(legacy) = legacy.filter(|legacy| legacy.is_file()) {
            return Some((legacy, Some(path)));
        }
    }
    Some((path, None))
}

fn user_dir(
    own: &str,
    xdg: &str,
    under_home: &str,
    windows: fn() -> Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(dir) = env_dir(own) {
        return Some(std::path::absolute(&dir).unwrap_or(dir));
    }
    let base = if cfg!(windows) {
        windows()
    } else {
        env_dir(xdg)
            .filter(|dir| dir.is_absolute())
            .or_else(|| home_dir().map(|home| home.join(under_home)))
    };
    base.map(|dir| dir.join("mars"))
}

/// The user's home directory, when it is known as an absolute path.
pub fn home_dir() -> Option<PathBuf> {
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
    fn folders_follow_mars_then_xdg_then_home() {
        type Case<'a> = (&'a str, Option<&'a str>, Option<&'a str>, &'a str, &'a str);
        let cases: [Case; 4] = [
            (
                "own variable wins",
                Some("/own"),
                Some("/xdg"),
                "/own",
                "/own",
            ),
            (
                "empty counts as unset",
                Some(""),
                Some("/xdg"),
                "/xdg/mars",
                "/xdg/mars",
            ),
            ("xdg", None, Some("/xdg"), "/xdg/mars", "/xdg/mars"),
            (
                "home",
                None,
                Some(""),
                "/h/.config/mars",
                "/h/.local/share/mars",
            ),
        ];
        for (case, own, xdg, config, data) in cases {
            let _env = Env::set(&[
                ("HOME", Some("/h")),
                ("MARS_CONFIG_DIR", own),
                ("MARS_DATA_DIR", own),
                ("XDG_CONFIG_HOME", xdg),
                ("XDG_DATA_HOME", xdg),
            ]);
            assert_eq!(
                mars_config_dir().as_deref(),
                Some(Path::new(config)),
                "{case}"
            );
            assert_eq!(mars_data_dir().as_deref(), Some(Path::new(data)), "{case}");
        }
    }
}
