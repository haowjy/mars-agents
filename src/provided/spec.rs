//! The `mars provide` spec: `<provider>@<version>=<directory>`.

use std::path::PathBuf;

use crate::error::MarsError;

/// One provider's bundle, as the host names it on the command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProvideSpec {
    /// The provider's own namespace, separate from dependency keys.
    pub provider: String,
    /// Opaque: recorded for display and change detection, never parsed or compared as semver.
    pub version: String,
    /// The directory holding `skills/`.
    pub package_dir: PathBuf,
}

impl ProvideSpec {
    pub(crate) fn parse(raw: &str) -> Result<Self, MarsError> {
        let invalid = |message: String| MarsError::InvalidRequest {
            message: format!(
                "provided-spec-invalid: `{raw}` is not a provide spec ({message}); expected `<provider>@<version>=<directory>`"
            ),
        };
        let Some((provider_version, package_dir)) = raw.split_once('=') else {
            return Err(invalid("it has no `=`".to_string()));
        };
        let Some((provider, version)) = provider_version.split_once('@') else {
            return Err(invalid(
                "the provider and version are not separated by `@`".to_string(),
            ));
        };
        if !mars_schema::provider::is_provider_name(provider) {
            return Err(invalid(format!(
                "provider `{provider}` must match `[a-z0-9][a-z0-9-]*`"
            )));
        }
        if version.is_empty() {
            return Err(invalid("the version is empty".to_string()));
        }
        if package_dir.is_empty() {
            return Err(invalid("the directory is empty".to_string()));
        }
        Ok(ProvideSpec {
            provider: provider.to_string(),
            version: version.to_string(),
            package_dir: PathBuf::from(package_dir),
        })
    }
}
