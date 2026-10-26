//! What the rest of Mars may know about `.mars/provided.lock`: which skills a provider installed.
//!
//! This is the readers' half of the record, loaded by `sync::load_config` once the sync lock is
//! held. A reader never writes the file, and never fails because of what a host left in it: an
//! absent, corrupt or newer-than-this-Mars record reads as "nothing provided here" with one
//! warning, because a plain run without the host must neither delete provided skills nor fail
//! on them. (`mars provide` itself refuses a newer record; it is the writer.)

use std::path::Path;

use super::lock::ProvidedLock;
use crate::diagnostic::DiagnosticCollector;
use crate::error::{ConfigError, MarsError};
use crate::types::DestPath;

#[derive(Debug, Default)]
pub(crate) struct ProvidedNames {
    lock: ProvidedLock,
}

impl ProvidedNames {
    pub(crate) fn load(root: &Path, diag: &mut DiagnosticCollector) -> Result<Self, MarsError> {
        let loaded = match ProvidedLock::load(root) {
            Ok(loaded) => loaded,
            // A record written by a newer Mars: `load` reports it as an invalid config, and
            // it is the only way `load` produces that error.
            Err(MarsError::Config(ConfigError::Invalid { message })) => {
                diag.warn("provided-lock-unusable", message);
                return Ok(Self::default());
            }
            Err(error) => return Err(error),
        };
        for warning in loaded.warnings {
            diag.warn("provided-lock-unusable", warning);
        }
        Ok(Self {
            lock: loaded.record,
        })
    }

    /// Whether `provider` installed `skill` here.
    pub(crate) fn provides(&self, provider: &str, skill: &str) -> bool {
        self.lock
            .provider(provider)
            .is_some_and(|record| record.skills.iter().any(|name| name == skill))
    }

    /// Whether a provider installed the skill directory at `dest` (`skills/<name>`).
    pub(crate) fn owns(&self, dest: &DestPath) -> bool {
        let Some(name) = dest.as_str().strip_prefix("skills/") else {
            return false;
        };
        self.lock
            .providers()
            .any(|(_, record)| record.skills.iter().any(|skill| skill == name))
    }
}
