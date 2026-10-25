use serde::Serialize;

use crate::config::Config;
use crate::error::{ConfigError, MarsError};

#[derive(Debug, Serialize)]
pub(super) struct PackageAuthor {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

#[derive(Debug)]
pub(super) struct PackageMetadata {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub author: Option<PackageAuthor>,
    pub homepage: Option<String>,
    pub license: Option<String>,
}

impl PackageMetadata {
    pub(super) fn from(manifest: &Config) -> Result<Self, MarsError> {
        let package = manifest.package.as_ref().ok_or_else(|| {
            MarsError::Config(ConfigError::Invalid {
                message: "mars.toml needs a [package] table to build a plugin".to_string(),
            })
        })?;
        Ok(Self {
            name: package.name.clone(),
            version: package.version.clone(),
            description: package.description.clone(),
            author: None,
            homepage: None,
            license: None,
        })
    }
}
