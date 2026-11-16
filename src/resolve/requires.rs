use mars_schema::resolve::{BadVersion, Engine, Failure};
use semver::Version;

use crate::config::PackageInfo;
use crate::error::ResolutionError;

use super::ResolveOptions;

/// The versions the running Mars and Meridian are judged by. `MERIDIAN_VERSION` is read here but
/// fails only for a package that declares `requires-meridian`.
pub(super) fn engine(options: &ResolveOptions) -> Engine {
    Engine {
        mars: options.mars_version.clone().unwrap_or_else(|| {
            Version::parse(env!("CARGO_PKG_VERSION")).expect("crate version is semver")
        }),
        meridian: match &options.meridian_version {
            Some(version) => Some(Ok(version.clone())),
            None => meridian_from_env(std::env::var_os("MERIDIAN_VERSION")),
        },
        ignore_requires_mars: options.ignore_requires_mars,
        ignore_requires_meridian: options.ignore_requires_meridian,
    }
}

fn meridian_from_env(raw: Option<std::ffi::OsString>) -> Option<Result<Version, BadVersion>> {
    let raw = raw?;
    Some(match raw.into_string() {
        Err(raw) => Err(BadVersion {
            version: raw.to_string_lossy().into_owned(),
            message: "value is not valid Unicode".to_string(),
        }),
        Ok(raw) => Version::parse(&raw).map_err(|error| BadVersion {
            version: raw,
            message: error.to_string(),
        }),
    })
}

/// The error for an engine failure the check found in a requirement or a running version.
pub(super) fn engine_error(failure: Failure) -> ResolutionError {
    match failure {
        Failure::EngineRequirementInvalid {
            name,
            engine,
            requirement,
            message,
        } => ResolutionError::InvalidEngineRequirement {
            package: name,
            engine,
            requirement,
            message,
        },
        Failure::EngineVersionInvalid {
            engine,
            version,
            message,
        } => ResolutionError::InvalidRunningEngineVersion {
            engine,
            version,
            message,
        },
        other => unreachable!("an engine check fails only as {other:?}"),
    }
}

/// Whether the engines meet `package`'s `requires-*`.
pub(super) fn engines_accept(
    package: &PackageInfo,
    options: &ResolveOptions,
) -> Result<bool, ResolutionError> {
    Ok(engine(options)
        .check(package)
        .map_err(engine_error)?
        .is_empty())
}

pub(crate) fn check_consumer_package_requirements(
    package: &PackageInfo,
    options: &ResolveOptions,
) -> Result<(), ResolutionError> {
    let failures = engine(options).check(package).map_err(engine_error)?;
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
