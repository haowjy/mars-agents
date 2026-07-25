//! Config-entry removal outcomes and the operations derived from them.
//!
//! Removal and write operations carry their authorized pair and payload together.
//! Mutation boundaries derive paths and records from those operations; callers
//! cannot provide an independent target or surface.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::diagnostic::DiagnosticCollector;
use crate::lock::ConfigEntryRecord;
use crate::target::ConfigEntry;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Surface {
    Mcp,
    Hook,
}

impl Surface {
    pub(crate) fn of_key(key: &str) -> Self {
        if key.starts_with("hook:") {
            Self::Hook
        } else {
            Self::Mcp
        }
    }
}

pub(crate) struct SurfaceRemoval {
    pub keys_to_remove: Vec<String>,
    pub prior_records: BTreeMap<String, ConfigEntryRecord>,
}

pub(crate) struct RemovalPlan {
    per_pair: BTreeMap<(String, Surface), SurfaceRemoval>,
    stale_keys: BTreeMap<String, Vec<String>>,
    preserved_records: BTreeMap<String, BTreeMap<String, ConfigEntryRecord>>,
}

impl RemovalPlan {
    pub(crate) fn build(
        previous: &BTreeMap<String, BTreeMap<String, ConfigEntryRecord>>,
        desired: &BTreeMap<String, BTreeMap<String, ConfigEntryRecord>>,
    ) -> Self {
        Self::build_preserving(previous, desired, &BTreeMap::new())
    }

    pub(crate) fn build_preserving(
        previous: &BTreeMap<String, BTreeMap<String, ConfigEntryRecord>>,
        desired: &BTreeMap<String, BTreeMap<String, ConfigEntryRecord>>,
        preserved_keys: &BTreeMap<String, std::collections::BTreeSet<String>>,
    ) -> Self {
        let mut per_pair: BTreeMap<(String, Surface), SurfaceRemoval> = BTreeMap::new();
        let mut stale_keys: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut preserved_records: BTreeMap<String, BTreeMap<String, ConfigEntryRecord>> =
            BTreeMap::new();
        for (target_root, records) in previous {
            for (key, record) in records {
                if preserved_keys
                    .get(target_root)
                    .is_some_and(|keys| keys.contains(key))
                {
                    preserved_records
                        .entry(target_root.clone())
                        .or_default()
                        .insert(key.clone(), record.clone());
                    continue;
                }
                let surface = Surface::of_key(key);
                per_pair
                    .entry((target_root.clone(), surface))
                    .or_insert_with(|| SurfaceRemoval {
                        keys_to_remove: Vec::new(),
                        prior_records: BTreeMap::new(),
                    })
                    .prior_records
                    .insert(key.clone(), record.clone());
                if desired
                    .get(target_root)
                    .is_none_or(|entries| !entries.contains_key(key))
                {
                    stale_keys
                        .entry(target_root.clone())
                        .or_default()
                        .push(key.clone());
                    if surface == Surface::Mcp {
                        per_pair
                            .get_mut(&(target_root.clone(), surface))
                            .expect("pair was inserted above")
                            .keys_to_remove
                            .push(key.clone());
                    }
                }
            }
        }
        for ((_, surface), removal) in &mut per_pair {
            if *surface == Surface::Hook {
                removal.keys_to_remove = removal.prior_records.keys().cloned().collect();
            }
        }
        Self {
            per_pair,
            stale_keys,
            preserved_records,
        }
    }

    pub(crate) fn stale_keys(&self) -> BTreeMap<String, Vec<String>> {
        self.stale_keys.clone()
    }

