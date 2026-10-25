use std::collections::BTreeSet;
use std::path::PathBuf;

use serde::Serialize;

use crate::config::Config;
use crate::error::ConfigError;
use crate::plugin::{PluginHarnessSpec, harness_specs};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct PackageAuthor {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

#[derive(Clone)]
pub(crate) struct PackageMetadata {
    pub name: String,
    pub marketplace: String,
    pub version: String,
    pub description: Option<String>,
    pub authors: Vec<PackageAuthor>,
    pub homepage: Option<String>,
    pub license: Option<String>,
    pub dir: PathBuf,
    pub harnesses: Vec<&'static PluginHarnessSpec>,
}

impl PackageMetadata {
    pub(crate) fn author(&self) -> Option<&PackageAuthor> {
        self.authors.first()
    }
}

pub(crate) fn validate(config: &Config) -> Result<PackageMetadata, ConfigError> {
    let package = config
        .package
        .as_ref()
        .ok_or_else(|| invalid("mars.toml needs a [package] table to build a plugin"))?;
    let plugin = config.plugin.as_ref();

    let dir = plugin
        .and_then(|plugin| plugin.get("dir"))
        .map(|value| required_string("[plugin].dir", value).map(PathBuf::from))
        .transpose()?
        .unwrap_or_else(|| PathBuf::from("plugin"));
    crate::config::validate_plugin_dir(&dir)?;

    let harnesses = parse_harnesses(plugin.and_then(|plugin| plugin.get("harnesses")))?;
    let authors = parse_authors(package.authors.as_ref())?;
    let homepage = optional_string("[package].homepage", package.homepage.as_ref())?;
    let license = optional_string("[package].license", package.license.as_ref())?;
    let name = optional_string(
        "[plugin].name",
        plugin.and_then(|plugin| plugin.get("name")),
    )?
    .unwrap_or_else(|| package.name.clone());
    let marketplace = optional_string(
        "[plugin].marketplace",
        plugin.and_then(|plugin| plugin.get("marketplace")),
    )?
    .unwrap_or_else(|| package.name.clone());
    let description = optional_string(
        "[plugin].description",
        plugin.and_then(|plugin| plugin.get("description")),
    )?
    .or_else(|| package.description.clone());

    let name_field = if plugin.is_some_and(|plugin| plugin.get("name").is_some()) {
        "[plugin].name"
    } else {
        "[package].name"
    };
    for spec in &harnesses {
        if !spec.name_rule.accepts(&name) {
            return Err(invalid(format!(
                "mars.toml {name_field} `{name}` is invalid: {}",
                spec.name_rule.message
            )));
        }
    }

    let marketplace_field = if plugin.is_some_and(|plugin| plugin.get("marketplace").is_some()) {
        "[plugin].marketplace"
    } else {
        "[package].name"
    };
    let mut checked = BTreeSet::new();
    for spec in &harnesses {
        if !checked.insert(spec.tree.as_str()) {
            continue;
        }
        let Some(rule) = spec.tree.spec().marketplace_name_rule else {
            continue;
        };
        if !rule.accepts(&marketplace) {
            return Err(invalid(format!(
                "mars.toml {marketplace_field} `{marketplace}` is invalid as a marketplace: {}",
                rule.message
            )));
        }
    }

    Ok(PackageMetadata {
        name,
        marketplace,
        version: package.version.clone(),
        description,
        authors,
        homepage,
        license,
        dir,
        harnesses,
    })
}

pub(crate) fn validate_package_values(config: &Config) -> Result<(), ConfigError> {
    let Some(package) = config.package.as_ref() else {
        return Ok(());
    };
    parse_authors(package.authors.as_ref())?;
    optional_string("[package].homepage", package.homepage.as_ref())?;
    optional_string("[package].license", package.license.as_ref())?;
    Ok(())
}

fn parse_harnesses(
    value: Option<&toml::Value>,
) -> Result<Vec<&'static PluginHarnessSpec>, ConfigError> {
    let Some(value) = value else {
        return Ok(harness_specs().collect());
    };
    let values = value
        .as_array()
        .ok_or_else(|| invalid_value("[plugin].harnesses", value, "an array of strings"))?;
    if values.is_empty() {
        return Err(invalid("a plugin build needs at least one harness"));
    }

    let names = values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            value.as_str().map(str::to_owned).ok_or_else(|| {
                invalid_value(&format!("[plugin].harnesses[{index}]"), value, "a string")
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if let Some(message) = crate::plugin::unknown_plugin_harness_messages(&names)
        .into_iter()
        .next()
    {
        return Err(invalid(message));
    }

    Ok(harness_specs()
        .filter(|spec| names.iter().any(|name| name == spec.key))
        .collect())
}

fn parse_authors(value: Option<&toml::Value>) -> Result<Vec<PackageAuthor>, ConfigError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let values = value
        .as_array()
        .ok_or_else(|| invalid_value("[package].authors", value, "an array"))?;
    values
        .iter()
        .enumerate()
        .map(|(index, value)| parse_author(index, value))
        .collect()
}

fn parse_author(index: usize, value: &toml::Value) -> Result<PackageAuthor, ConfigError> {
    let (name, email) = match value {
        toml::Value::String(value) => {
            let value = value.trim();
            value
                .strip_suffix('>')
                .and_then(|without_close| without_close.rsplit_once(" <"))
                .filter(|(name, email)| !name.trim().is_empty() && !email.trim().is_empty())
                .map_or_else(
                    || (value.to_string(), None),
                    |(name, email)| (name.trim().to_string(), Some(email.trim().to_string())),
                )
        }
        toml::Value::Table(table) => {
            let name_value = table.get("name").ok_or_else(|| {
                invalid_value(
                    &format!("[package].authors[{index}]"),
                    value,
                    "a string or a table with a string `name`",
                )
            })?;
            let name = required_string(&format!("[package].authors[{index}].name"), name_value)?
                .to_string();
            let email = table
                .get("email")
                .map(|value| {
                    required_string(&format!("[package].authors[{index}].email"), value)
                        .map(str::to_owned)
                })
                .transpose()?;
            (name, email)
        }
        _ => {
            return Err(invalid_value(
                &format!("[package].authors[{index}]"),
                value,
                "a string or a table with a string `name`",
            ));
        }
    };
    if name.trim().is_empty() {
        return Err(invalid(format!(
            "mars.toml [package].authors[{index}].name cannot be empty; value was `{value}`"
        )));
    }
    Ok(PackageAuthor { name, email })
}

fn optional_string(
    field: &str,
    value: Option<&toml::Value>,
) -> Result<Option<String>, ConfigError> {
    value
        .map(|value| required_string(field, value).map(str::to_owned))
        .transpose()
}

fn required_string<'a>(field: &str, value: &'a toml::Value) -> Result<&'a str, ConfigError> {
    value
        .as_str()
        .ok_or_else(|| invalid_value(field, value, "a string"))
}

fn invalid_value(field: &str, value: &toml::Value, expected: &str) -> ConfigError {
    invalid(format!(
        "mars.toml {field} has invalid value `{value}`; expected {expected}"
    ))
}

fn invalid(message: impl Into<String>) -> ConfigError {
    ConfigError::Invalid {
        message: message.into(),
    }
}
