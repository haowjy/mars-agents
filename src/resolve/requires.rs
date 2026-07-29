use semver::{BuildMetadata, Prerelease, Version, VersionReq};

use crate::config::PackageInfo;
use crate::error::ResolutionError;

use super::ResolveOptions;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EngineRequirementFailure {
    pub(crate) engine: &'static str,
    pub(crate) requirement: String,
    pub(crate) running: Version,
}

fn parse_requirement(
    package: &str,
    engine: &'static str,
    raw: &str,
) -> Result<VersionReq, ResolutionError> {
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
    VersionReq::parse(&normalized).map_err(|error| ResolutionError::InvalidEngineRequirement {
        package: package.to_string(),
        engine: engine.to_string(),
        requirement: raw.to_string(),
        message: error.to_string(),
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

fn stable_running_version(version: &Version) -> Version {
    let mut stable = version.clone();
    stable.pre = Prerelease::EMPTY;
    stable.build = BuildMetadata::EMPTY;
    stable
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
    if parsed.matches(&stable_running_version(&running)) {
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

pub(crate) fn check_package_requirements(
    package: &PackageInfo,
    options: &ResolveOptions,
) -> Result<Vec<EngineRequirementFailure>, ResolutionError> {
    let mut failures = Vec::new();
    if !options.ignore_requires_mars
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

#[cfg(test)]
mod tests {
    use super::*;

    fn package(requirement: Option<&str>) -> PackageInfo {
        PackageInfo {
            name: "pkg".into(),
            version: "1.0.0".into(),
            description: None,
            requires_mars: requirement.map(str::to_string),
            requires_meridian: None,
        }
    }

    #[test]
    fn bare_version_is_a_minimum() {
        let options = ResolveOptions {
            mars_version: Some(Version::new(0, 13, 0)),
            ..ResolveOptions::default()
        };
        assert!(
            check_package_requirements(&package(Some("0.12")), &options)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn prerelease_running_version_matches_stable_floor() {
        let options = ResolveOptions {
            mars_version: Some(Version::parse("0.12.0-rc.1").unwrap()),
            ..ResolveOptions::default()
        };
        assert!(
            check_package_requirements(&package(Some(">=0.12.0")), &options)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn meridian_absence_skips_requirement() {
        let mut package = package(None);
        package.requires_meridian = Some(">=999.0.0".into());
        assert!(
            check_engine_requirement(
                &package.name,
                "meridian",
                package.requires_meridian.as_deref(),
                None,
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn absent_meridian_version_is_the_only_silent_skip() {
        assert_eq!(parse_meridian_version(None).unwrap(), None);
    }

    #[test]
    fn malformed_meridian_version_is_an_error() {
        let error = parse_meridian_version(Some("not-semver".into())).unwrap_err();
        assert!(matches!(
            error,
            ResolutionError::InvalidRunningEngineVersion { engine, version, .. }
                if engine == "meridian" && version == "not-semver"
        ));
    }

    #[test]
    fn invalid_requirement_is_not_treated_as_a_ref_pin() {
        let error = parse_requirement("pkg", "mars", "definitely-not-semver").unwrap_err();
        assert!(matches!(
            error,
            ResolutionError::InvalidEngineRequirement { .. }
        ));
    }

    #[test]
    fn syntax_validation_accepts_bare_and_range_requirements_for_both_engines() {
        for requirement in ["0.12", ">=0.12.0", ">=0.12, <2"] {
            let mut package = package(Some(requirement));
            package.requires_meridian = Some(requirement.into());
            validate_package_requirement_syntax(&package).unwrap();
        }
    }

    #[test]
    fn syntax_validation_rejects_garbage_for_either_engine() {
        let mut mars_package = package(Some("not a version"));
        assert!(validate_package_requirement_syntax(&mars_package).is_err());
        mars_package.requires_mars = None;
        mars_package.requires_meridian = Some("still not a version".into());
        assert!(validate_package_requirement_syntax(&mars_package).is_err());
    }

    #[test]
    fn consumer_requirement_is_hard_unless_ignored() {
        let options = ResolveOptions {
            mars_version: Some(Version::new(0, 11, 0)),
            ..ResolveOptions::default()
        };
        let package = package(Some(">=99"));
        assert!(check_consumer_package_requirements(&package, &options).is_err());

        let ignored = ResolveOptions {
            ignore_requires_mars: true,
            ..options
        };
        check_consumer_package_requirements(&package, &ignored).unwrap();
    }

    #[test]
    fn old_manifest_serialization_does_not_gain_engine_fields() {
        let raw = "name = \"pkg\"\nversion = \"1.0.0\"\n";
        let parsed: PackageInfo = toml::from_str(raw).unwrap();
        assert_eq!(parsed.requires_mars, None);
        assert_eq!(parsed.requires_meridian, None);
        let serialized = toml::to_string(&parsed).unwrap();
        assert!(!serialized.contains("requires-mars"));
        assert!(!serialized.contains("requires-meridian"));
    }
}
