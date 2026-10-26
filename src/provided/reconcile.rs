//! The reconcile loop: design steps 2-8 of `mars provide`.
//!
//! The fast path runs before the lock because hosts launch in parallel and during long syncs;
//! taking the lock first would turn every no-op into `busy` and a fallback read. It is safe
//! unlocked because `.mars/provided.lock` and `mars.lock` are only ever written by tmp + rename.
//!
//! `up-to-date` means "a re-plan would do nothing", so both passes decide it from the same
//! [`Plan`] over the files on disk: checking the record against itself would leave a skill that
//! was shadowed or collided on an earlier run uninstalled once its blocker is gone. Both passes
//! are one call to [`assess`], the second under the lock so another run's finished work is seen.
//!
//! Crash safety needs no journal: the record always covers every directory this run may write,
//! each directory install is atomic, and the next run replans from the files on disk. A crash
//! anywhere after the pre-record leaves `digest = "pending"`, which the next run can never read
//! as current, so a skill the host dropped meanwhile is still on record and still removed.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use crate::error::MarsError;
use crate::fs::FileLock;
use crate::lock::{CANONICAL_TARGET_ROOT, LockIndex};
use crate::types::DestPath;

use super::entries::{self, ProvidedEntries};
use super::lock::{PENDING_DIGEST, ProvidedLock, ProviderRecord};
use super::spec::ProvideSpec;
use super::{ProvideReport, Status};

/// Install one provider's skills into `root`'s canonical store.
pub(crate) fn reconcile(root: &Path, raw_spec: &str) -> Result<ProvideReport, MarsError> {
    let spec = ProvideSpec::parse(raw_spec)?;
    let (entries, mut warnings) = entries::discover(&spec)?;
    let skills_dir = root.join(".mars").join("skills");

    // Step 2: the fast path runs before the lock.
    if sync_pending(root) {
        return Ok(ProvideReport::without_record(
            Status::SyncPending,
            &spec,
            warnings,
        ));
    }
    let unlocked = assess(root, &spec)?;
    if matches_source(&unlocked.previous, &spec, &entries) {
        let plan = plan_against_disk(root, &entries, &unlocked.previous, &skills_dir)?;
        if plan.writes_nothing(&entries, &unlocked.previous, &spec) {
            return Ok(current_report(&spec, &unlocked.previous, &plan, warnings));
        }
    }

    // Step 3: never wait. A holder means someone else is already writing `.mars/`.
    let lock_path = root.join(".mars").join("sync.lock");
    let Some(_sync_lock) = FileLock::try_acquire(&lock_path)? else {
        return Ok(ProvideReport::without_record(Status::Busy, &spec, warnings));
    };

    // Step 4: read ownership under the lock; another run may have finished in between.
    if sync_pending(root) {
        return Ok(ProvideReport::without_record(
            Status::SyncPending,
            &spec,
            warnings,
        ));
    }
    if crate::platform::fs::recover_interrupted_skill_installs(&root.join(".mars"))? {
        warnings.push(
            "reclaimed an interrupted skill install left staged under .mars/skills".to_string(),
        );
    }
    let mut assessed = assess(root, &spec)?;
    warnings.extend(assessed.warning.take());
    // This pass is about to apply, so it always plans; the unlocked pass skipped the hash only when
    // the record was not this source's.
    let plan = plan_against_disk(root, &entries, &assessed.previous, &skills_dir)?;
    // A plan that writes nothing leaves both the record and the disk as they are, so a lingering
    // shadow or collision does not rewrite `provided.lock` on every launch.
    if plan.writes_nothing(&entries, &assessed.previous, &spec) {
        return Ok(current_report(&spec, &assessed.previous, &plan, warnings));
    }

    // Step 6: record every directory this run may write before writing any of them. `recorded` is
    // the old record plus everything about to be written: replaces and keeps are already recorded.
    let mut pre_record: BTreeSet<String> = assessed.previous.skills.iter().cloned().collect();
    pre_record.extend(plan.recorded(&entries));
    assessed.record.set_provider(
        &spec.provider,
        ProviderRecord {
            version: spec.version.clone(),
            digest: PENDING_DIGEST.to_string(),
            skills: pre_record.into_iter().collect(),
        },
    );
    assessed.record.save(root)?;

    // Step 7: whole-directory installs, then removals of names no package claims.
    for index in plan.installs.iter().chain(&plan.replaces) {
        let skill = &entries.skills[*index];
        crate::fs::atomic_install_dir(&skill.dir, &skills_dir.join(&skill.name))?;
    }
    for name in &plan.removals {
        crate::platform::fs::safe_remove(&skills_dir.join(name))?;
    }

    // Step 8.
    let skills = plan.recorded(&entries);
    assessed.record.set_provider(
        &spec.provider,
        ProviderRecord {
            version: spec.version.clone(),
            digest: entries.digest.clone(),
            skills: skills.clone(),
        },
    );
    assessed.record.save(root)?;

    Ok(ProvideReport {
        status: Status::Applied,
        provider: spec.provider.clone(),
        version: spec.version.clone(),
        skills,
        installed: names(&entries, plan.installs.iter().copied()),
        replaced: names(&entries, plan.replaces.iter().copied()),
        adopted: names(&entries, plan.adopts.iter().copied()),
        removed: plan.removals,
        shadowed: plan.shadowed,
        collisions: plan.collisions,
        warnings,
    })
}

