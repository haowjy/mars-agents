use std::time::Duration;

use mars_schema::HarnessId;
use serde::{Deserialize, Serialize};

use super::command::{probe_timeout, run_command, strip_ansi};

/// Result of probing OpenCode's runtime availability.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OpenCodeProbeResult {
    /// Full model slug list, e.g. `openai/gpt-5.4`.
    pub model_slugs: Vec<String>,
    /// Whether the model list probe succeeded.
    pub model_probe_success: bool,
    /// Redacted error message if probing failed.
    pub error: Option<String>,
}

/// Probe OpenCode with the configured timeout.
pub fn probe() -> OpenCodeProbeResult {
    probe_with_timeout(probe_timeout())
}

impl super::cache::CachedProbe for OpenCodeProbeResult {
    const HARNESS: HarnessId = HarnessId::OpenCode;
    const CACHE_FILE: &'static str = "opencode-probe.json";
    const LOCK_FILE: &'static str = ".opencode-probe.lock";
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
            slugs: self.model_slugs.clone(),
            succeeded: self.model_probe_success,
            error: self.error.clone(),
            ..super::Listing::default()
        }
    }
}

/// Probe OpenCode with a specific timeout.
pub fn probe_with_timeout(timeout: Duration) -> OpenCodeProbeResult {
    let mut result = OpenCodeProbeResult::default();

    match run_command(HarnessId::OpenCode.as_str(), &["models"], timeout) {
        Ok(stdout) => {
            result.model_slugs = parse_models_output(&stdout);
            result.model_probe_success = true;
        }
        Err(error) => {
            result.model_probe_success = false;
            result.error = Some(format!("model probe failed: {error}"));
        }
    }

    result
}

fn parse_models_output(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .map(|line| strip_ansi(line.trim()))
        .filter(|line| !line.is_empty() && line.contains('/'))
        .collect()
}
