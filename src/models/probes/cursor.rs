use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use wait_timeout::ChildExt;

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

const DEFAULT_PROBE_TIMEOUT_SECS: u64 = 5;

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

fn probe_timeout() -> Duration {
    std::env::var("MARS_PROBE_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(DEFAULT_PROBE_TIMEOUT_SECS))
}

fn run_command(cmd: &str, args: &[&str], timeout: Duration) -> Result<String, String> {
    let program = resolve_command(cmd);
    let mut child = Command::new(&program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("spawn failed: {error}"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "stdout capture unavailable".to_string())?;
    let stdout_reader = thread::spawn(move || {
        let mut stdout = stdout;
        let mut output = Vec::new();
        stdout
            .read_to_end(&mut output)
            .map(|_| output)
            .map_err(|error| format!("stdout read failed: {error}"))
    });

    match child
        .wait_timeout(timeout)
        .map_err(|error| format!("wait failed: {error}"))?
    {
        Some(status) if status.success() => {
            let stdout = stdout_reader
                .join()
                .map_err(|_| "stdout reader panicked".to_string())??;
            String::from_utf8(stdout).map_err(|error| format!("invalid utf8: {error}"))
        }
        Some(status) => {
            let _ = stdout_reader.join();
            Err(format!("exit code {}", status.code().unwrap_or(-1)))
        }
        None => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_reader.join();
            Err("timeout".to_string())
        }
    }
}

fn resolve_command(cmd: &str) -> PathBuf {
    let resolver = crate::harness::host::PathExecutableResolver;
    crate::harness::host::resolve_binary_path(cmd, &resolver).unwrap_or_else(|| cmd.into())
}

fn strip_ansi(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            while let Some(&next) = chars.peek() {
                chars.next();
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            result.push(ch);
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