    pub(crate) fn execute(
        self,
        mut remove: impl FnMut(RemovalOperation<'_>, &mut DiagnosticCollector) -> RemovalReport,
        diag: &mut DiagnosticCollector,
    ) -> RetentionPlan {
        let Self {
            per_pair,
            stale_keys,
            preserved_records,
        } = self;
        let mut outcomes = BTreeMap::new();
        for ((target_root, surface), removal) in per_pair {
            let outcome = if removal.keys_to_remove.is_empty() {
                RemovalOutcome::Confirmed
            } else {
                let operation = RemovalOperation {
                    target_root: &target_root,
                    surface,
                    removal: &removal,
                };
                match remove(operation, diag) {
                    RemovalReport::Confirmed => RemovalOutcome::Confirmed,
                    RemovalReport::Unconfirmed { message, retained } => {
                        diag.warn("config-entry-remove", message);
                        RemovalOutcome::Unconfirmed { retained }
                    }
                }
            };
            outcomes.insert((target_root, surface), outcome);
        }
        for (target_root, keys) in stale_keys {
            let removed_keys: Vec<_> = keys
                .into_iter()
                .filter(|key| {
                    !matches!(
                        outcomes.get(&(target_root.clone(), Surface::of_key(key))),
                        Some(RemovalOutcome::Unconfirmed { .. })
                    )
                })
                .collect();
            if !removed_keys.is_empty() {
                diag.info(
                    "stale-config-entry",
                    format!(
                        "removed stale config entries from `{target_root}`: {}",
                        removed_keys.join(", ")
                    ),
                );
            }
        }
        RetentionPlan {
            outcomes,
            preserved_records,
        }
    }
}

pub struct RemovalOperation<'a> {
    target_root: &'a str,
    surface: Surface,
    removal: &'a SurfaceRemoval,
}

impl<'a> RemovalOperation<'a> {
    pub(crate) fn target_root(&self) -> &str {
        self.target_root
    }
    pub(crate) fn surface(&self) -> Surface {
        self.surface
    }
    pub(crate) fn into_parts(self, project_root: &Path) -> (PathBuf, &'a SurfaceRemoval) {
        (project_root.join(self.target_root), self.removal)
    }
}

pub enum RemovalReport {
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
    outcomes: BTreeMap<(String, Surface), RemovalOutcome>,
    preserved_records: BTreeMap<String, BTreeMap<String, ConfigEntryRecord>>,
}

impl RetentionPlan {
    pub(crate) fn write_permit<'p>(
        &'p self,
        target_root: &'p str,
        surface: Surface,
    ) -> Option<WritePermit<'p>> {
        match self.outcomes.get(&(target_root.to_string(), surface)) {
            Some(RemovalOutcome::Unconfirmed { .. }) => None,
            Some(RemovalOutcome::Confirmed) | None => Some(WritePermit {
                target_root,
                surface,
            }),
        }
    }

    pub(crate) fn into_retained_records(
        self,
    ) -> BTreeMap<String, BTreeMap<String, ConfigEntryRecord>> {
        let mut records = self.preserved_records;
        for ((target_root, _), outcome) in self.outcomes {
            if let RemovalOutcome::Unconfirmed { retained } = outcome {
                records.entry(target_root).or_default().extend(retained);
            }
        }
        records
    }
}

pub struct WritePermit<'p> {
    target_root: &'p str,
    surface: Surface,
}

#[derive(Debug)]
pub struct SurfaceMismatch;

impl std::fmt::Display for SurfaceMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("write payload does not match permit surface")
    }
}

