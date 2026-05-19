// qa-validated: harness-order-settings-audit

use std::collections::HashSet;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use wait_timeout::ChildExt;

const HARNESS_BINARIES: &[(&str, &str)] = &[
    ("claude", "claude"),
    ("codex", "codex"),
    ("opencode", "opencode"),
    ("cursor", "cursor"),
    ("pi", "pi"),
];

const ORDERABLE_HARNESSES: &[&str] = &["claude", "codex", "opencode", "cursor", "pi"];

pub const VALID_HARNESSES: &[&str] = &["claude", "codex", "pi", "opencode", "cursor"];

pub fn detect_installed_harnesses() -> HashSet<String> {
    HARNESS_BINARIES
        .iter()
        .filter(|(_, binary)| harness_binary_exists(binary))
        .map(|(name, _)| name.to_string())
        .collect()
}

fn harness_binary_exists(binary: &str) -> bool {
    if which::which(binary).is_ok() {
        return true;
    }

    #[cfg(windows)]
    {
        ["exe", "cmd", "bat"]
            .iter()
            .any(|ext| which::which(format!("{binary}.{ext}")).is_ok())
    }

    #[cfg(not(windows))]
    {
        false
    }
}

const PROVIDER_HARNESS_PREFERENCES: &[(&str, &[&str])] = &[
    ("anthropic", &["claude", "pi", "opencode", "cursor"]),
    ("openai", &["codex", "pi", "opencode", "cursor"]),
    ("google", &["pi", "opencode", "cursor"]),
    ("meta", &["pi", "opencode", "cursor"]),
    ("mistral", &["pi", "opencode", "cursor"]),
    ("deepseek", &["pi", "opencode", "cursor"]),
    ("cohere", &["pi", "opencode", "cursor"]),
];

const DEFAULT_FALLBACK_ORDER: &[&str] = &["pi", "opencode", "cursor"];

pub fn is_valid_harness(name: &str) -> bool {
    normalize_harness_name(name).is_some()
}

pub fn normalize_harness_name(name: &str) -> Option<String> {
    let normalized = name.trim().to_ascii_lowercase();
    VALID_HARNESSES
        .contains(&normalized.as_str())
        .then_some(normalized)
}

fn harness_preferences(provider: &str) -> &'static [&'static str] {
    let provider_lower = provider.to_ascii_lowercase();
    PROVIDER_HARNESS_PREFERENCES
        .iter()
        .find(|(p, _)| *p == provider_lower)
        .map(|(_, prefs)| *prefs)
        .unwrap_or(DEFAULT_FALLBACK_ORDER)
}

pub fn harness_candidates_for_provider(provider: &str) -> Vec<String> {
    harness_preferences(provider)
        .iter()
        .map(|h| h.to_string())
        .collect()
}

pub fn native_harness_authenticated(harness: &str) -> bool {
    match harness {
        "codex" => run_auth_status_command("codex", &["login", "status"]),
        "claude" => run_auth_status_command("claude", &["auth", "status"]),
        _ => false,
    }
}

pub fn run_auth_status_command(command: &str, args: &[&str]) -> bool {
    let mut child = match Command::new(resolve_command(command))
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return false,
    };

    match child.wait_timeout(auth_probe_timeout()) {
        Ok(Some(status)) => status.success(),
        Ok(None) => {
            let _ = child.kill();
            let _ = child.wait();
            false
        }
        Err(_) => false,
    }
}

pub fn auth_probe_timeout() -> Duration {
    std::env::var("MARS_NATIVE_HARNESS_AUTH_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(2))
}

pub fn resolve_command(command: &str) -> PathBuf {
    if let Ok(path) = which::which(command) {
        return path;
    }

    #[cfg(windows)]
    {
        for ext in ["exe", "cmd", "bat"] {
            if let Ok(path) = which::which(format!("{command}.{ext}")) {
                return path;
            }
        }
    }

    PathBuf::from(command)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HarnessOrderFailure {
    Empty,
    NoneInstalled { valid_candidates: Vec<String> },
}

pub struct ParsedHarnessOrder {
    pub valid_candidates: Vec<String>,
    pub warnings: Vec<String>,
    pub failure: Option<HarnessOrderFailure>,
}

pub fn parse_settings_harness_order(order: &[String]) -> ParsedHarnessOrder {
    if order.is_empty() {
        return ParsedHarnessOrder {
            valid_candidates: Vec::new(),
            warnings: Vec::new(),
            failure: Some(HarnessOrderFailure::Empty),
        };
    }

    let mut valid_candidates = Vec::new();
    let mut warnings = Vec::new();
    for candidate in order {
        let Some(normalized) = normalize_harness_name(candidate) else {
            warnings.push(format!(
                "settings.harness_order contains unrecognized harness `{candidate}`; skipping (valid: {})",
                ORDERABLE_HARNESSES.join(", ")
            ));
            continue;
        };
        if !ORDERABLE_HARNESSES.contains(&normalized.as_str()) {
            warnings.push(format!(
                "settings.harness_order contains unrecognized harness `{candidate}`; skipping (valid: {})",
                ORDERABLE_HARNESSES.join(", ")
            ));
            continue;
        }
        valid_candidates.push(normalized);
    }

    ParsedHarnessOrder {
        valid_candidates,
        warnings,
        failure: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_for_known_provider() {
        let candidates = harness_candidates_for_provider("openai");
        assert_eq!(candidates, vec!["codex", "pi", "opencode", "cursor"]);
    }

    #[test]
    fn candidates_for_anthropic_use_pi_first_fallback_chain() {
        let candidates = harness_candidates_for_provider("anthropic");
        assert_eq!(candidates, vec!["claude", "pi", "opencode", "cursor"]);
    }

    #[test]
    fn candidates_for_unknown_provider() {
        let candidates = harness_candidates_for_provider("unknown");
        assert_eq!(candidates, vec!["pi", "opencode", "cursor"]);
    }

    #[test]
    fn valid_harness_validation_rejects_gemini() {
        assert!(is_valid_harness("claude"));
        assert!(is_valid_harness("OpenCode"));
        assert!(!is_valid_harness("gemini"));
        assert!(!is_valid_harness("unknown"));
    }
}
