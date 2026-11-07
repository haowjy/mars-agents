//! The target-scoped copy of a package-owned directory.
//!
//! Hooks and MCP servers are installed as one directory per managed target.
//! Canonical build, target sync, lock ownership, and recovery all have to name
//! the same copy, so every path and identifier for it is derived here.

use mars_schema::item::ItemKind;

use crate::types::DestPath;

fn surface(kind: ItemKind) -> Option<&'static str> {
    match kind {
        ItemKind::Hook => Some("hooks"),
        ItemKind::McpServer => Some("mcp"),
        _ => None,
    }
}

pub(crate) fn installs_per_target(kind: ItemKind) -> bool {
    surface(kind).is_some()
}

fn canonical_target(target: &str) -> &str {
    target.trim_start_matches('.')
}

/// The target is always the target directory name (`.claude`), whichever
/// spelling the constructing caller used. Canonical paths and lock keys trim
/// the dot back off.
#[derive(Debug, Clone)]
pub(crate) struct PackageDir {
    kind: ItemKind,
    /// `kind`'s `.mars` surface, resolved once where the kind was validated so
    /// every path below is total.
    surface: &'static str,
    target: String,
    name: String,
}

impl PackageDir {
    pub(crate) fn new(kind: ItemKind, target: &str, name: &str) -> Option<Self> {
        Some(Self {
            kind,
            surface: surface(kind)?,
            target: format!(".{}", canonical_target(target)),
            name: name.to_string(),
        })
    }

    pub(crate) fn parse_canonical(kind: ItemKind, dest: &DestPath) -> Option<Self> {
        let surface = surface(kind)?;
        let mut parts = dest.as_str().split('/');
        match (parts.next(), parts.next(), parts.next(), parts.next()) {
            (Some(found), Some(target), Some(name), None) if found == surface => {
                Self::new(kind, target, name)
            }
            _ => None,
        }
    }

    pub(crate) fn from_native(kind: ItemKind, target: &str, dest: &str) -> Option<Self> {
        let (found, name) = dest.split_once('/')?;
        if found != surface(kind)? || name.contains('/') {
            return None;
        }
        Self::new(kind, target, name)
    }

    pub(crate) fn kind(&self) -> ItemKind {
        self.kind
    }

    pub(crate) fn target(&self) -> &str {
        &self.target
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn belongs_to(&self, target: &str) -> bool {
        self.target == target
    }

    /// `mcp/claude/echo`: where the copy lives under `.mars/`.
    pub(crate) fn canonical(&self) -> DestPath {
        DestPath::new(format!(
            "{}/{}/{}",
            self.surface,
            canonical_target(&self.target),
            self.name
        ))
        .expect("validated target and item names form a safe canonical path")
    }

    /// `mcp/echo`: where the copy lives under the target root.
    pub(crate) fn native(&self) -> String {
        format!("{}/{}", self.surface, self.name)
    }

    /// `echo@claude`: this copy's canonical item name and lock-key segment.
    pub(crate) fn lock_name(&self) -> String {
        format!("{}@{}", self.name, canonical_target(&self.target))
    }

    pub(crate) fn not_installed_label(&self) -> (&'static str, &'static str) {
        match self.kind {
            ItemKind::McpServer => ("mcp-not-installed", "MCP server"),
            ItemKind::Hook => ("hook-not-installed", "hook"),
            other => unreachable!("{other:?} installs no package-owned directory"),
        }
    }
}
