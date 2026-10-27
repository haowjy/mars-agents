//! The target-scoped copy of a package-owned directory.
//!
//! Hooks and MCP servers are installed as one directory per managed target.
//! Canonical build, target sync, lock ownership, and recovery all have to name
//! the same copy, so every path and identifier for it is derived here.

use mars_schema::item::ItemKind;

use crate::types::DestPath;

/// A package-owned directory kind: the item kinds that install once per target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PackageDirKind {
    Hook,
    McpServer,
}

impl PackageDirKind {
    /// The package-directory kind of `kind`, or `None` for items installed once
    /// for the whole project.
    pub(crate) fn of(kind: ItemKind) -> Option<Self> {
        match kind {
            ItemKind::Hook => Some(Self::Hook),
            ItemKind::McpServer => Some(Self::McpServer),
            _ => None,
        }
    }

    pub(crate) fn item_kind(self) -> ItemKind {
        match self {
            Self::Hook => ItemKind::Hook,
            Self::McpServer => ItemKind::McpServer,
        }
    }

    /// Canonical `.mars` directory holding this kind's copies: `hooks/claude`.
    pub(crate) fn canonical_dir(self, target: &str) -> String {
        format!("{}/{}", self.surface(), target.trim_start_matches('.'))
    }

    /// Directory under a target that holds this kind's package-owned copy.
    fn surface(self) -> &'static str {
        match self {
            Self::Hook => "hooks",
            Self::McpServer => "mcp",
        }
    }
}

/// One package-owned directory installed for one target.
///
/// The target is stored without its leading dot, the form a canonical `.mars`
/// path uses; comparisons and constructors accept it either way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PackageDir {
    kind: PackageDirKind,
    target: String,
    name: String,
}

impl PackageDir {
    /// The copy `kind` installs for `target`; `target` may keep its leading dot.
    pub(crate) fn new(kind: PackageDirKind, target: &str, name: &str) -> Self {
        Self {
            kind,
            target: target.trim_start_matches('.').to_string(),
            name: name.to_string(),
        }
    }

    /// Parse a canonical `.mars` dest path such as `mcp/claude/echo`.
    pub(crate) fn parse_canonical(kind: ItemKind, dest: &DestPath) -> Option<Self> {
        let kind = PackageDirKind::of(kind)?;
        let mut parts = dest.as_str().split('/');
        match (parts.next(), parts.next(), parts.next(), parts.next()) {
            (Some(surface), Some(target), Some(name), None) if surface == kind.surface() => {
                Some(Self {
                    kind,
                    target: target.to_string(),
                    name: name.to_string(),
                })
            }
            _ => None,
        }
    }

    /// Parse a destination relative to a target, such as `mcp/echo`.
    pub(crate) fn from_native(kind: ItemKind, target: &str, dest: &str) -> Option<Self> {
        let kind = PackageDirKind::of(kind)?;
        let (surface, name) = dest.split_once('/')?;
        (surface == kind.surface() && !name.contains('/')).then(|| Self::new(kind, target, name))
    }

    pub(crate) fn item_kind(&self) -> ItemKind {
        self.kind.item_kind()
    }

    /// Whether this copy belongs to `target`, which may keep its leading dot.
    pub(crate) fn belongs_to(&self, target: &str) -> bool {
        self.target == target.trim_start_matches('.')
    }

    /// `mcp/claude/echo`: where the copy lives under `.mars/`.
    pub(crate) fn canonical(&self) -> DestPath {
        DestPath::new(format!(
            "{}/{}",
            self.kind.canonical_dir(&self.target),
            self.name
        ))
        .expect("validated target and item names form a safe canonical path")
    }

    /// `mcp/echo`: where the copy lives under the target root.
    pub(crate) fn native(&self) -> String {
        format!("{}/{}", self.kind.surface(), self.name)
    }

    /// `echo@claude`: this copy's canonical item name and lock-key segment.
    pub(crate) fn lock_name(&self) -> String {
        format!("{}@{}", self.name, self.target)
    }
}
