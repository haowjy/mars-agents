//! The one reconcile engine for named entries Mars owns in one section of a
//! config file the user may share.
//!
//! A user of the engine declares a [`KeyedSection`] (file and section shape)
//! and a [`KeyedKind`] (lock record key prefix and the words and diagnostic
//! codes of its messages). Ownership follows the lock: an entry still equal to
//! its record is Mars's; an edited one is kept unless forced; an unrecorded one
//! collides unless identical or forced. Hooks are not keyed entries: their
//! event-array semantics differ.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::config_doc::{Doc, SectionFormat};
use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::lock::ConfigEntryRecord;

/// Where a kind's entries live: a project-relative file and the section in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyedSection {
    pub path: &'static str,
    pub format: SectionFormat,
}

/// What differs between users of the engine. Diagnostic codes are
/// `<code>-divergent`, `<code>-unmanaged-adopted` and `<code>-unmanaged-collision`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyedKind {
    /// Lock record key prefix; the record key is `<prefix><name>`.
    pub key_prefix: &'static str,
    pub code: &'static str,
    /// Singular and plural nouns for messages, e.g. "MCP server" / "MCP servers".
    pub noun: &'static str,
    pub plural: &'static str,
}

impl KeyedKind {
    pub(crate) fn key(&self, name: &str) -> String {
        format!("{}{name}", self.key_prefix)
    }

    fn diagnostic(&self, suffix: &str) -> String {
        format!("{}-{suffix}", self.code)
    }
}

/// An entry Mars wants in the section.
#[derive(Debug, Clone)]
pub struct Rendered {
    pub name: String,
    pub value: Value,
}

impl Rendered {
    pub(crate) fn key(&self, kind: &KeyedKind) -> String {
        kind.key(&self.name)
    }

    pub(crate) fn record(&self) -> ConfigEntryRecord {
        ConfigEntryRecord {
            emitted_json: self.value.to_string(),
        }
    }
}

pub(crate) enum Ownership {
    Unowned,
    Owned,
    Edited,
}

pub(crate) fn ownership(record: Option<&ConfigEntryRecord>, current: &Value) -> Ownership {
    let Some(record) = record else {
        return Ownership::Unowned;
    };
    if serde_json::from_str::<Value>(&record.emitted_json).is_ok_and(|v| v == *current) {
        Ownership::Owned
    } else {
        Ownership::Edited
    }
}

pub(crate) fn validate(section: &KeyedSection, path: &Path) -> Result<(), MarsError> {
    Doc::load(path, section.format).map(|_| ())
}

/// Whether the file at `rel_path` (project-relative) holds nothing but entries Mars recorded
/// in `lock` and that still equal what it wrote. A user's entry, an edited one or an
/// unreadable file make it the user's.
pub(crate) fn holds_only_owned(
    project_root: &Path,
    rel_path: &str,
    lock: &crate::lock::LockFile,
) -> bool {
    let mut sections = Vec::new();
    for (target, records) in &lock.config_entries {
        let Some(spec) = super::spec(target) else {
            continue;
        };
        for (kind, section) in spec.keyed_sections() {
            if section.path == rel_path {
                sections.push((kind, section, records));
            }
        }
    }
    let Some((_, first, _)) = sections.first() else {
        return false;
    };
    let Ok(Some(mut doc)) = Doc::load(&project_root.join(rel_path), first.format) else {
        return false;
    };
    for (kind, _, records) in sections {
        for (key, record) in records {
            let Some(name) = key.strip_prefix(kind.key_prefix) else {
                continue;
            };
            let owned = doc.get(name).is_some_and(|current| {
                matches!(ownership(Some(record), &current), Ownership::Owned)
            });
            if owned {
                doc.remove(name);
            }
        }
    }
    doc.is_empty()
}

struct Open {
    path: PathBuf,
    doc: Doc,
    existed: bool,
    changed: bool,
}

impl Open {
    fn load(project_root: &Path, at: KeyedSection) -> Result<Self, MarsError> {
        let path = project_root.join(at.path);
        let loaded = Doc::load(&path, at.format)?;
        Ok(Self {
            path,
            existed: loaded.is_some(),
            doc: loaded.unwrap_or_else(|| Doc::new(at.format)),
            changed: false,
        })
    }

    fn remove(&mut self, name: &str) {
        self.changed |= self.doc.remove(name);
    }

    fn insert(&mut self, name: &str, value: Value) {
        self.doc.insert(name, value);
        self.changed = true;
    }
}

pub(crate) struct Reconciled {
    pub records: BTreeMap<String, ConfigEntryRecord>,
    pub removed: Vec<String>,
    /// Whether this reconcile wrote or deleted the file (or, in a dry run, would).
    pub changed: bool,
}

