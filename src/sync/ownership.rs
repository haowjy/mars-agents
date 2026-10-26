//! Who owns a canonical destination under `.mars/`.
//!
//! Mars never overwrites a destination it did not create, so every sync decision about an
//! existing path asks one question. A destination is Mars-owned if `mars.lock` claims it, or if
//! `.mars/provided.lock` lists it: a provider's copy (`mars provide`) is Mars's own file, so a
//! package item that wants the same destination replaces it instead of colliding. The package
//! then wins on every machine, with or without the host, and the next `mars provide` sees the
//! claim in `mars.lock` and drops its record.
//!
//! Sync recovery asks a narrower question (was this journaled write already published to
//! `mars.lock`?) and keeps reading `mars.lock` alone.

use crate::lock::{CANONICAL_TARGET_ROOT, LockFile, LockIndex};
use crate::provided::ProvidedNames;
use crate::types::DestPath;

pub(crate) struct Ownership<'a> {
    lock: LockIndex<'a>,
    provided: &'a ProvidedNames,
}

impl<'a> Ownership<'a> {
    pub(crate) fn new(lock: &'a LockFile, provided: &'a ProvidedNames) -> Self {
        Self {
            lock: LockIndex::new(lock),
            provided,
        }
    }

    pub(crate) fn owns(&self, dest: &DestPath) -> bool {
        self.lock
            .contains_installed_output(CANONICAL_TARGET_ROOT, dest)
            || self.provided.owns(dest)
    }
}