impl<'p> WritePermit<'p> {
    pub fn bind_config_entries(
        self,
        entries: Vec<ConfigEntry>,
    ) -> Result<ConfigWrite<'p>, SurfaceMismatch> {
        if entries.iter().all(|entry| entry.surface() == self.surface) {
            Ok(ConfigWrite {
                target_root: self.target_root,
                entries,
            })
        } else {
            Err(SurfaceMismatch)
        }
    }

    pub(crate) fn bind_file_hooks(
        self,
        files: Vec<(String, String, String)>,
    ) -> Result<FileHookWrite<'p>, SurfaceMismatch> {
        if self.surface == Surface::Hook {
            Ok(FileHookWrite {
                target_root: self.target_root,
                files,
            })
        } else {
            Err(SurfaceMismatch)
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(target_root: &'static str, surface: Surface) -> WritePermit<'static> {
        WritePermit {
            target_root,
            surface,
        }
    }
}

pub struct ConfigWrite<'p> {
    target_root: &'p str,
    entries: Vec<ConfigEntry>,
}
impl<'p> ConfigWrite<'p> {
    pub(crate) fn target_root(&self) -> &str {
        self.target_root
    }
    pub(crate) fn entries(&self) -> &[ConfigEntry] {
        &self.entries
    }
    pub(crate) fn into_parts(self, project_root: &Path) -> (PathBuf, Vec<ConfigEntry>) {
        (project_root.join(self.target_root), self.entries)
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

    #[test]
    fn empty_removal_is_confirmed_without_invoking_closure() {
        let previous = BTreeMap::from([(
            ".claude".to_owned(),
            BTreeMap::from([("mcp:kept".to_owned(), record(None))]),
        )]);
        let desired = previous.clone();
        let plan = RemovalPlan::build(&previous, &desired);
        let mut diag = DiagnosticCollector::new();
        let retention = plan.execute(|_, _| panic!("closure must not run"), &mut diag);

        assert!(retention.write_permit(".claude", Surface::Mcp).is_some());
        assert!(retention.into_retained_records().is_empty());
    }

    #[test]
    fn failure_retains_only_its_surface_and_withholds_its_permit() {
        let previous = BTreeMap::from([(
            ".claude".to_owned(),
            BTreeMap::from([
                ("mcp:old".to_owned(), record(None)),
                ("hook:SessionStart:audit".to_owned(), record(Some("[]"))),
            ]),
        )]);
        let plan = RemovalPlan::build(&previous, &BTreeMap::new());
        let mut diag = DiagnosticCollector::new();
        let retention = plan.execute(
            |operation, _| match operation.surface() {
                Surface::Mcp => {
                    let (_, removal) = operation.into_parts(Path::new("."));
                    RemovalReport::failed("mcp failed", removal.prior_records.clone())
                }
                Surface::Hook => RemovalReport::confirmed(),
            },
            &mut diag,
        );

        assert!(retention.write_permit(".claude", Surface::Mcp).is_none());
        assert!(retention.write_permit(".claude", Surface::Hook).is_some());
        assert_eq!(
            retention.into_retained_records(),
            BTreeMap::from([(
                ".claude".to_owned(),
                BTreeMap::from([("mcp:old".to_owned(), record(None))])
            )])
        );
        let diagnostics = diag.drain();
        let stale = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "stale-config-entry")
            .expect("successful hook removal should be reported");
        assert!(stale.message.contains("hook:SessionStart:audit"));
        assert!(!stale.message.contains("mcp:old"));
    }

    #[test]
    fn failed_hook_removal_is_not_reported_as_removed() {
        let previous = BTreeMap::from([(
            ".claude".to_owned(),
            BTreeMap::from([("hook:SessionStart:audit".to_owned(), record(Some("[]")))]),
        )]);
        let plan = RemovalPlan::build(&previous, &BTreeMap::new());
        let mut diag = DiagnosticCollector::new();
        let retention = plan.execute(
            |operation, _| {
                let (_, removal) = operation.into_parts(Path::new("."));
                RemovalReport::failed("hook failed", removal.prior_records.clone())
            },
            &mut diag,
        );

        assert!(
            diag.drain()
                .iter()
                .all(|diagnostic| diagnostic.code != "stale-config-entry")
        );
        assert_eq!(
            retention.into_retained_records(),
            BTreeMap::from([(
                ".claude".to_owned(),
                BTreeMap::from([("hook:SessionStart:audit".to_owned(), record(Some("[]")))])
            )])
        );
    }

