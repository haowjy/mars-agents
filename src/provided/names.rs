//! What the rest of Mars may know about `.mars/provided.lock`: which skills a provider installed.
//!
//! This is the readers' half of the record, loaded by `sync::load_config` once the sync lock is
//! held. A reader never writes the file, and never fails because of what a host left in it: an
//! absent, corrupt or newer-than-this-Mars record reads as "nothing provided here" (one warning
//! for the last two), because a plain run without the host must neither delete provided skills
//! nor fail on them. (`mars provide` itself refuses a newer record; it is the writer.)
//!
//! The record is flattened here on purpose: the loader rejects any name filed under a provider
//! that is not its own (`<skill>__<provider>`), so a reader that has the name has every fact the
//! provider key could add. Read it as two sets.
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
    /// Every name the record lists, across providers: ownership is the record's, deliberately a
    /// superset of the disk.
    recorded: HashSet<String>,
    /// Recorded names whose `.mars/skills/<name>` is a directory: what an agent can use here.
    installed: HashSet<String>,
}

impl ProvidedNames {
    pub(crate) fn load(root: &Path, diag: &mut DiagnosticCollector) -> Result<Self, MarsError> {
        let loaded = ProvidedLock::load(root)?;
        let Some(unusable) = loaded.unusable else {
            let skills_dir = root.join(".mars").join("skills");
            let recorded: HashSet<String> = loaded
                .record
                .providers()
                .flat_map(|(_, provider)| provider.skills.iter().cloned())
                .collect();
            let installed = recorded
                .iter()
                .filter(|name| skills_dir.join(name).is_dir())
                .cloned()
                .collect();
            return Ok(Self {
                recorded,
                installed,
            });
        };
        diag.warn("provided-lock-unusable", unusable.reader_message());
        Ok(Self::default())
    }

    /// Whether a provider installed `name` here. A recorded name whose directory is gone — a
    /// package replaced it and was removed, or a crash interrupted the apply — is not available.
    pub(crate) fn provides(&self, name: &str) -> bool {
        self.installed.contains(name)
    }

    /// Whether a provider installed the skill directory at `dest` (`skills/<name>`).
    pub(crate) fn owns(&self, dest: &DestPath) -> bool {
        dest.as_str()
            .strip_prefix("skills/")
            .is_some_and(|name| self.recorded.contains(name))
    }
}
