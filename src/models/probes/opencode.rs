use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use wait_timeout::ChildExt;

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

const DEFAULT_PROBE_TIMEOUT_SECS: u64 = 5;

/// Probe OpenCode with the configured timeout.
pub fn probe() -> OpenCodeProbeResult {
    probe_with_timeout(probe_timeout())
}

/// Probe OpenCode with a specific timeout.
pub fn probe_with_timeout(timeout: Duration) -> OpenCodeProbeResult {
    let mut result = OpenCodeProbeResult::default();

    match run_command("opencode", &["models"], timeout) {
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

/// Parse `opencode models` output into slug list.
fn parse_models_output(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .map(|line| strip_ansi(line.trim()))
        .filter(|line| !line.is_empty() && line.contains('/'))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_opencode_models_output() {
        // Only provider-qualified lines are slugs; ANSI and blank lines are noise.
        let output = "\x1b[32mopencode/big-pickle\x1b[0m\ngoogle/gemini-2.5-pro\n\
some-invalid-line\n\n\
openrouter/anthropic/claude-opus-4.7\n";
        assert_eq!(
            parse_models_output(output),
            vec![
                "opencode/big-pickle".to_string(),
                "google/gemini-2.5-pro".to_string(),
                "openrouter/anthropic/claude-opus-4.7".to_string(),
            ]
        );
    }
}
