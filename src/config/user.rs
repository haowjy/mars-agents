use std::collections::BTreeMap;

use serde::Deserialize;

use mars_schema::network::{NetworkHost, NetworkPolicy};
use mars_schema::permission_set::PermissionSet;

use crate::error::{ConfigError, MarsError};

#[derive(Debug, Default, Deserialize)]
struct UserConfigFile {
    #[serde(default)]
    settings: UserNetworkSettings,
    #[serde(flatten)]
    ignored: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Default, Deserialize)]
struct UserNetworkSettings {
    #[serde(default, alias = "default-network")]
    default_network: Option<NetworkPolicy>,
    #[serde(default, rename = "network-allow", alias = "network_allow")]
    network_allow: PermissionSet<NetworkHost>,
    #[serde(default, rename = "network-deny", alias = "network_deny")]
    network_deny: PermissionSet<NetworkHost>,
    #[serde(flatten)]
    ignored: BTreeMap<String, toml::Value>,
}

pub(super) struct UserNetworkConfig {
    pub(super) default_network: Option<NetworkPolicy>,
    pub(super) network_allow: PermissionSet<NetworkHost>,
    pub(super) network_deny: PermissionSet<NetworkHost>,
    pub(super) warnings: Vec<String>,
}

pub(super) fn load() -> Result<Option<UserNetworkConfig>, MarsError> {
    let directory =
        crate::platform::config::mars_config_dir().ok_or_else(|| ConfigError::Invalid {
            message: "cannot determine config directory; set MARS_CONFIG_DIR".to_string(),
        })?;
    let path = directory.join("config.toml");
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(MarsError::Io {
                operation: "read user config".to_string(),
                path,
                source,
            });
        }
    };
    let user: UserConfigFile = toml::from_str(&content).map_err(|error| ConfigError::Invalid {
        message: format!("failed to parse user config {}: {error}", path.display()),
    })?;
    let mut warnings = user
        .ignored
        .keys()
        .map(|key| {
            format!(
                "user config {}: `{key}` is ignored; only [settings] network defaults are supported",
                path.display()
            )
        })
        .collect::<Vec<_>>();
    warnings.extend(user.settings.ignored.keys().map(|key| {
        format!(
            "user config {}: [settings].{key} is ignored; accepted keys: {}",
            path.display(),
            super::NETWORK_SETTINGS_KEYS,
        )
    }));
    Ok(Some(UserNetworkConfig {
        default_network: user.settings.default_network,
        network_allow: user.settings.network_allow,
        network_deny: user.settings.network_deny,
        warnings,
    }))
}
