//! Config-entry removal outcomes and the permits derived from them.
//!
//! A removal outcome is recorded exactly once for each target/surface pair. Prior
//! ownership can only leave this module through an unconfirmed outcome, and a
//! replacement write can only proceed with a permit issued for a confirmed one.

use std::collections::BTreeMap;
use std::marker::PhantomData;

use crate::diagnostic::DiagnosticCollector;
use crate::lock::ConfigEntryRecord;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Surface {
    Mcp,
    Hook,
}

impl Surface {
    /// Total classification of a lock config-entry key.
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
}

impl RemovalPlan {
    pub(crate) fn build(
        previous: &BTreeMap<String, BTreeMap<String, ConfigEntryRecord>>,
        desired: &BTreeMap<String, BTreeMap<String, ConfigEntryRecord>>,
    ) -> Self {
        let mut per_pair: BTreeMap<(String, Surface), SurfaceRemoval> = BTreeMap::new();
        let mut stale_keys: BTreeMap<String, Vec<String>> = BTreeMap::new();

        for (target_root, records) in previous {
            for (key, record) in records {
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

        // Hook removal is sweep-then-rewrite, so all prior hook keys are work.
        for ((_, surface), removal) in &mut per_pair {
            if *surface == Surface::Hook {
                removal.keys_to_remove = removal.prior_records.keys().cloned().collect();
            }
        }

        Self {
            per_pair,
            stale_keys,
        }
    }

    pub(crate) fn stale_keys(&self) -> BTreeMap<String, Vec<String>> {
        self.stale_keys.clone()
    }

    pub(crate) fn execute(
        self,
        mut remove: impl FnMut(
            RemovalToken<'_>,
            &str,
            Surface,
            &SurfaceRemoval,
            &mut DiagnosticCollector,
        ) -> Result<(), String>,
        diag: &mut DiagnosticCollector,
    ) -> RetentionPlan {
        let mut outcomes = BTreeMap::new();
        for ((target_root, surface), removal) in self.per_pair {
            let outcome = if removal.keys_to_remove.is_empty() {
                RemovalOutcome::Confirmed
            } else {
                match remove(RemovalToken::new(), &target_root, surface, &removal, diag) {
                    Ok(()) => {
                        if surface == Surface::Mcp {
                            diag.info(
                                "stale-config-entry",
                                format!(
                                    "removed stale config entries from `{target_root}`: {}",
                                    removal.keys_to_remove.join(", ")
                                ),
                            );
                        }
                        RemovalOutcome::Confirmed
                    }
                    Err(message) => {
                        diag.warn("config-entry-remove", message);
                        RemovalOutcome::Unconfirmed {
                            retained: removal.prior_records,
                        }
                    }
                }
            };
            outcomes.insert((target_root, surface), outcome);
        }
        RetentionPlan { outcomes }
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
                _sealed: (),
            }),
        }
    }

    pub(crate) fn into_retained_records(
        self,
    ) -> BTreeMap<String, BTreeMap<String, ConfigEntryRecord>> {
        let mut records: BTreeMap<String, BTreeMap<String, ConfigEntryRecord>> = BTreeMap::new();
        for ((target_root, _), outcome) in self.outcomes {
            if let RemovalOutcome::Unconfirmed { retained } = outcome {
                records.entry(target_root).or_default().extend(retained);
            }
        }
        records
    }
}

#[derive(Clone, Copy)]
pub struct WritePermit<'p> {
    target_root: &'p str,
    surface: Surface,
    _sealed: (),
}

#[allow(dead_code)]
impl<'p> WritePermit<'p> {
    pub(crate) fn target_root(&self) -> &str {
        self.target_root
    }

    pub(crate) fn surface(&self) -> Surface {
        self.surface
    }

    #[cfg(test)]
    pub(crate) fn for_test(target_root: &'static str, surface: Surface) -> WritePermit<'static> {
        WritePermit {
            target_root,
            surface,
            _sealed: (),
        }
    }
}

#[derive(Clone, Copy)]
pub struct RemovalToken<'a> {
    _sealed: PhantomData<&'a ()>,
}

impl RemovalToken<'_> {
    fn new() -> Self {
        Self {
            _sealed: PhantomData,
        }
    }
}

#[cfg(test)]
impl RemovalToken<'static> {
    pub(crate) fn for_test() -> Self {
        Self {
            _sealed: PhantomData,
        }
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
        let retention = plan.execute(|_, _, _, _, _| panic!("closure must not run"), &mut diag);

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
            |_, _, surface, _, _| match surface {
                Surface::Mcp => Err("mcp failed".to_owned()),
                Surface::Hook => Ok(()),
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
    }

    #[test]
    fn absent_pair_is_vacuously_confirmed() {
        let plan = RemovalPlan::build(&BTreeMap::new(), &BTreeMap::new());
        let mut diag = DiagnosticCollector::new();
        let retention = plan.execute(|_, _, _, _, _| unreachable!(), &mut diag);
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
}
