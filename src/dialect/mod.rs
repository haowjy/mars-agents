//! Inbound dialect vocabulary for foreign → canonical lift.
//!
//! Mirrors the first-class harness set (minus Pi) plus `MarsNative` for
//! already-canonical mars-authored sources.

use std::fmt;
use std::path::Path;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::compiler::agents::HarnessKind;

/// Recognized inbound source dialects for lift.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Dialect {
    Claude,
    Codex,
    OpenCode,
    Cursor,
    #[serde(rename = "mars-native")]
    MarsNative,
}

impl Dialect {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::OpenCode => "opencode",
            Self::Cursor => "cursor",
            Self::MarsNative => "mars-native",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        let normalized = name.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "opencode" => Some(Self::OpenCode),
            "cursor" => Some(Self::Cursor),
            "mars-native" | "marsnative" | "mars_native" => Some(Self::MarsNative),
            _ => None,
        }
    }

    pub fn from_harness_kind(kind: HarnessKind) -> Option<Self> {
        match kind {
            HarnessKind::Claude => Some(Self::Claude),
            HarnessKind::Codex => Some(Self::Codex),
            HarnessKind::OpenCode => Some(Self::OpenCode),
            HarnessKind::Cursor => Some(Self::Cursor),
            HarnessKind::Pi => None,
        }
    }

    pub fn to_harness_kind(self) -> Option<HarnessKind> {
        match self {
            Self::Claude => Some(HarnessKind::Claude),
            Self::Codex => Some(HarnessKind::Codex),
            Self::OpenCode => Some(HarnessKind::OpenCode),
            Self::Cursor => Some(HarnessKind::Cursor),
            Self::MarsNative => None,
        }
    }

    /// Resolve dialect for a dependency: explicit config > container inference > Claude.
    pub fn resolve(explicit: Option<Self>, package_root: &Path) -> Self {
        Self::resolve_with_default(explicit, package_root, Self::Claude)
    }

    /// Resolve dialect for local project items: explicit > container inference > MarsNative.
    pub fn resolve_local(explicit: Option<Self>, package_root: &Path) -> Self {
        Self::resolve_with_default(explicit, package_root, Self::MarsNative)
    }

    fn resolve_with_default(explicit: Option<Self>, package_root: &Path, default: Self) -> Self {
        if let Some(dialect) = explicit {
            return dialect;
        }
        if let Some(inferred) = infer_from_foreign_containers(package_root) {
            return inferred;
        }
        default
    }
}

impl fmt::Display for Dialect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Dialect {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s).ok_or(())
    }
}

const FOREIGN_CONTAINER_SIGNALS: &[(&str, Dialect)] = &[
    (".claude", Dialect::Claude),
    (".codex", Dialect::Codex),
    (".opencode", Dialect::OpenCode),
    (".cursor", Dialect::Cursor),
];

/// Infer dialect when foreign ecosystem container directories contain items.
fn infer_from_foreign_containers(package_root: &Path) -> Option<Dialect> {
    let mut matched = Vec::new();
    for (container, dialect) in FOREIGN_CONTAINER_SIGNALS {
        if foreign_container_has_items(package_root, container) {
            matched.push(*dialect);
        }
    }
    if matched.len() == 1 {
        matched.first().copied()
    } else {
        None
    }
}

fn foreign_container_has_items(package_root: &Path, container: &str) -> bool {
    for sub in ["skills", "agents"] {
        let dir = package_root.join(container).join(sub);
        if dir.is_dir()
            && std::fs::read_dir(&dir)
                .ok()
                .into_iter()
                .flatten()
                .flatten()
                .next()
                .is_some()
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn parse_dialect_names() {
        assert_eq!(Dialect::parse("claude"), Some(Dialect::Claude));
        assert_eq!(Dialect::parse("mars-native"), Some(Dialect::MarsNative));
        assert_eq!(Dialect::parse("unknown"), None);
    }

    #[test]
    fn infer_from_claude_container() {
        let dir = TempDir::new().unwrap();
        let skill = dir.path().join(".claude/skills/demo");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(skill.join("SKILL.md"), "# demo").unwrap();

        assert_eq!(
            infer_from_foreign_containers(dir.path()),
            Some(Dialect::Claude)
        );
        assert_eq!(Dialect::resolve(None, dir.path()), Dialect::Claude);
    }

    #[test]
    fn bare_skills_default_to_claude_for_dependencies() {
        let dir = TempDir::new().unwrap();
        let skill = dir.path().join("skills/demo");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(skill.join("SKILL.md"), "# demo").unwrap();

        assert_eq!(infer_from_foreign_containers(dir.path()), None);
        assert_eq!(Dialect::resolve(None, dir.path()), Dialect::Claude);
    }

    #[test]
    fn bare_skills_default_to_mars_native_for_local_items() {
        let dir = TempDir::new().unwrap();
        let skill = dir.path().join("skills/demo");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(skill.join("SKILL.md"), "# demo").unwrap();

        assert_eq!(
            Dialect::resolve_local(None, dir.path()),
            Dialect::MarsNative
        );
    }

    #[test]
    fn local_claude_container_still_infers_claude() {
        let dir = TempDir::new().unwrap();
        let skill = dir.path().join(".claude/skills/demo");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(skill.join("SKILL.md"), "# demo").unwrap();

        assert_eq!(Dialect::resolve_local(None, dir.path()), Dialect::Claude);
    }

    #[test]
    fn explicit_beats_inference() {
        let dir = TempDir::new().unwrap();
        let skill = dir.path().join(".claude/skills/demo");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(skill.join("SKILL.md"), "# demo").unwrap();

        assert_eq!(
            Dialect::resolve(Some(Dialect::Codex), dir.path()),
            Dialect::Codex
        );
    }
}
