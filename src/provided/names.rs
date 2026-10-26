//! What the rest of Mars may know about `.mars/provided.lock`: which skills a provider installed.
//!
//! This is the readers' half of the record, loaded by `sync::load_config` once the sync lock is
//! held. A reader never writes the file, and never fails because of what a host left in it: an
//! absent, corrupt or newer-than-this-Mars record reads as "nothing provided here" (one warning
//! for the last two), because a plain run without the host must neither delete provided skills
//! nor fail on them. (`mars provide` itself refuses a newer record; it is the writer.)
//!
//! Availability is the disk's, ownership is the record's. A recorded name counts only while
//! `.mars/skills/<name>` is a directory — the record is a superset of the disk by design, so a
//! name it lists with no directory is a leftover, not something an agent can use. Ownership
//! keeps the full record instead: owning an absent path is harmless, and the record must cover a
//! directory a crash has not left behind yet.

use std::collections::HashSet;
use std::path::Path;

use super::lock::ProvidedLock;
use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::types::DestPath;

#[derive(Debug, Default)]
pub(crate) struct ProvidedNames {
    /// The full record: ownership is the record's, deliberately a superset of the disk.
    lock: ProvidedLock,
    /// Recorded names whose `.mars/skills/<name>` is a directory: what an agent can use here.
    installed: HashSet<String>,
}

impl ProvidedNames {
    pub(crate) fn load(root: &Path, diag: &mut DiagnosticCollector) -> Result<Self, MarsError> {
        let loaded = ProvidedLock::load(root)?;
        let Some(unusable) = loaded.unusable else {
            let skills_dir = root.join(".mars").join("skills");
            let installed = loaded
                .record
                .providers()
                .flat_map(|(_, record)| record.skills.iter())
                .filter(|name| skills_dir.join(name).is_dir())
                .cloned()
                .collect();
            return Ok(Self {
                lock: loaded.record,
                installed,
            });
        };
        diag.warn("provided-lock-unusable", unusable.reader_message());
        Ok(Self::default())
    }

    /// Whether `provider` installed `skill` here: the record lists it and its directory is on
    /// disk. A recorded name whose directory is gone — a package replaced it and was removed, or
    /// a crash interrupted the apply — does not satisfy a ref.
    pub(crate) fn provides(&self, provider: &str, skill: &str) -> bool {
        self.installed.contains(skill)
            && self
                .lock
                .provider(provider)
                .is_some_and(|record| record.skills.iter().any(|name| name == skill))
    }

    /// Whether this machine records `provider` at all. `mars doctor` uses it to tell a name a
    /// host may supply from one that merely looks like a provider's.
    pub(crate) fn records_provider(&self, provider: &str) -> bool {
        self.lock.provider(provider).is_some()
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
