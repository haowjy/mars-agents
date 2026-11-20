use std::time::Duration;

use mars_schema::HarnessId;
use serde::{Deserialize, Serialize};

use super::command::{probe_timeout, run_command, strip_ansi};

/// Result of probing `agy models`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgyProbeResult {
    /// The slugs `agy models` lists, effort tier included (`gemini-3.8-flash-low`).
    pub slugs: Vec<String>,
    /// Whether the model list probe succeeded.
    pub model_probe_success: bool,
    /// Redacted error message if probing failed.
    pub error: Option<String>,
}

/// Probe `agy` with the configured timeout.
pub fn probe() -> AgyProbeResult {
    probe_with_timeout(probe_timeout())
}

impl super::cache::CachedProbe for AgyProbeResult {
    const HARNESS: HarnessId = HarnessId::Agy;
    const CACHE_FILE: &'static str = "agy-probe.json";
    const LOCK_FILE: &'static str = ".agy-probe.lock";
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

/// Probe `agy models` with a specific timeout. A non-zero exit or an empty list is a failed
/// probe; `agy` printing nothing when logged out was not observed, so both read as "no
/// evidence" rather than as a listing.
pub fn probe_with_timeout(timeout: Duration) -> AgyProbeResult {
    let mut result = AgyProbeResult::default();
    match run_command(HarnessId::Agy.as_str(), &["models"], timeout) {
        Ok(stdout) => {
            result.slugs = parse_models_output(&stdout);
            result.model_probe_success = true;
        }
        Err(error) => result.error = Some(format!("model probe failed: {error}")),
    }
    result
}

/// `<slug>\t<display name>` per line, after a `Fetching available models...` line. A line
/// with no tab is that banner or a message, not a model.
fn parse_models_output(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter_map(|line| {
            let clean = strip_ansi(line);
            let (slug, _) = clean.split_once('\t')?;
            let slug = slug.trim();
            (!slug.is_empty()).then(|| slug.to_owned())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_the_first_column_after_the_banner() {
        let out = "Fetching available models...\ngemini-3.8-flash-high\tGemini 3.8 Flash (High)\nclaude-sonnet-4-6\tClaude Sonnet 4.6 (Thinking)\n";
        assert_eq!(
            parse_models_output(out),
            ["gemini-3.8-flash-high", "claude-sonnet-4-6"]
        );
    }
}
