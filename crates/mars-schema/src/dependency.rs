//! Rules for one `[dependencies]` entry that hold wherever it is read.

use crate::dialect::Dialect;
use crate::names::SourceSubpath;

/// A dependency entry that selects a marketplace plugin with a key that contradicts it.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PluginSelectorError {
    #[error("source `{name}` has `plugin = \"{plugin}\"`; a plugin name is {rule}")]
    BadName {
        name: String,
        plugin: String,
        rule: &'static str,
    },
    #[error(
        "source `{name}` has both `plugin` and `subpath` — a marketplace is read at the root of its source, and the marketplace entry says where the plugin is"
    )]
    WithSubpath { name: String },
    #[error(
        "source `{name}` has both `plugin` and `dialect` — a marketplace plugin is read as a Claude plugin"
    )]
    WithDialect { name: String },
}

/// `plugin` picks one plugin of the marketplace at the source's root, read as a Claude
/// marketplace; a `subpath` or `dialect` would name a different root or reading.
pub fn validate_plugin_selector(
    name: &str,
    plugin: Option<&str>,
    subpath: Option<&SourceSubpath>,
    dialect: Option<Dialect>,
) -> Result<(), PluginSelectorError> {
    let Some(plugin) = plugin else {
        return Ok(());
    };
    let name = name.to_string();
    if !crate::claude::manifest::is_plugin_name(plugin) {
        return Err(PluginSelectorError::BadName {
            name,
            plugin: plugin.to_string(),
            rule: crate::claude::manifest::PLUGIN_NAME_RULE,
        });
    }
    if subpath.is_some() {
        return Err(PluginSelectorError::WithSubpath { name });
    }
    if dialect.is_some() {
        return Err(PluginSelectorError::WithDialect { name });
    }
    Ok(())
}
