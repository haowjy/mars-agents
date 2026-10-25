use crate::config::Config;
use crate::error::{ConfigError, MarsError};

pub(super) use crate::config::PackageAuthor;

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
        let plugin = manifest.plugin.as_ref();
        Ok(Self {
            name: plugin
                .map(|plugin| plugin.name(package))
                .unwrap_or(&package.name)
                .to_string(),
            version: package.version.clone(),
            description: match plugin {
                Some(plugin) => plugin.description(package),
                None => package.description.as_deref(),
            }
            .map(str::to_string),
            author: package.authors.first().cloned(),
            homepage: package.homepage.clone(),
            license: package.license.clone(),
        })
    }
}
