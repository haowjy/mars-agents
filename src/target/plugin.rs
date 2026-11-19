//! A target that is a plugin directory the harness loads through a pointer file: its entries
//! live in files the user may also edit, so Mars owns them by key (`keyed_entries`).

use serde_json::json;

use super::keyed_entries::{KeyedKind, KeyedSection, Rendered};

/// The plugin's entry in the project's plugin list, keyed by its `path`.
pub(crate) const POINTER_KIND: KeyedKind = KeyedKind {
    key_prefix: "pointer:",
    code: "plugin-pointer",
    noun: "plugin entry",
    plural: "plugin entries",
};

/// The keys of the plugin's manifest that Mars owns.
pub(crate) const MANIFEST_KIND: KeyedKind = KeyedKind {
    key_prefix: "manifest:",
    code: "plugin-manifest",
    noun: "plugin manifest key",
    plural: "plugin manifest keys",
};

/// A hook, named after the hook, in the plugin's hook file.
pub(crate) const HOOK_ENTRY_KIND: KeyedKind = KeyedKind {
    key_prefix: "hook-entry:",
    code: "hook-entry",
    noun: "hook",
    plural: "hooks",
};

/// The manifest `agy plugin validate --strict` accepts: a plugin with no `version` is flagged
/// as unlistable in the marketplace.
const MANIFEST_NAME: &str = "mars";
const MANIFEST_VERSION: &str = "1.0.0";

/// Where a plugin target keeps the files Mars edits by key.
#[derive(Debug, Clone, Copy)]
pub struct PluginTarget {
    /// The project's plugin list: an array whose elements name a plugin directory by `path`.
    pub pointer: KeyedSection,
    pub manifest: KeyedSection,
    /// Where `hooks::targets` places this target's keyed hooks.
    pub hooks: KeyedSection,
}

impl PluginTarget {
    pub fn sections(&self) -> [(KeyedKind, KeyedSection); 3] {
        [
            (POINTER_KIND, self.pointer),
            (MANIFEST_KIND, self.manifest),
            (HOOK_ENTRY_KIND, self.hooks),
        ]
    }

    /// What a managed plugin target always has: its pointer and the manifest keys Mars owns.
    pub fn fixed_entries(&self, target: &str) -> Vec<(KeyedKind, Rendered)> {
        let entry = |kind, name: &str, value| {
            (
                kind,
                Rendered {
                    name: name.to_owned(),
                    value,
                },
            )
        };
        vec![
            entry(POINTER_KIND, target, json!({ "path": target })),
            entry(MANIFEST_KIND, "name", json!(MANIFEST_NAME)),
            entry(MANIFEST_KIND, "version", json!(MANIFEST_VERSION)),
        ]
    }
}
