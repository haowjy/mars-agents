//! Versioned package metadata and manifest classification.

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

/// The first Mars release that reads schema 1. An older one reads a `schema = 1` package as
/// pre-v1, so a schema v1 package's `requires-mars` may not admit one.
pub const SCHEMA_1_MARS_FLOOR: &str = "0.20.0";

/// Parse a `requires-mars` or `requires-meridian` requirement. A bare version means `>=` it
/// (`0.19.0` is `>=0.19.0`), as `mars.toml` documents both fields.
pub fn parse_requirement(raw: &str) -> Result<VersionReq, semver::Error> {
    let raw = raw.trim();
    let parts: Vec<&str> = raw.split('.').collect();
    let normalized = if (1..=3).contains(&parts.len())
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
    {
        let mut parts = parts;
        while parts.len() < 3 {
            parts.push("0");
        }
        format!(">={}", parts.join("."))
    } else {
        raw.to_string()
    };
    VersionReq::parse(&normalized)
}

/// Whether `requirement` demands at least `floor`, so a Mars older than one floor cannot read
/// the package. A range with no lower bound (`<0.25`) does not; an unparseable one cannot say.
pub fn requirement_has_floor(requirement: Option<&str>, floor: &Version) -> bool {
    let Some(raw) = requirement else {
        return false;
    };
    let Ok(requirement) = parse_requirement(raw) else {
        return false;
    };
    requirement
        .comparators
        .iter()
        .any(|comparator| comparator_floor(comparator).is_some_and(|minimum| minimum >= *floor))
}

fn comparator_floor(comparator: &semver::Comparator) -> Option<Version> {
    use semver::Op;

    // Partial strict bounds advance to the next specified component: `>0.18`
    // means `>=0.19.0`, unlike the fully specified `>0.18.0`.
    let (major, minor, patch) = match comparator.op {
        Op::Greater if comparator.minor.is_none() => (comparator.major.checked_add(1)?, 0, 0),
        Op::Greater if comparator.patch.is_none() => {
            (comparator.major, comparator.minor?.checked_add(1)?, 0)
        }
        Op::Exact | Op::Greater | Op::GreaterEq | Op::Tilde | Op::Caret | Op::Wildcard => (
            comparator.major,
            comparator.minor.unwrap_or(0),
            comparator.patch.unwrap_or(0),
        ),
        _ => return None,
    };
    let mut version = Version::new(major, minor, patch);
    version.pre = comparator.pre.clone();
    Some(version)
}

/// Package metadata carried by `mars.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PackageMetadata {
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authors: Option<toml::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<toml::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<toml::Value>,
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

/// The schema dispatch result for a package root's optional `mars.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestClassification {
    Schema1,
    SchemaNewer {
        schema: u64,
    },
    MarsLegacy,
    /// A `mars.toml` with no `[package]`: a consumer project's configuration, which makes the
    /// directory a project and not a package.
    ConsumerConfig,
    Invalid {
        message: String,
    },
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
    let Some(package) = manifest.get("package") else {
        return ManifestClassification::ConsumerConfig;
    };
    let Some(package) = package.as_table() else {
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
