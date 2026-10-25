//! Versioned package metadata and manifest classification.

use serde::{Deserialize, Serialize};

/// Package metadata carried by `mars.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PackageMetadata {
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<PackageAuthor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<u64>,
    #[serde(
        default,
        rename = "uses-providers",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub uses_providers: Vec<String>,
    #[serde(
        default,
        rename = "requires-mars",
        skip_serializing_if = "Option::is_none"
    )]
    pub requires_mars: Option<String>,
    #[serde(
        default,
        rename = "requires-meridian",
        skip_serializing_if = "Option::is_none"
    )]
    pub requires_meridian: Option<String>,
}

/// Package author metadata used by generated plugin manifests and catalogs.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PackageAuthor {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

impl<'de> Deserialize<'de> for PackageAuthor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum AuthorValue {
            Text(String),
            Table {
                name: String,
                #[serde(default)]
                email: Option<String>,
            },
        }

        Ok(match AuthorValue::deserialize(deserializer)? {
            AuthorValue::Table { name, email } => Self { name, email },
            AuthorValue::Text(value) => {
                let value = value.trim();
                let (name, email) = value
                    .strip_suffix('>')
                    .and_then(|without_close| without_close.rsplit_once(" <"))
                    .filter(|(name, email)| !name.trim().is_empty() && !email.trim().is_empty())
                    .map_or_else(
                        || (value.to_string(), None),
                        |(name, email)| (name.trim().to_string(), Some(email.trim().to_string())),
                    );
                Self { name, email }
            }
        })
    }
}

/// The schema dispatch result for a package root's optional `mars.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestClassification {
    Schema1,
    SchemaNewer { schema: u64 },
    MarsLegacy,
    Invalid { message: String },
    NoManifest,
}

/// Classify manifest bytes before any package content is lifted or discovered.
pub fn classify_manifest(bytes: Option<&[u8]>) -> ManifestClassification {
    let Some(bytes) = bytes else {
        return ManifestClassification::NoManifest;
    };
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) => {
            return ManifestClassification::Invalid {
                message: format!("mars.toml is not UTF-8: {error}"),
            };
        }
    };
    let manifest: toml::Value = match toml::from_str(text) {
        Ok(manifest) => manifest,
        Err(error) => {
            return ManifestClassification::Invalid {
                message: format!("failed to parse mars.toml: {error}"),
            };
        }
    };
    let Some(package) = manifest.get("package").and_then(toml::Value::as_table) else {
        return ManifestClassification::Invalid {
            message: "mars.toml at a package root must contain [package]".to_string(),
        };
    };
    let Some(schema) = package.get("schema") else {
        return ManifestClassification::MarsLegacy;
    };
    let Some(schema) = schema
        .as_integer()
        .and_then(|value| u64::try_from(value).ok())
    else {
        return ManifestClassification::Invalid {
            message: "[package] schema must be a positive integer".to_string(),
        };
    };
    match schema {
        0 => ManifestClassification::Invalid {
            message: "[package] schema must be a positive integer".to_string(),
        },
        1 => ManifestClassification::Schema1,
        schema => ManifestClassification::SchemaNewer { schema },
    }
}