/// One pass over the record and `mars.lock`: the record's previous entry for this provider is all
/// a run needs to decide whether it is up to date, and to apply if it is not.
struct Assessed {
    record: ProvidedLock,
    previous: ProviderRecord,
    /// The writer's warning for a corrupt record this pass starts from empty.
    warning: Option<String>,
}

/// Read the record and this provider's previous entry.
///
/// The unlocked pass discards `warning`: a run that writes nothing prints nothing (JSON keeps it
/// on the applied run that reports it).
fn assess(root: &Path, spec: &ProvideSpec) -> Result<Assessed, MarsError> {
    let (record, warning) = match ProvidedLock::load(root)? {
        Ok(record) => (record, None),
        Err(unusable) => unusable.for_write()?,
    };
    let previous = record.provider(&spec.provider).cloned().unwrap_or_default();
    Ok(Assessed {
        record,
        previous,
        warning,
    })
}

/// The record was written for exactly this source: same version, same digest.
///
/// [`PENDING_DIGEST`] never equals a computed digest, so a record left by a crashed run always
/// replans instead of being mistaken for current.
fn matches_source(
    previous: &ProviderRecord,
    spec: &ProvideSpec,
    entries: &ProvidedEntries,
) -> bool {
    previous.version == spec.version && previous.digest == entries.digest
}

/// What each skill's state on disk calls for, against `mars.lock`.
fn plan_against_disk(
    root: &Path,
    entries: &ProvidedEntries,
    previous: &ProviderRecord,
    skills_dir: &Path,
) -> Result<Plan, MarsError> {
    let package_lock = crate::lock::load(root)?;
    let claims = LockIndex::new(&package_lock);
    plan(entries, previous, &claims, skills_dir)
}

fn current_report(
    spec: &ProvideSpec,
    previous: &ProviderRecord,
    plan: &Plan,
    warnings: Vec<String>,
) -> ProvideReport {
    ProvideReport {
        skills: previous.skills.clone(),
        shadowed: plan.shadowed.clone(),
        collisions: plan.collisions.clone(),
        ..ProvideReport::without_record(Status::UpToDate, spec, warnings)
    }
}

fn sync_pending(root: &Path) -> bool {
    crate::sync::recovery::intent_path(root).exists()
}

fn claimed(claims: &LockIndex, name: &str) -> bool {
    claims.contains_installed_output(
        CANONICAL_TARGET_ROOT,
        &DestPath::from(format!("skills/{name}")),
    )
}

