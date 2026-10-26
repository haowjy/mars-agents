//! The reconcile loop: design steps 2-8 of `mars provide`.
//!
//! The fast path runs before the lock because hosts launch in parallel and during long syncs;
//! taking the lock first would turn every no-op into `busy` and a fallback read. It is safe
//! unlocked because `.mars/provided.lock` and `mars.lock` are only ever written by tmp + rename.
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

pub(crate) fn reconcile(root: &Path, spec: &ProvideSpec) -> Result<ProvideReport, MarsError> {
    let (entries, mut warnings) = entries::discover(spec)?;
    let skills_dir = root.join(".mars").join("skills");

    // Step 2: the fast path runs before the lock.
    if sync_pending(root) {
        return Ok(ProvideReport::without_record(
            Status::SyncPending,
            spec,
            warnings,
        ));
    }
    if let Some(current) = current_record(spec, root, &entries, &skills_dir)? {
        return Ok(ProvideReport::current(spec, &current, warnings));
    }

    // Step 3: never wait. A holder means someone else is already writing `.mars/`.
    let lock_path = root.join(".mars").join("sync.lock");
    let Some(_sync_lock) = FileLock::try_acquire(&lock_path)? else {
        return Ok(ProvideReport::without_record(Status::Busy, spec, warnings));
    };

    // Step 4: read ownership under the lock; another run may have finished in between.
    if sync_pending(root) {
        return Ok(ProvideReport::without_record(
            Status::SyncPending,
            spec,
            warnings,
        ));
    }
    let loaded = ProvidedLock::load(root)?;
    warnings.extend(loaded.warnings);
    let mut record = loaded.record;
    let package_lock = crate::lock::load(root)?;
    let claims = LockIndex::new(&package_lock);
    if let Some(current) = current_record_in(spec, &record, &claims, &entries, &skills_dir) {
        return Ok(ProvideReport::current(spec, &current, warnings));
    }
    let previous = record
        .provider(&spec.provider)
        .cloned()
        .unwrap_or_else(|| ProviderRecord {
            version: String::new(),
            digest: String::new(),
            skills: Vec::new(),
        });

    // Step 5: what each name's state on disk calls for.
    let plan = plan(&entries, &previous, &claims, &skills_dir)?;
    warnings.append(&mut plan.warnings());

    // Step 6: record every directory this run may write before writing any of them.
    let mut pre_record = previous.skills.iter().cloned().collect::<BTreeSet<_>>();
    pre_record.extend(names(
        &entries,
        plan.installs.iter().chain(&plan.adopts).copied(),
    ));
    record.set_provider(
        &spec.provider,
        ProviderRecord {
            version: spec.version.clone(),
            digest: PENDING_DIGEST.to_string(),
            skills: pre_record.into_iter().collect(),
        },
    );
    record.save(root)?;

    // Step 7: whole-directory installs, then removals of names no package claims.
    for index in plan.installs.iter().chain(&plan.replaces) {
        let skill = &entries.skills[*index];
        crate::fs::atomic_install_dir(&skill.dir, &skills_dir.join(&skill.name))?;
    }
    for name in &plan.removals {
        crate::platform::fs::safe_remove(&skills_dir.join(name))?;
    }

    // Step 8.
    let recorded = plan
        .installs
        .iter()
        .chain(&plan.replaces)
        .chain(&plan.adopts)
        .chain(&plan.keeps)
        .collect::<BTreeSet<_>>();
    let skills = names(&entries, recorded.into_iter().copied());
    record.set_provider(
        &spec.provider,
        ProviderRecord {
            version: spec.version.clone(),
            digest: entries.digest.clone(),
            skills: skills.clone(),
        },
    );
    record.save(root)?;

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

/// The record for this provider when it already describes what is on disk.
///
/// Reaching this without the lock is safe: both lock files are published by rename, so this sees
/// one consistent file or the other, never a torn one.
fn current_record(
    spec: &ProvideSpec,
    root: &Path,
    entries: &ProvidedEntries,
    skills_dir: &Path,
) -> Result<Option<ProviderRecord>, MarsError> {
    let loaded = ProvidedLock::load(root)?;
    let package_lock = crate::lock::load(root)?;
    let claims = LockIndex::new(&package_lock);
    Ok(current_record_in(
        spec,
        &loaded.record,
        &claims,
        entries,
        skills_dir,
    ))
}

fn current_record_in(
    spec: &ProvideSpec,
    record: &ProvidedLock,
    claims: &LockIndex,
    entries: &ProvidedEntries,
    skills_dir: &Path,
) -> Option<ProviderRecord> {
    let provider = record.provider(&spec.provider)?;
    if provider.version != spec.version
        || provider.digest == PENDING_DIGEST
        || provider.digest != entries.digest
    {
        return None;
    }
    if !provider
        .skills
        .iter()
        .all(|name| skills_dir.join(name).is_dir())
    {
        return None;
    }
    if provider.skills.iter().any(|name| claimed(claims, name)) {
        return None;
    }
    Some(provider.clone())
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
    fn warnings(&self) -> Vec<String> {
        let shadowed = self.shadowed.iter().map(|name| {
            format!(
                "provided skill `{name}` is owned by a package in `mars.lock`; leaving it to the package"
            )
        });
        let collisions = self.collisions.iter().map(|name| {
            format!(
                "`.mars/skills/{name}` exists, is not recorded as provided or installed by a package, and differs from the provided copy; left untouched"
            )
        });
        shadowed.chain(collisions).collect()
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
                let same =
                    metadata.is_dir() && tree_digest(&dest).as_deref() == Some(&skill.digest);
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

/// The digest of an authored directory, or `None` when it is not a regular tree.
///
/// A destination that cannot be read as regular files is never byte-equal to the source, so it
/// is a collision rather than an error: `mars provide` must not fail because of a file it does
/// not own.
fn tree_digest(dir: &Path) -> Option<String> {
    let files = crate::hash::strict_file_hashes(dir, &[]).ok()?;
    Some(crate::hash::hash_file_manifest(files.into_iter().collect()))
}
