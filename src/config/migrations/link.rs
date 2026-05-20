//! Link normalization and legacy compatibility.
//!
//! Backward-compatible migration facade over live `config::targets`.

/// Canonical read-time view of one `settings.targets` / `managed_root` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedLink {
    pub target: String,
    pub harness: Option<String>,
}

pub fn normalize_link(raw: &str) -> NormalizedLink {
    let link = crate::config::targets::normalize_link(raw);
    NormalizedLink {
        target: link.target,
        harness: link.harness.map(|harness| harness.to_string()),
    }
}

pub fn normalized_targets<'a>(links: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    crate::config::targets::normalized_targets(links)
}

pub fn linked_harnesses<'a>(links: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    crate::config::targets::linked_harnesses(links)
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
