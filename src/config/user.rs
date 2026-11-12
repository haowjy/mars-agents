use std::path::PathBuf;

use crate::error::{ConfigError, MarsError};
use mars_schema::keys::{self, Surface};

use super::LocalConfig;

/// The user's `config.toml`: the `mars.local.toml` schema, layered beneath `mars.toml`.
#[derive(Debug, Clone, Default)]
pub struct UserConfig {
    /// Names the file in warnings; empty when no user file exists, and then nothing warns.
    pub path: PathBuf,
    pub local: LocalConfig,
    /// A warning for each key the file holds that belongs in a project's files; the layer drops it.
    pub misplaced: Vec<String>,
}

pub(super) fn load() -> Result<UserConfig, MarsError> {
    let directory =
        crate::platform::config::mars_config_dir().ok_or_else(|| ConfigError::Invalid {
            message: "cannot determine config directory; set MARS_CONFIG_DIR".to_string(),
        })?;
    let path = directory.join("config.toml");
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(UserConfig::default());
        }
        Err(source) => return Err(super::read_error(&path, source)),
    };
    let file = path.display().to_string();
    let local = super::parse_local(&content, &path, &file)?;
    let (local, misplaced) = without_project_keys(local, &content, &file)?;
    Ok(UserConfig {
        path,
        local,
        misplaced,
    })
}

/// Drop the keys the key table says belong in a project's files. A user-wide `targets` or
/// override would apply to every project unseen, and ignoring either loses no restriction.
fn without_project_keys(
    local: LocalConfig,
    content: &str,
    file: &str,
) -> Result<(LocalConfig, Vec<String>), MarsError> {
    let authored: toml::Table = toml::from_str(content).expect("the file parsed as a config");
    let mut misplaced = Vec::new();
    let mut rewritten = toml::Table::try_from(&local).map_err(|error| ConfigError::Invalid {
        message: format!("{file}: {error}"),
    })?;
    for (surface, table) in [
        (Surface::UserConfig, ""),
        (Surface::UserSettings, "settings"),
    ] {
        let authored = if table.is_empty() {
            Some(&authored)
        } else {
            authored.get(table).and_then(toml::Value::as_table)
        };
        for key in authored.into_iter().flat_map(toml::Table::keys) {
            let Some(advice) = keys::elsewhere_key(surface, key) else {
                continue;
            };
            let place = if table.is_empty() {
                format!("`[{key}]`")
            } else {
                format!("[{table}] `{key}`")
            };
            misplaced.push(format!(
                "{file}: {place} is not read in the user config; {advice}"
            ));
            if table.is_empty() {
                rewritten.remove(key);
            } else if let Some(toml::Value::Table(settings)) = rewritten.get_mut(table) {
                settings.remove(key);
            }
        }
    }
    if misplaced.is_empty() {
        return Ok((local, misplaced));
    }
    let (unknown_keys, floor) = (local.unknown_keys.clone(), local.floor.clone());
    let mut stripped: LocalConfig =
        rewritten
            .try_into()
            .map_err(|error: toml::de::Error| ConfigError::Invalid {
                message: format!("{file}: {error}"),
            })?;
    stripped.unknown_keys = unknown_keys;
    stripped.floor = floor;
    Ok((stripped, misplaced))
}
