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

use crate::layout::Layout;
use std::collections::{BTreeMap, HashSet};
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
    /// What the lock records as each host dependency's version, by provider: the digest of the
    /// hooks and MCP servers `mars provide` staged, or the host's own version for a bundle read
    /// in place (`sync --provided-root`). A provider with neither has no entry.
    versions: BTreeMap<String, String>,
}

impl ProvidedNames {
    pub(crate) fn load(layout: &Layout, diag: &mut DiagnosticCollector) -> Result<Self, MarsError> {
        let record = match ProvidedLock::load(layout)? {
            Ok(record) => record,
            Err(unusable) => {
                diag.warn_advisory("provided-lock-unusable", unusable.reader_message());
                return Ok(Self::default());
            }
        };
        let skills_dir = layout.store().join("skills");
        let recorded: HashSet<String> = record
            .providers()
            .flat_map(|(_, provider)| provider.skills.iter().cloned())
            .collect();
        let installed = recorded
            .iter()
            .filter(|name| skills_dir.join(name).is_dir())
            .cloned()
            .collect();
        let mut versions = BTreeMap::new();
        for (provider, _) in record.providers() {
            let dir = super::reconcile::staged_dir(layout, provider);
            if dir.is_dir() {
                versions.insert(provider.to_string(), super::entries::tree_digest(&dir)?);
            }
        }
        Ok(Self {
            recorded,
            installed,
            versions,
        })
    }

    /// What the lock records as a host dependency's version, so a sync that sees the host
    /// arrive or change has a record that moved, and one that changes nothing leaves it alone.
    pub(crate) fn host_version(&self, provider: &str) -> Option<&str> {
        self.versions.get(provider).map(String::as_str)
    }

    /// The versions of the hosts read from a bundle directory, which replace a staged digest.
    pub(crate) fn record_host_versions(&mut self, versions: BTreeMap<String, String>) {
        self.versions.extend(versions);
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

/// Drop from `.mars/provided.lock` every skill `lock` records an install of in `.mars/skills/`:
/// a package's or a host's bundle (`sync --provided-root`) replaced `mars provide`'s copy, and a
/// path has one owner. Called by sync under the sync lock, after it published `lock`. A record
/// this Mars cannot read is left as it is: it is not this build's to rewrite.
pub(crate) fn retire_claimed(
    layout: &Layout,
    lock: &crate::lock::LockFile,
) -> Result<(), MarsError> {
    let Ok(mut record) = ProvidedLock::load(layout)? else {
        return Ok(());
    };
    let claims = crate::lock::LockIndex::new(lock);
    let claimed = |name: &String| {
        claims.contains_installed_output(
            crate::lock::CANONICAL_TARGET_ROOT,
            &DestPath::from(format!("skills/{name}")),
        )
    };
    let mut changed = false;
    let providers: Vec<(String, super::lock::ProviderRecord)> = record
        .providers()
        .map(|(name, provider)| (name.to_string(), provider.clone()))
        .collect();
    for (name, mut provider) in providers {
        let before = provider.skills.len();
        provider.skills.retain(|skill| !claimed(skill));
        if provider.skills.len() != before {
            changed = true;
            record.set_provider(&name, provider);
        }
    }
    if changed {
        record.save(layout)?;
    }
    Ok(())
}

/// Every skill name `mars provide` recorded at `layout`, for launch lookup, which never serves
/// those copies. A record this Mars cannot read hides nothing: lookup then reads the lock alone.
pub(crate) fn recorded_skills(layout: &Layout) -> Vec<String> {
    match ProvidedLock::load(layout) {
        Ok(Ok(record)) => record
            .providers()
            .flat_map(|(_, provider)| provider.skills.iter().cloned())
            .collect(),
        _ => Vec::new(),
    }
}

/// The provided skills a target sync projects, as outcomes with no `.mars` action: the copy in
/// `.mars/skills/` is `mars provide`'s, so nothing here installs, updates or removes it.
/// A skill in `taken` (a package or the project holds the same name) yields to it, so one lock
/// key has one outcome. Sync and `link` both read this, which is why it is the one place that
/// turns the record into outcomes. With no managed target taking provided content there is
/// nothing to project and no skill is read.
pub(crate) fn skill_outcomes(
    names: &ProvidedNames,
    mars_dir: &Path,
    taken: &HashSet<String>,
    managed_targets: &[String],
    diag: &mut DiagnosticCollector,
) -> Vec<ActionOutcome> {
    if !super::takes_provided_any(managed_targets) {
        return Vec::new();
    }
    use crate::layer::Layer;
    // Every name in `taken` comes from a layer ranked before the host's.
    let yielded = crate::layer::yielded(
        taken
            .iter()
            .map(|name| (name.as_str(), Layer::Project))
            .chain(
                names
                    .installed
                    .iter()
                    .map(|name| (name.as_str(), Layer::Host)),
            ),
    );
    let mut installed: Vec<&String> = names
        .installed
        .iter()
        .filter(|name| !yielded.contains(&(name.as_str(), Layer::Host)))
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
