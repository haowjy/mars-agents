use semver::Version;

use super::Failure;
use crate::package::PackageMetadata;

/// The versions `requires-mars` and `requires-meridian` are judged against.
#[derive(Debug, Clone)]
pub struct Engine {
    pub mars: Version,
    /// `None`: no Meridian to judge against. An `Err` is reported only when a package declares
    /// `requires-meridian`, so a bad `MERIDIAN_VERSION` costs only the projects that ask.
    pub meridian: Option<Result<Version, BadVersion>>,
    pub ignore_requires_mars: bool,
    pub ignore_requires_meridian: bool,
}

/// A running engine version that is not a version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BadVersion {
    pub version: String,
    pub message: String,
}

/// One unmet `requires-*`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineFailure {
    pub engine: &'static str,
    pub requirement: String,
    pub running: Version,
}

impl Engine {
    pub fn check(&self, package: &PackageMetadata) -> Result<Vec<EngineFailure>, Failure> {
        let mut failures = Vec::new();
        if !self.ignore_requires_mars
            && let Some(requirement) = package.requires_mars.as_deref()
            && let Some(failure) = check("mars", &package.name, requirement, &self.mars)?
        {
            failures.push(failure);
        }
        if !self.ignore_requires_meridian
            && let Some(requirement) = package.requires_meridian.as_deref()
        {
            match &self.meridian {
                Some(Err(bad)) => {
                    return Err(Failure::EngineVersionInvalid {
                        engine: "meridian".to_string(),
                        version: bad.version.clone(),
                        message: bad.message.clone(),
                    });
                }
                Some(Ok(running)) => {
                    if let Some(failure) = check("meridian", &package.name, requirement, running)? {
                        failures.push(failure);
                    }
                }
                None => {}
            }
        }
        Ok(failures)
    }
}

fn check(
    engine: &'static str,
    package: &str,
    requirement: &str,
    running: &Version,
) -> Result<Option<EngineFailure>, Failure> {
    let parsed = crate::package::parse_requirement(requirement).map_err(|error| {
        Failure::EngineRequirementInvalid {
            name: package.to_string(),
            engine: engine.to_string(),
            requirement: requirement.trim().to_string(),
            message: error.to_string(),
        }
    })?;
    Ok(
        (!crate::package::requirement_matches(&parsed, running)).then(|| EngineFailure {
            engine,
            requirement: requirement.to_string(),
            running: running.clone(),
        }),
    )
}

pub(super) fn describe(failures: &[EngineFailure]) -> String {
    failures
        .iter()
        .map(|failure| {
            format!(
                "requires-{} `{}` (running {}; use a {} version matching `{}`)",
                failure.engine,
                failure.requirement,
                failure.running,
                failure.engine,
                failure.requirement
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}
