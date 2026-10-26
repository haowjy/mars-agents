//! `mars provide` — a host installs the skills it ships into `.mars/skills/`.
//!
//! A host ships skills beside its binary and hands Mars the directory holding them:
//! `mars provide meridian@0.15.0=<package dir>`. Mars copies the skills into the canonical store
//! and records their names in `.mars/provided.lock`, which only this command writes.
//!
//! The rule behind every edge case here: **a provider never changes committed state.** Nothing
//! in this module writes `mars.toml`, `mars.lock` or a harness target, so a project stays
//! byte-identical with or without a host until someone runs `mars sync`. Provided skills are
//! never projected into targets either, since projection reads only `mars.lock` records.
//!
//! ```text
//! cli/provide.rs ─→ run
//!                    ├ spec::parse                            `<provider>@<version>=<dir>`
//!                    ├ entries::discover                      names, digests, validation
//!                    ├ fs::FileLock::try_acquire(.mars/sync.lock)
//!                    ├ lock::{ProvidedLock::load, save}       tmp + rename
//!                    ├ reconcile::reconcile                   design steps 2-8
//!                    └ fs::atomic_install_dir, platform::fs::safe_remove
//! ```

mod entries;
mod lock;
mod names;
mod reconcile;
mod spec;

use std::path::Path;

use serde::Serialize;

use crate::error::MarsError;
pub(crate) use names::ProvidedNames;
use spec::ProvideSpec;

/// What one `mars provide` run did, or why it wrote nothing.
#[derive(Debug, Serialize)]
pub(crate) struct ProvideReport {
    pub status: Status,
    pub provider: String,
    pub version: String,
    /// The provider's recorded names after the run. Nothing was recorded for `busy` and
    /// `sync-pending`, which write nothing.
    pub skills: Vec<String>,
    pub installed: Vec<String>,
    pub replaced: Vec<String>,
    pub adopted: Vec<String>,
    pub removed: Vec<String>,
    /// Provided skills a package owns in `mars.lock`; left to the package. Reported on every run
    /// while the claim lasts, on `up-to-date` as well as `applied`.
    pub shadowed: Vec<String>,
    /// Destinations Mars did not create and cannot adopt; left untouched. Reported on every run
    /// while the directory stays in the way.
    pub collisions: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Status {
    /// A re-plan would write nothing: the record already describes what is on disk.
    UpToDate,
    /// The record was rewritten to match the source.
    Applied,
    /// Another Mars process holds the sync lock; nothing was written.
    Busy,
    /// An interrupted sync owns canonical destinations this run would have to reconcile
    /// against; nothing was written until a sync completes recovery.
    SyncPending,
}

impl ProvideReport {
    /// A run that wrote nothing: no recorded names to report, and no per-name actions.
    fn without_record(status: Status, spec: &ProvideSpec, warnings: Vec<String>) -> Self {
        Self {
            status,
            provider: spec.provider.clone(),
            version: spec.version.clone(),
            skills: Vec::new(),
            installed: Vec::new(),
            replaced: Vec::new(),
            adopted: Vec::new(),
            removed: Vec::new(),
            shadowed: Vec::new(),
            collisions: Vec::new(),
            warnings,
        }
    }
}

/// Install one provider's skills into `root`'s canonical store.
pub(crate) fn run(root: &Path, raw_spec: &str) -> Result<ProvideReport, MarsError> {
    let spec = ProvideSpec::parse(raw_spec)?;
    reconcile::reconcile(root, &spec)
}
