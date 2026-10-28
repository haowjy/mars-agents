//! Hook-entry removal outcomes and the operations derived from them.
//!
//! Hooks are unkeyed event-array elements, so a write cannot replace an entry
//! in place: every recorded hook entry of a target is removed first, then the
//! desired ones are appended. Removal and write operations carry their
//! authorized target and payload together; mutation boundaries derive paths and
//! records from those operations, so callers cannot provide an independent
//! target. MCP servers are keyed and replaced in place, and never come through
//! here (`target::mcp_file::reconcile`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::diagnostic::DiagnosticCollector;
use crate::lock::ConfigEntryRecord;
use crate::target::{HOOK_KEY_PREFIX, HookEntry};

/// The recorded hook entries of one target, all of which are removed before a write.
pub(crate) struct HookRemoval {
    pub prior_records: BTreeMap<String, ConfigEntryRecord>,
}

pub(crate) struct RemovalPlan {
    per_target: BTreeMap<String, HookRemoval>,
    stale_keys: BTreeMap<String, Vec<String>>,
}

impl RemovalPlan {
    /// Plan the removal of the hook records in `previous` (keyed by target root);
    /// other records are not this plan's.
    pub(crate) fn build(
        previous: &BTreeMap<String, BTreeMap<String, ConfigEntryRecord>>,
        desired: &BTreeMap<String, BTreeMap<String, ConfigEntryRecord>>,
    ) -> Self {
        let mut per_target = BTreeMap::new();
        let mut stale_keys: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (target_root, records) in previous {
            let prior_records: BTreeMap<_, _> = records
                .iter()
                .filter(|(key, _)| key.starts_with(HOOK_KEY_PREFIX))
                .map(|(key, record)| (key.clone(), record.clone()))
                .collect();
            if prior_records.is_empty() {
                continue;
            }
            let stale: Vec<_> = prior_records
                .keys()
                .filter(|key| {
                    !desired
                        .get(target_root)
                        .is_some_and(|entries| entries.contains_key(*key))
                })
                .cloned()
                .collect();
            if !stale.is_empty() {
                stale_keys.insert(target_root.clone(), stale);
            }
            per_target.insert(target_root.clone(), HookRemoval { prior_records });
        }
        Self {
            per_target,
            stale_keys,
        }
    }

    pub(crate) fn execute(
        self,
        mut remove: impl FnMut(RemovalOperation<'_>, &mut DiagnosticCollector) -> RemovalReport,
        diag: &mut DiagnosticCollector,
    ) -> RetentionPlan {
        let Self {
            per_target,
            stale_keys,
        } = self;
        let mut outcomes = BTreeMap::new();
        for (target_root, removal) in per_target {
            let operation = RemovalOperation {
                target_root: &target_root,
                removal: &removal,
            };
            let outcome = match remove(operation, diag) {
                RemovalReport::Confirmed => RemovalOutcome::Confirmed,
                RemovalReport::Unconfirmed { message, retained } => {
                    diag.warn("config-entry-remove", message);
                    RemovalOutcome::Unconfirmed { retained }
                }
            };
            outcomes.insert(target_root, outcome);
        }
        let removed_keys = stale_keys
            .into_iter()
            .filter(|(target_root, _)| {
                !matches!(
                    outcomes.get(target_root),
                    Some(RemovalOutcome::Unconfirmed { .. })
                )
            })
            .collect();
        RetentionPlan {
            outcomes,
            removed_keys,
        }
    }
}

pub struct RemovalOperation<'a> {
    target_root: &'a str,
    removal: &'a HookRemoval,
}

impl<'a> RemovalOperation<'a> {
    pub(crate) fn target_root(&self) -> &str {
        self.target_root
    }
    pub(crate) fn into_parts(self, project_root: &Path) -> (PathBuf, &'a HookRemoval) {
        (project_root.join(self.target_root), self.removal)
    }
}

pub enum RemovalReport {
    /// Nothing is left to retry.
    Confirmed,
    Unconfirmed {
        message: String,
        retained: BTreeMap<String, ConfigEntryRecord>,
    },
}

impl RemovalReport {
    pub(crate) fn confirmed() -> Self {
        Self::Confirmed
    }
    pub(crate) fn failed(
        error: impl std::fmt::Display,
        retained: BTreeMap<String, ConfigEntryRecord>,
    ) -> Self {
        Self::Unconfirmed {
            message: error.to_string(),
            retained,
        }
    }

    pub(crate) fn context(self, context: impl std::fmt::Display) -> Self {
        match self {
            Self::Confirmed => Self::Confirmed,
            Self::Unconfirmed { message, retained } => Self::Unconfirmed {
                message: format!("{context}: {message}"),
                retained,
            },
        }
    }

    #[cfg(test)]
    pub(crate) fn unwrap(self) {
        if let Self::Unconfirmed { message, .. } = self {
            panic!("removal was unconfirmed: {message}");
        }
    }
}

enum RemovalOutcome {
    Confirmed,
    Unconfirmed {
        retained: BTreeMap<String, ConfigEntryRecord>,
    },
}

pub(crate) struct RetentionPlan {
    outcomes: BTreeMap<String, RemovalOutcome>,
    removed_keys: BTreeMap<String, Vec<String>>,
}