/// On failure nothing is retired: the result is `previous`'s records of this
/// kind, so the next sync retries.
///
/// A `dry_run` computes the same result and `changed` without saving or deleting the file.
#[allow(clippy::too_many_arguments)]
pub(crate) fn reconcile(
    kind: &KeyedKind,
    section: &KeyedSection,
    project_root: &Path,
    target: &str,
    previous: &BTreeMap<String, ConfigEntryRecord>,
    desired: &[Rendered],
    force: bool,
    dry_run: bool,
    diag: &mut DiagnosticCollector,
) -> Reconciled {
    let prior: BTreeMap<&str, &ConfigEntryRecord> = previous
        .iter()
        .filter_map(|(key, record)| Some((key.strip_prefix(kind.key_prefix)?, record)))
        .collect();
    match try_reconcile(
        kind,
        section,
        project_root,
        target,
        &prior,
        desired,
        force,
        dry_run,
        diag,
    ) {
        Ok(reconciled) => reconciled,
        Err(error) => {
            diag.warn(
                "config-entry-write",
                format!("failed to reconcile {} in `{target}`: {error}", kind.plural),
            );
            Reconciled {
                records: prior
                    .into_iter()
                    .map(|(name, record)| (kind.key(name), record.clone()))
                    .collect(),
                removed: Vec::new(),
                changed: false,
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn try_reconcile(
    kind: &KeyedKind,
    section: &KeyedSection,
    project_root: &Path,
    target: &str,
    prior: &BTreeMap<&str, &ConfigEntryRecord>,
    desired: &[Rendered],
    force: bool,
    dry_run: bool,
    diag: &mut DiagnosticCollector,
) -> Result<Reconciled, MarsError> {
    let noun = kind.noun;
    let mut open = Open::load(project_root, *section)?;
    let wanted: BTreeSet<&str> = desired.iter().map(|entry| entry.name.as_str()).collect();
    let mut kept = BTreeSet::new();

    for (&name, &record) in prior {
        if wanted.contains(name) {
            continue;
        }
        let Some(current) = open.doc.get(name) else {
            continue;
        };
        match ownership(Some(record), &current) {
            Ownership::Owned => open.remove(name),
            _ => {
                diag.warn(
                    kind.diagnostic("divergent"),
                    format!(
                        "target `{target}` {noun} `{name}` in `{}` was edited after mars wrote it (left in place; delete it by hand if unwanted)",
                        section.path
                    ),
                );
                kept.insert(name);
            }
        }
    }

    let mut records = BTreeMap::new();
    for entry in desired {
        let name = entry.name.as_str();
        let recorded = prior.get(name).copied();
        let existing = open.doc.get(name);
        let write = match &existing {
            Some(existing) if *existing == entry.value => true,
            Some(existing) => match ownership(recorded, existing) {
                Ownership::Owned => true,
                Ownership::Edited if force => true,
                Ownership::Edited => {
                    diag.warn(
                        kind.diagnostic("divergent"),
                        format!(
                            "target `{target}` {noun} `{name}` in `{}` was edited after mars wrote it (preserved; run `{}` to take it over)",
                            section.path,
                            crate::types::managed_cmd("mars sync --force")
                        ),
                    );
                    false
                }
                Ownership::Unowned if force => {
                    diag.warn(
                        kind.diagnostic("unmanaged-adopted"),
                        format!(
                            "target `{target}` {noun} `{name}` in `{}` was not owned by mars; adopting with `--force`",
                            section.path
                        ),
                    );
                    true
                }
                Ownership::Unowned => {
                    diag.warn(
                        kind.diagnostic("unmanaged-collision"),
                        format!(
                            "target `{target}` {noun} `{name}` in `{}` exists but mars does not own it (preserved; run `{}` to take it over)",
                            section.path,
                            crate::types::managed_cmd("mars sync --force")
                        ),
                    );
                    false
                }
            },
            None => true,
        };
        if write {
            if existing.as_ref() != Some(&entry.value) {
                open.insert(name, entry.value.clone());
            }
            records.insert(entry.key(kind), entry.record());
        } else if let Some(record) = recorded {
            records.insert(entry.key(kind), record.clone());
        }
    }

    // Only a file this reconcile changed can be deleted: one mars never touched
    // may be the user's, empty on purpose.
    let changed = open.changed;
    if changed && !dry_run {
        if open.existed && open.doc.is_empty() {
            std::fs::remove_file(&open.path)?;
        } else {
            open.doc.save(&open.path)?;
        }
    }

    let removed = prior
        .keys()
        .filter(|name| !wanted.contains(*name) && !kept.contains(*name))
        .map(|name| kind.key(name))
        .collect();
    Ok(Reconciled {
        records,
        removed,
        changed,
    })
}
