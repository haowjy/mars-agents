//! The `hook.toml` definition and every per-target hook rule.
//!
//! A hook is written once as Claude's hooks JSON in TOML (`[hooks]`, see `portable`), a
//! native fragment per target (`[targets]`), or both. This module owns what is authored
//! (`HookDef`) and which targets receive it (`HookDef::select`); `read` turns text into a
//! `HookDef`, `output` says what it installs on one target, `targets` is the per-target
//! table and `render` turns a `[hooks]` table into a profile's entries.
//! Nothing here touches the filesystem: the caller reads the bytes and places the result,
//! and adds the location (a path, or a package and version) to the path-free diagnostics.

mod output;
pub mod portable;
mod read;
pub mod render;
pub mod targets;
#[cfg(test)]
mod tests;
mod typescript;

use std::collections::{BTreeMap, BTreeSet};

use crate::ordered::OrderedValue;
use crate::visibility::Visibility;
use portable::PortableHooks;
use render::Skip;

pub use output::{
    FileOutput, FragmentError, HookOutput, MergeOutput, OutputFile, file_fragment_problem,
};
pub use read::{HookRead, REMOVED_SCHEMA_CODE, REMOVED_SCHEMA_MESSAGE, read, uses_removed_schema};

/// The authored placeholder for the directory a hook's files are installed in.
pub const HOOK_DIR_TOKEN: &str = "${MARS_HOOK_DIR}";

/// Hook names ending in this are reserved for files Mars generates beside authored ones.
pub(crate) const RESERVED_NAME_SUFFIX: &str = ".hooks";

/// Where a `hook.toml` came from. The project's own is strict. A dependency's is lenient
/// where one author's slip should not stop a sync: an unknown top-level key is ignored and
/// a merge fragment's unknown event is skipped, each with a warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Local,
    Dependency,
}

/// The authored placeholder for the target a hook is installed for.
pub const HOOK_TARGET_TOKEN: &str = "${MARS_HOOK_TARGET}";

/// The values a hook's text is substituted with.
#[derive(Debug, Clone, Copy, Default)]
pub struct Tokens<'a> {
    /// Replaces [`HOOK_DIR_TOKEN`]: where the hook's directory is installed.
    pub hook_dir: &'a str,
    /// Replaces [`HOOK_TARGET_TOKEN`]: the target root (`.claude`, `.codex`, …), or `plugin`
    /// in a plugin build's shared tree.
    pub hook_target: &'a str,
    /// The hook's name, which a generated file carries as its identity. Not a token: no
    /// authored text is substituted with it.
    pub hook_name: &'a str,
}

impl Tokens<'_> {
    /// `text` with every token replaced.
    pub fn substitute(&self, text: &str) -> String {
        text.replace(HOOK_DIR_TOKEN, self.hook_dir)
            .replace(HOOK_TARGET_TOKEN, self.hook_target)
    }

    fn substitute_value(&self, value: &mut OrderedValue) {
        match value {
            OrderedValue::String(text) => *text = self.substitute(text),
            OrderedValue::Array(values) => values.iter_mut().for_each(|v| self.substitute_value(v)),
            OrderedValue::Object(values) => {
                values.values_mut().for_each(|v| self.substitute_value(v))
            }
            _ => {}
        }
    }
}

/// One target's fragment table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fragment {
    /// The fragment file in the hook's directory: the declared one, else `<target>.json`.
    pub file: String,
    unchecked: bool,
}

impl Fragment {
    /// `unchecked = true`: events the target does not list pass through verbatim.
    pub fn is_unchecked(&self) -> bool {
        self.unchecked
    }
}

/// A `hook.toml`.
#[derive(Debug, Clone)]
pub struct HookDef {
    pub name: String,
    pub visibility: Visibility,
    pub order: i32,
    targets: BTreeMap<String, Fragment>,
    portable: Option<PortableHooks>,
    skip_targets: BTreeSet<String>,
}

impl HookDef {
    /// The fragment table this hook declares for `target_root`.
    pub fn fragment(&self, target_root: &str) -> Option<&Fragment> {
        self.targets.get(target_root)
    }

    /// Every declared fragment table, by target root.
    pub fn fragments(&self) -> impl Iterator<Item = (&str, &Fragment)> {
        self.targets.iter().map(|(root, f)| (root.as_str(), f))
    }

    /// The `[hooks]` table, if the hook has one.
    pub fn portable(&self) -> Option<&PortableHooks> {
        self.portable.as_ref()
    }

    /// Whether `[hooks]` generates entries for `target_root`: it is not in `skip-targets`.
    fn generates_for(&self, target_root: &str) -> bool {
        self.portable.is_some() && !self.skip_targets.contains(target_root)
    }

    /// The managed target roots this hook is installed for: those where `[hooks]` renders at
    /// least one event (unless in `skip-targets`) and those with a fragment table. A fragment
    /// on a skipped target still installs; `skip-targets` only turns the generated entries
    /// off.
    pub fn select<'a>(&'a self, managed_targets: &'a [String]) -> Vec<&'a str> {
        managed_targets
            .iter()
            .map(String::as_str)
            .filter(|root| {
                self.targets.contains_key(*root)
                    || (self.generates_for(root)
                        && targets::target(root).is_some_and(|target| {
                            self.portable
                                .as_ref()
                                .is_some_and(|p| p.renders_on(&target.profile))
                        }))
            })
            .collect()
    }

    /// What `[hooks]` leaves out on `target_root` when nothing there overrides it; empty
    /// for a target in `skip-targets` and for a hook with no `[hooks]`.
    pub fn skips_for(&self, target_root: &str) -> Vec<Skip> {
        match (&self.portable, targets::target(target_root)) {
            (Some(portable), Some(target)) if self.generates_for(target_root) => {
                portable.render(&target.profile, &Tokens::default()).skips
            }
            _ => Vec::new(),
        }
    }
}
