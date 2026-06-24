//! Flat-root foreign skill source naming shared by discovery and staging overlay.

use std::path::Path;

/// Source name for a flat-root package (`SKILL.md` at package root).
///
/// When `explicit_source_name` is provided (dependency resolution / staging), that
/// name is used. Otherwise falls back to the package directory basename.
pub(crate) fn flat_root_skill_source_name(
    package_root: &Path,
    explicit_source_name: Option<&str>,
) -> String {
    explicit_source_name
        .map(str::to_owned)
        .or_else(|| {
            package_root
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "unknown-skill".to_string())
}
