//! The target-scoped copy of a package-owned directory.
//!
//! Hooks and MCP servers are installed as one directory per managed target.
//! Canonical build, target sync, lock ownership, and recovery all have to name
//! the same copy, so every path and identifier for it is derived here.

use mars_schema::item::ItemKind;

use crate::types::DestPath;

/// The `.mars` surface holding `kind`'s per-target copies, or `None` for a
/// kind installed once for the whole project.
fn surface(kind: ItemKind) -> Option<&'static str> {
    match kind {
        ItemKind::Hook => Some("hooks"),
        ItemKind::McpServer => Some("mcp"),
        _ => None,
    }
}

/// Whether `kind` installs one package-owned directory per target.
pub(crate) fn installs_per_target(kind: ItemKind) -> bool {
    surface(kind).is_some()
}

/// `hooks/claude`: the canonical `.mars` directory holding `kind`'s copies for
/// `target`, or `None` for a kind installed once for the whole project.
pub(crate) fn canonical_dir(kind: ItemKind, target: &str) -> Option<String> {
    Some(format!("{}/{}", surface(kind)?, canonical_target(target)))
}

/// A target name without the leading dot its target directory carries.
fn canonical_target(target: &str) -> &str {
    target.trim_start_matches('.')
}

/// One package-owned directory installed for one target.
///
/// The target keeps the spelling the constructing caller used: the target
/// directory (`.claude`) when built from a target root, the canonical form
/// (`claude`) when parsed from a `.mars` path. Canonical paths and lock keys
/// trim the dot; comparisons accept either.
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
    /// The copy `kind` installs for `target`, or `None` for a kind installed
    /// once for the whole project.
    pub(crate) fn new(kind: ItemKind, target: &str, name: &str) -> Option<Self> {
        Some(Self {
            kind,
            surface: surface(kind)?,
            target: target.to_string(),
            name: name.to_string(),
        })
    }

    /// Parse a canonical `.mars` dest path such as `mcp/claude/echo`.
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

    /// Parse a destination relative to a target, such as `mcp/echo`.
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

    /// The target this copy belongs to. A selection record builds it from the
    /// managed target root, so it is the target directory name (`.claude`); a
    /// copy parsed from a `.mars` path carries the canonical form.
    pub(crate) fn target(&self) -> &str {
        &self.target
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    /// Whether this copy belongs to `target`, which may keep its leading dot.
    pub(crate) fn belongs_to(&self, target: &str) -> bool {
        canonical_target(&self.target) == canonical_target(target)
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

    /// The diagnostic code and noun for "this copy did not install".
    pub(crate) fn not_installed_label(&self) -> (&'static str, &'static str) {
        match self.kind {
            ItemKind::McpServer => ("mcp-not-installed", "MCP server"),
            _ => ("hook-not-installed", "hook"),
        }
    }
}
