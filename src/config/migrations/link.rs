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