    #[test]
    fn absent_pair_is_vacuously_confirmed() {
        let plan = RemovalPlan::build(&BTreeMap::new(), &BTreeMap::new());
        let mut diag = DiagnosticCollector::new();
        let retention = plan.execute(|_, _| unreachable!(), &mut diag);
        assert!(retention.write_permit(".claude", Surface::Hook).is_some());
    }

    #[test]
    fn stale_keys_match_previous_minus_desired_across_surfaces() {
        let previous = BTreeMap::from([(
            ".claude".to_owned(),
            BTreeMap::from([
                ("hook:old:audit".to_owned(), record(Some("[]"))),
                ("mcp:kept".to_owned(), record(None)),
                ("mcp:old".to_owned(), record(None)),
            ]),
        )]);
        let desired = BTreeMap::from([(
            ".claude".to_owned(),
            BTreeMap::from([("mcp:kept".to_owned(), record(None))]),
        )]);
        assert_eq!(
            RemovalPlan::build(&previous, &desired).stale_keys()[".claude"],
            ["hook:old:audit", "mcp:old"]
        );
    }

    #[test]
    fn classifies_only_hook_prefix_as_hook() {
        assert_eq!(Surface::of_key("hook:Start:audit"), Surface::Hook);
        assert_eq!(Surface::of_key("mcp:server"), Surface::Mcp);
        assert_eq!(Surface::of_key("future:key"), Surface::Mcp);
    }

    #[test]
    fn binding_rejects_entries_from_another_surface() {
        let entry = ConfigEntry::McpServer(crate::target::McpServerEntry {
            name: "server".to_owned(),
            command: "server".to_owned(),
            args: Vec::new(),
            env: indexmap::IndexMap::new(),
        });
        let result =
            WritePermit::for_test(".opencode", Surface::Hook).bind_config_entries(vec![entry]);
        assert!(result.is_err());
    }

    #[test]
    fn fault_matrix_derives_retention_and_permits_from_each_pair_outcome() {
        for failed_target in [".claude", ".codex", ".cursor", ".opencode"] {
            for failed_surface in [Surface::Mcp, Surface::Hook] {
                let mut previous = BTreeMap::new();
                for target in [".claude", ".codex", ".cursor", ".opencode"] {
                    previous.insert(
                        target.to_owned(),
                        BTreeMap::from([
                            ("mcp:old".to_owned(), record(None)),
                            ("hook:Start:audit".to_owned(), record(Some("[]"))),
                        ]),
                    );
                }
                let plan = RemovalPlan::build(&previous, &BTreeMap::new());
                let mut diag = DiagnosticCollector::new();
                let retention = plan.execute(
                    |operation, _| {
                        let failed = operation.target_root() == failed_target
                            && operation.surface() == failed_surface;
                        let (_, removal) = operation.into_parts(Path::new("."));
                        if failed {
                            RemovalReport::failed(
                                "injected removal failure",
                                removal.prior_records.clone(),
                            )
                        } else {
                            RemovalReport::confirmed()
                        }
                    },
                    &mut diag,
                );

                // Reference outcome table: exactly the injected pair is
                // unconfirmed; all adjacent successful pairs are confirmed.
                for target in [".claude", ".codex", ".cursor", ".opencode"] {
                    for surface in [Surface::Mcp, Surface::Hook] {
                        let unconfirmed = target == failed_target && surface == failed_surface;
                        assert_eq!(
                            retention.write_permit(target, surface).is_none(),
                            unconfirmed,
                            "permit disagrees with outcome table for ({target}, {surface:?})"
                        );
                    }
                }
                let retained = retention.into_retained_records();
                let expected_key = match failed_surface {
                    Surface::Mcp => "mcp:old",
                    Surface::Hook => "hook:Start:audit",
                };
                assert_eq!(
                    retained,
                    BTreeMap::from([(
                        failed_target.to_owned(),
                        BTreeMap::from([(
                            expected_key.to_owned(),
                            previous[failed_target][expected_key].clone(),
                        )]),
                    )])
                );
            }
        }
    }
}