impl RetentionPlan {
    /// The keys of recorded hooks that are gone and not wanted, by target root.
    pub(crate) fn removed_keys(&self) -> &BTreeMap<String, Vec<String>> {
        &self.removed_keys
    }

    pub(crate) fn write_permit<'p>(&'p self, target_root: &'p str) -> Option<WritePermit<'p>> {
        match self.outcomes.get(target_root) {
            Some(RemovalOutcome::Unconfirmed { .. }) => None,
            Some(RemovalOutcome::Confirmed) | None => Some(WritePermit { target_root }),
        }
    }

    pub(crate) fn into_retained_records(
        self,
    ) -> BTreeMap<String, BTreeMap<String, ConfigEntryRecord>> {
        self.outcomes
            .into_iter()
            .filter_map(|(target_root, outcome)| match outcome {
                RemovalOutcome::Unconfirmed { retained } => Some((target_root, retained)),
                RemovalOutcome::Confirmed => None,
            })
            .collect()
    }
}

pub struct WritePermit<'p> {
    target_root: &'p str,
}

impl<'p> WritePermit<'p> {
    pub(crate) fn bind_hooks(self, hooks: Vec<HookEntry>) -> ConfigWrite<'p> {
        ConfigWrite {
            target_root: self.target_root,
            hooks,
        }
    }

    pub(crate) fn bind_file_hooks(self, files: Vec<(String, String, String)>) -> FileHookWrite<'p> {
        FileHookWrite {
            target_root: self.target_root,
            files,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(target_root: &'static str) -> WritePermit<'static> {
        WritePermit { target_root }
    }
}

pub struct ConfigWrite<'p> {
    target_root: &'p str,
    hooks: Vec<HookEntry>,
}
impl<'p> ConfigWrite<'p> {
    pub(crate) fn target_root(&self) -> &str {
        self.target_root
    }
    pub(crate) fn hooks(&self) -> &[HookEntry] {
        &self.hooks
    }
    pub(crate) fn into_parts(self, project_root: &Path) -> (PathBuf, Vec<HookEntry>) {
        (project_root.join(self.target_root), self.hooks)
    }
}

pub(crate) struct FileHookWrite<'p> {
    target_root: &'p str,
    files: Vec<(String, String, String)>,
}
impl<'p> FileHookWrite<'p> {
    pub(crate) fn target_root(&self) -> &str {
        self.target_root
    }
    pub(crate) fn into_parts(
        self,
        project_root: &Path,
    ) -> (PathBuf, Vec<(String, String, String)>) {
        (project_root.join(self.target_root), self.files)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(value: Option<&str>) -> ConfigEntryRecord {
        ConfigEntryRecord {
            emitted_json: value.map(str::to_owned),
        }
    }

    const AUDIT: &str = "hook:SessionStart:audit";

    #[test]
    fn failure_retains_only_its_target_and_withholds_its_permit() {
        let previous = BTreeMap::from([
            (
                ".claude".to_owned(),
                BTreeMap::from([
                    ("mcp:old".to_owned(), record(None)),
                    (AUDIT.to_owned(), record(Some("[]"))),
                ]),
            ),
            (
                ".codex".to_owned(),
                BTreeMap::from([(AUDIT.to_owned(), record(Some("[]")))]),
            ),
        ]);
        let plan = RemovalPlan::build(&previous, &BTreeMap::new());
        let mut diag = DiagnosticCollector::new();
        let retention = plan.execute(
            |operation, _| {
                if operation.target_root() == ".claude" {
                    let (_, removal) = operation.into_parts(Path::new("."));
                    RemovalReport::failed("hook failed", removal.prior_records.clone())
                } else {
                    RemovalReport::confirmed()
                }
            },
            &mut diag,
        );

        assert!(retention.write_permit(".claude").is_none());
        assert!(retention.write_permit(".codex").is_some());
        let removed = retention.removed_keys().clone();
        assert_eq!(
            retention.into_retained_records(),
            BTreeMap::from([(
                ".claude".to_owned(),
                BTreeMap::from([(AUDIT.to_owned(), record(Some("[]")))])
            )])
        );
        assert_eq!(
            removed,
            BTreeMap::from([(".codex".to_owned(), vec![AUDIT.to_owned()])]),
            "only a confirmed target reports its stale hooks removed"
        );
    }

    #[test]
    fn mcp_records_are_not_in_the_plan() {
        let previous = BTreeMap::from([(
            ".claude".to_owned(),
            BTreeMap::from([("mcp:old".to_owned(), record(None))]),
        )]);
        let plan = RemovalPlan::build(&previous, &BTreeMap::new());
        let mut diag = DiagnosticCollector::new();
        let retention = plan.execute(|_, _| panic!("closure must not run"), &mut diag);
        assert!(retention.write_permit(".claude").is_some());
        assert!(retention.into_retained_records().is_empty());
    }

    #[test]
    fn absent_target_is_vacuously_confirmed() {
        let plan = RemovalPlan::build(&BTreeMap::new(), &BTreeMap::new());
        let mut diag = DiagnosticCollector::new();
        let retention = plan.execute(|_, _| unreachable!(), &mut diag);
        assert!(retention.write_permit(".claude").is_some());
    }
}