/// What to do with each provided skill, by the state of its destination.
///
/// Every decision here is a name Mars either owns or does not; nothing in the cycle deletes a
/// file Mars did not create, which is why an unrecorded directory is reported, never replaced.
/// Both the fast path and the planner read this one plan, so their answers cannot drift.
#[derive(Default)]
struct Plan {
    /// Indexes into [`ProvidedEntries::skills`].
    installs: Vec<usize>,
    replaces: Vec<usize>,
    adopts: Vec<usize>,
    keeps: Vec<usize>,
    /// Recorded names that leave the record and whose directories this run deletes.
    removals: Vec<String>,
    shadowed: Vec<String>,
    collisions: Vec<String>,
}

/// The names at `indexes`, in the order the planner placed them.
fn names(entries: &ProvidedEntries, indexes: impl Iterator<Item = usize>) -> Vec<String> {
    indexes
        .map(|index| entries.skills[index].name.clone())
        .collect()
}

impl Plan {
    /// True when applying this plan would write neither `.mars/skills/` nor `provided.lock`: no
    /// install, replace, adopt or removal, and the record it would leave equals the old one.
    ///
    /// This is the whole `up-to-date` condition on both paths.
    fn writes_nothing(
        &self,
        entries: &ProvidedEntries,
        previous: &ProviderRecord,
        spec: &ProvideSpec,
    ) -> bool {
        if !matches_source(previous, spec, entries) {
            return false;
        }
        let mut recorded = previous.skills.clone();
        recorded.sort();
        self.installs.is_empty()
            && self.replaces.is_empty()
            && self.adopts.is_empty()
            && self.removals.is_empty()
            && self.recorded(entries) == recorded
    }

    /// The names this provider's record lists after applying the plan.
    fn recorded(&self, entries: &ProvidedEntries) -> Vec<String> {
        let mut names = names(
            entries,
            self.installs
                .iter()
                .chain(&self.replaces)
                .chain(&self.adopts)
                .chain(&self.keeps)
                .copied(),
        );
        names.sort();
        names
    }
}

fn plan(
    entries: &ProvidedEntries,
    previous: &ProviderRecord,
    claims: &LockIndex,
    skills_dir: &Path,
) -> Result<Plan, MarsError> {
    let recorded = previous
        .skills
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut plan = Plan::default();
    for (index, skill) in entries.skills.iter().enumerate() {
        if claimed(claims, &skill.name) {
            plan.shadowed.push(skill.name.clone());
            continue;
        }
        let dest = skills_dir.join(&skill.name);
        match fs::symlink_metadata(&dest) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => plan.installs.push(index),
            Err(source) => {
                return Err(MarsError::Io {
                    operation: "inspect provided destination".to_string(),
                    path: dest,
                    source,
                });
            }
            Ok(metadata) => {
                // Only a real directory can be byte-equal: a file or symlink at the name is
                // something Mars did not install this way.
                let same = metadata.is_dir()
                    && entries::tree_digest(&dest).ok().as_deref() == Some(&skill.digest);
                if recorded.contains(skill.name.as_str()) {
                    if same {
                        plan.keeps.push(index);
                    } else {
                        plan.replaces.push(index);
                    }
                } else if same {
                    plan.adopts.push(index);
                } else {
                    plan.collisions.push(skill.name.clone());
                }
            }
        }
    }

    let owned = plan
        .installs
        .iter()
        .chain(&plan.replaces)
        .chain(&plan.adopts)
        .chain(&plan.keeps)
        .map(|index| entries.skills[*index].name.as_str())
        .collect::<BTreeSet<_>>();
    for name in &previous.skills {
        if owned.contains(name.as_str()) || claimed(claims, name) {
            continue;
        }
        plan.removals.push(name.clone());
    }
    Ok(plan)
}
