use std::path::PathBuf;

use crate::error::{ConfigError, MarsError};

use super::LocalConfig;

/// The user's `config.toml`: the `mars.local.toml` schema, layered beneath `mars.toml`.
#[derive(Debug, Clone, Default)]
pub struct UserConfig {
    /// Names the file in warnings; empty when no user file exists, and then nothing warns.
    pub path: PathBuf,
    pub local: LocalConfig,
    /// `[settings]` keys that describe one project, removed from `local` and reported by name.
    pub ignored_settings: Vec<&'static str>,
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
    let mut local: LocalConfig = super::parse_without_removed_visibility(&content, &path)?;
    super::validate_agent_overlays(&path, &local.agents)?;
    // A user-wide override would point every project at a local checkout unseen.
    if !local.overrides.is_empty() {
        return Err(ConfigError::Invalid {
            message: format!(
                "{}: [overrides] is not accepted in the user config; put it in the project's mars.local.toml",
                path.display()
            ),
        }
        .into());
    }
    let settings = &mut local.settings;
    let mut ignored_settings = Vec::new();
    if settings.managed_root.take().is_some() {
        ignored_settings.push("managed_root");
    }
    if settings.targets.take().is_some() {
        ignored_settings.push("targets");
    }
    if settings.min_mars_version.take().is_some() {
        ignored_settings.push("min_mars_version");
    }
    Ok(UserConfig {
        path,
        local,
        ignored_settings,
    })
}
