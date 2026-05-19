//! Link normalization and legacy compatibility.
//!
//! This module is the migration boundary between historical path-form link
//! entries (`.codex`) and canonical read-time intent (`harness = codex`,
//! `target = .codex`). Normal resolver and sync code should consume the
//! normalized outputs instead of checking legacy spellings directly.

use std::collections::HashSet;

/// Canonical read-time view of one `settings.targets` / `managed_root` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedLink {
    /// Materialization directory to sync into.
    pub target: String,
    /// Harness intent for known harness targets. Generic targets leave this unset.
    pub harness: Option<String>,
}

/// Normalize one configured link target.
///
/// Known harnesses accept both harness-name and legacy path-form spelling:
/// `codex` and `.codex` both become `{ target: ".codex", harness: "codex" }`.
/// Unknown simple names are generic materialization targets and are dot-prefixed
/// for compatibility with historical target directories.
///
/// Values containing path separators are preserved as generic targets; CLI
/// parsing rejects those for new writes, but read-time normalization should not
/// reinterpret old hand-written path-like config.
pub fn normalize_link(raw: &str) -> NormalizedLink {
    let trimmed = raw.trim().trim_end_matches('/').trim_end_matches('\\');
    if trimmed.contains('/') || trimmed.contains('\\') {
        return NormalizedLink {
            target: trimmed.to_string(),
            harness: None,
        };
    }

    let bare = trimmed.strip_prefix('.').unwrap_or(trimmed);
    if let Some(harness) = crate::models::harness::normalize_harness_name(bare) {
        return NormalizedLink {
            target: format!(".{harness}"),
            harness: Some(harness),
        };
    }

    if bare.is_empty() {
        return NormalizedLink {
            target: trimmed.to_string(),
            harness: None,
        };
    }

    NormalizedLink {
        target: format!(".{bare}"),
        harness: None,
    }
}

/// Return canonical materialization target paths, de-duplicated in input order.
pub fn normalized_targets<'a>(links: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut targets = Vec::new();
    for link in links {
        let target = normalize_link(link).target;
        if seen.insert(target.clone()) {
            targets.push(target);
        }
    }
    targets
}

/// Return linked harness intents, de-duplicated in input order.
pub fn linked_harnesses<'a>(links: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut harnesses = Vec::new();
    for link in links {
        if let Some(harness) = normalize_link(link).harness
            && seen.insert(harness.clone())
        {
            harnesses.push(harness);
        }
    }
    harnesses
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_harness_name_and_legacy_path_form() {
        assert_eq!(
            normalize_link("codex"),
            NormalizedLink {
                target: ".codex".to_string(),
                harness: Some("codex".to_string()),
            }
        );
        assert_eq!(
            normalize_link(".codex"),
            NormalizedLink {
                target: ".codex".to_string(),
                harness: Some("codex".to_string()),
            }
        );
    }

    #[test]
    fn normalizes_agents_as_generic_target() {
        assert_eq!(
            normalize_link("agents"),
            NormalizedLink {
                target: ".agents".to_string(),
                harness: None,
            }
        );
        assert_eq!(
            normalize_link(".agents"),
            NormalizedLink {
                target: ".agents".to_string(),
                harness: None,
            }
        );
    }

    #[test]
    fn dot_prefixes_unknown_bare_names_as_generic_targets() {
        assert_eq!(
            normalize_link("foo"),
            NormalizedLink {
                target: ".foo".to_string(),
                harness: None,
            }
        );
        assert_eq!(
            normalize_link(".foo"),
            NormalizedLink {
                target: ".foo".to_string(),
                harness: None,
            }
        );
    }

    #[test]
    fn extracts_linked_harnesses_from_legacy_target_paths() {
        let targets = [".codex", ".claude", ".agents", "foo"];
        assert_eq!(
            linked_harnesses(targets.iter().copied()),
            vec!["codex".to_string(), "claude".to_string()]
        );
    }
}
