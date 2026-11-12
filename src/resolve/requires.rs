use semver::{Version, VersionReq};

use crate::config::PackageInfo;
use crate::error::ResolutionError;

use super::ResolveOptions;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EngineRequirementFailure {
    pub(super) engine: &'static str,
    pub(super) requirement: String,
    pub(super) running: Version,
}

fn parse_requirement(
    package: &str,
    engine: &'static str,
    raw: &str,
) -> Result<VersionReq, ResolutionError> {
    mars_schema::package::parse_requirement(raw).map_err(|error| {
        ResolutionError::InvalidEngineRequirement {
            package: package.to_string(),
            engine: engine.to_string(),
            requirement: raw.trim().to_string(),
            message: error.to_string(),
        }
    })
}

pub(crate) fn validate_package_requirement_syntax(
    package: &PackageInfo,
) -> Result<(), ResolutionError> {
    if let Some(requirement) = package.requires_mars.as_deref() {
        parse_requirement(&package.name, "mars", requirement)?;
    }
    if let Some(requirement) = package.requires_meridian.as_deref() {
        parse_requirement(&package.name, "meridian", requirement)?;
    }
    Ok(())
}

fn check_engine_requirement(
    package: &str,
    engine: &'static str,
    requirement: Option<&str>,
    running: Option<Version>,
) -> Result<Option<EngineRequirementFailure>, ResolutionError> {
    let (Some(requirement), Some(running)) = (requirement, running) else {
        return Ok(None);
    };
    let parsed = parse_requirement(package, engine, requirement)?;
    if mars_schema::package::requirement_matches(&parsed, &running) {
        Ok(None)
    } else {
        Ok(Some(EngineRequirementFailure {
            engine,
            requirement: requirement.to_string(),
            running,
        }))
    }
}

fn mars_version(options: &ResolveOptions) -> Version {
    options.mars_version.clone().unwrap_or_else(|| {
        Version::parse(env!("CARGO_PKG_VERSION")).expect("crate version is semver")
    })
}

fn parse_meridian_version(
    raw: Option<std::ffi::OsString>,
) -> Result<Option<Version>, ResolutionError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let raw = raw
        .into_string()
        .map_err(|raw| ResolutionError::InvalidRunningEngineVersion {
            engine: "meridian".to_string(),
            version: raw.to_string_lossy().into_owned(),
            message: "value is not valid Unicode".to_string(),
        })?;
    Version::parse(&raw)
        .map(Some)
        .map_err(|error| ResolutionError::InvalidRunningEngineVersion {
            engine: "meridian".to_string(),
            version: raw,
            message: error.to_string(),
        })
}

fn meridian_version(options: &ResolveOptions) -> Result<Option<Version>, ResolutionError> {
    if let Some(version) = &options.meridian_version {
        return Ok(Some(version.clone()));
    }
    parse_meridian_version(std::env::var_os("MERIDIAN_VERSION"))
}

pub(super) fn check_package_requirements(
    package: &PackageInfo,
    options: &ResolveOptions,
) -> Result<Vec<EngineRequirementFailure>, ResolutionError> {
    let mut failures = Vec::new();
    if !options.ignore_requires_mars
        && package.requires_mars.is_some()
        && let Some(failure) = check_engine_requirement(
            &package.name,
            "mars",
            package.requires_mars.as_deref(),
            Some(mars_version(options)),
        )?
    {
        failures.push(failure);
    }
    if !options.ignore_requires_meridian
        && package.requires_meridian.is_some()
        && let Some(failure) = check_engine_requirement(
            &package.name,
            "meridian",
            package.requires_meridian.as_deref(),
            meridian_version(options)?,
        )?
    {
        failures.push(failure);
    }
    Ok(failures)
}

pub(crate) fn check_consumer_package_requirements(
    package: &PackageInfo,
    options: &ResolveOptions,
) -> Result<(), ResolutionError> {
    let failures = check_package_requirements(package, options)?;
    if failures.is_empty() {
        return Ok(());
    }
    Err(ResolutionError::RequiresEngineIncompatible {
        name: package.name.clone(),
        message: format!(
            "consumer package requires {}",
            failures
                .iter()
                .map(|failure| format!(
                    "{} `{}` (running {})",
                    failure.engine, failure.requirement, failure.running
                ))
                .collect::<Vec<_>>()
                .join(" and ")
        ),
    })
}
