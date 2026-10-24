use std::path::PathBuf;

/// User-authored Mars configuration directory.
pub fn mars_config_dir() -> Option<PathBuf> {
    std::env::var_os("MARS_CONFIG_DIR")
        .filter(|directory| !directory.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::config_dir().map(|directory| directory.join("mars")))
}
