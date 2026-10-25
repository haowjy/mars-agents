//! Package-manifest reading owned by the schema boundary.

use serde::Deserialize;

use crate::package::PackageMetadata;

#[derive(Deserialize)]
struct PackageSection {
    package: Option<PackageMetadata>,
}

/// Read the optional `[package]` section while ignoring consumer-only tables.
pub fn read_package(content: &str) -> Result<Option<PackageMetadata>, toml::de::Error> {
    toml::from_str::<PackageSection>(content).map(|manifest| manifest.package)
}
