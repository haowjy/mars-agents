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

use mars_schema::item::ItemKind;
use mars_schema::provider::provider_of;

use super::lock::ProvidedLock;
use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::lock::ItemId;
use crate::sync::apply::{ActionOutcome, ActionTaken};
use crate::types::{ContentHash, DestPath};

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
        let record = match ProvidedLock::load(root)? {
            Ok(record) => record,
            Err(unusable) => {
                diag.warn_advisory("provided-lock-unusable", unusable.reader_message());
                return Ok(Self::default());
            }
        };
        let skills_dir = root.join(".mars").join("skills");
        let recorded: HashSet<String> = record
            .providers()
            .flat_map(|(_, provider)| provider.skills.iter().cloned())
            .collect();
        let installed = recorded
            .iter()
            .filter(|name| skills_dir.join(name).is_dir())
            .cloned()
            .collect();
        Ok(Self {
            recorded,
            installed,
        })
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

/// The provided skills a target sync projects, as outcomes with no `.mars` action: the copy in
/// `.mars/skills/` is `mars provide`'s, so nothing here installs, updates or removes it.
/// A skill in `taken` (a package or the project holds the same name) is left out, so one lock
/// key has one outcome. Sync and `link` both read this, which is why it is the one place that
/// turns the record into outcomes.
pub(crate) fn skill_outcomes(
    names: &ProvidedNames,
    mars_dir: &Path,
    taken: &HashSet<String>,
    diag: &mut DiagnosticCollector,
) -> Vec<ActionOutcome> {
    let mut installed: Vec<&String> = names
        .installed
        .iter()
        .filter(|name| !taken.contains(*name))
        .collect();
    installed.sort();
    let mut outcomes = Vec::new();
    for name in installed {
        let Some(provider) = provider_of(name) else {
            continue;
        };
        let hash =
            match crate::hash::compute_hash(&mars_dir.join("skills").join(name), ItemKind::Skill) {
                Ok(hash) => ContentHash::from(hash),
                Err(error) => {
                    diag.warn(
                        "provided-skill-unreadable",
                        format!("provided skill `{name}` is not copied to targets: {error}"),
                    );
                    continue;
                }
            };
        outcomes.push(ActionOutcome {
            item_id: ItemId {
                kind: ItemKind::Skill,
                name: name.as_str().into(),
            },
            action: ActionTaken::Skipped,
            dest_path: DestPath::from(format!("skills/{name}")),
            source_name: super::source::source_name(provider).into(),
            source_checksum: Some(hash.clone()),
            installed_checksum: Some(hash),
        });
    }
    outcomes
}
