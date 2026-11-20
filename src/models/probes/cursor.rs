use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::command::{probe_timeout, run_command, strip_ansi};

/// Result of probing cursor's runtime model catalog.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CursorProbeResult {
    /// Raw slug strings from `cursor agent --list-models` (fast variants excluded).
    pub slugs: Vec<String>,
    /// Whether the model list probe succeeded.
    pub model_probe_success: bool,
    /// Redacted error message if probing failed.
    pub error: Option<String>,
}

/// Probe cursor with the configured timeout.
pub fn probe() -> CursorProbeResult {
    probe_with_timeout(probe_timeout())
}

impl super::cache::CachedProbe for CursorProbeResult {
    const HARNESS: &'static str = "cursor";
    const CACHE_FILE: &'static str = "cursor-probe.json";
    const LOCK_FILE: &'static str = ".cursor-probe.lock";
    const SCHEMA_VERSION: u32 = 1;

    fn probe() -> Self {
        probe()
    }

    fn is_usable(&self) -> bool {
        self.model_probe_success
    }

    fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    fn listing(&self) -> super::Listing {
        super::Listing {
            slugs: self.slugs.clone(),
            succeeded: self.model_probe_success,
            error: self.error.clone(),
            empty_is_failure: true,
            ..super::Listing::default()
        }
    }
}

/// Probe cursor with a specific timeout.
pub fn probe_with_timeout(timeout: Duration) -> CursorProbeResult {
    let mut result = CursorProbeResult::default();

    match run_command("cursor", &["agent", "--list-models"], timeout) {
        Ok(stdout) => {
            result.slugs = parse_cursor_models_output(&stdout);
            result.model_probe_success = true;
        }
        Err(error) => {
            result.model_probe_success = false;
            result.error = Some(format!("model probe failed: {error}"));
        }
    }

    result
}

fn parse_cursor_models_output(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter_map(|line| {
            let clean = strip_ansi(line.trim());
            if clean.is_empty()
                || clean.eq_ignore_ascii_case("available models")
                || clean.starts_with("Tip:")
            {
                return None;
            }

            let (slug, _) = clean.split_once(" - ")?;
            let slug = slug.trim();
            if slug.is_empty() || slug.ends_with("-fast") {
                return None;
            }

            Some(slug.to_string())
        })
        .collect()
}

/// Cursor's spelling of a model models.dev spells differently.
pub fn model_id_shim(model_id: &str) -> Option<String> {
    match model_id.to_ascii_lowercase().as_str() {
        "claude-opus-4-6" => Some("claude-4.6-opus".to_string()),
        "claude-sonnet-4-6" => Some("claude-4.6-sonnet".to_string()),
        _ => None,
    }
}
