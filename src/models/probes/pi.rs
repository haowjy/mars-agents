use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use wait_timeout::ChildExt;

use crate::harness::host::{PathExecutableResolver, resolve_binary_path};

const DEFAULT_PROBE_TIMEOUT_SECS: u64 = 5;

pub const PI_REQUIRED_HELP_TOKEN_GROUPS: &[&[&str]] = &[
    &["--mode"],
    &["rpc"],
    &["--model"],
    &["--append-system-prompt"],
    &["--session"],
    &["--fork"],
    &["--session-dir", "PI_CODING_AGENT_SESSION_DIR"],
    &["--no-extensions"],
    &["--no-skills"],
    &["--no-context-files"],
    &["--no-prompt-templates"],
    &["-e", "--extension"],
];

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PiProbeResult {
    pub binary_path: String,
    pub version: Option<String>,
    pub compatible: bool,
    pub help_surface_tokens_present: Vec<String>,
    pub help_surface_tokens_missing: Vec<String>,
    pub error: Option<String>,
}

pub fn probe() -> PiProbeResult {
    probe_with_timeout(probe_timeout())
}

pub fn probe_with_timeout(timeout: Duration) -> PiProbeResult {
    let resolver = PathExecutableResolver;
    let Some(binary_path) = resolve_binary_path("pi", &resolver) else {
        return PiProbeResult {
            compatible: false,
            error: Some("pi binary not found".to_string()),
            ..PiProbeResult::default()
        };
    };

    let binary_path_text = binary_path.to_string_lossy().to_string();
    let version_output = match run_command(&binary_path, &["--version"], timeout) {
        Ok(stdout) => stdout,
        Err(error) => {
            return PiProbeResult {
                binary_path: binary_path_text,
                compatible: false,
                error: Some(format!("pi --version probe failed: {error}")),
                ..PiProbeResult::default()
            };
        }
    };

    let help_output = match run_command(&binary_path, &["--help"], timeout) {
        Ok(stdout) => stdout,
        Err(error) => {
            return PiProbeResult {
                binary_path: binary_path_text,
                version: first_non_empty_line(&version_output),
                compatible: false,
                error: Some(format!("pi --help probe failed: {error}")),
                ..PiProbeResult::default()
            };
        }
    };

    let (present, missing) = classify_help_tokens(&help_output);

    PiProbeResult {
        binary_path: binary_path_text,
        version: first_non_empty_line(&version_output),
        compatible: missing.is_empty(),
        help_surface_tokens_present: present,
        help_surface_tokens_missing: missing,
        error: None,
    }
}

fn probe_timeout() -> Duration {
    std::env::var("MARS_PROBE_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(DEFAULT_PROBE_TIMEOUT_SECS))
}

fn run_command(
    program: &std::path::Path,
    args: &[&str],
    timeout: Duration,
) -> Result<String, String> {
    let mut child = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
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

fn first_non_empty_line(output: &str) -> Option<String> {
    output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

fn classify_help_tokens(help_output: &str) -> (Vec<String>, Vec<String>) {
    let mut present = Vec::new();
    let mut missing = Vec::new();
    let lowered = help_output.to_ascii_lowercase();

    for group in PI_REQUIRED_HELP_TOKEN_GROUPS {
        let found = group
            .iter()
            .copied()
            .find(|token| lowered.contains(&token.to_ascii_lowercase()));

        if let Some(token) = found {
            present.push(token.to_string());
        } else {
            missing.push(group.join(" | "));
        }
    }

    (present, missing)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_help_tokens_detects_full_compatibility() {
        let help = "--mode rpc --model --append-system-prompt --session --fork --session-dir --no-extensions --no-skills --no-context-files --no-prompt-templates -e";
        let (present, missing) = classify_help_tokens(help);
        assert_eq!(missing, Vec::<String>::new());
        assert!(present.contains(&"--mode".to_string()));
        assert!(present.contains(&"-e".to_string()));
    }

    #[test]
    fn classify_help_tokens_reports_missing_groups() {
        let help = "--mode rpc --model";
        let (_present, missing) = classify_help_tokens(help);
        assert!(missing.iter().any(|m| m.contains("--append-system-prompt")));
        assert!(missing.iter().any(|m| m.contains("--session")));
    }
}
