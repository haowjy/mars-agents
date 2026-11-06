use std::collections::HashSet;
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
    /// `--list-models` completed, independently of help-surface compatibility.
    #[serde(default)]
    pub model_probe_success: bool,
    pub help_surface_tokens_present: Vec<String>,
    pub help_surface_tokens_missing: Vec<String>,
    #[serde(default)]
    pub model_slugs: HashSet<String>,
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
    let compatible = missing.is_empty();

    let list_models_output = match run_command(&binary_path, &["--list-models"], timeout) {
        Ok(stdout) => stdout,
        Err(error) => {
            return PiProbeResult {
                binary_path: binary_path_text,
                version: first_non_empty_line(&version_output),
                compatible,
                model_probe_success: false,
                help_surface_tokens_present: present,
                help_surface_tokens_missing: missing,
                model_slugs: HashSet::new(),
                error: Some(format!("pi --list-models probe failed: {error}")),
            };
        }
    };

    PiProbeResult {
        binary_path: binary_path_text,
        version: first_non_empty_line(&version_output),
        compatible,
        model_probe_success: true,
        help_surface_tokens_present: present,
        help_surface_tokens_missing: missing,
        model_slugs: parse_models_output(&list_models_output),
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
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "stderr capture unavailable".to_string())?;
    let stdout_reader = thread::spawn(move || read_stream_to_end(stdout, "stdout"));
    let stderr_reader = thread::spawn(move || read_stream_to_end(stderr, "stderr"));

    match child
        .wait_timeout(timeout)
        .map_err(|error| format!("wait failed: {error}"))?
    {
        Some(status) if status.success() => {
            let stdout = stdout_reader
                .join()
                .map_err(|_| "stdout reader panicked".to_string())??;
            let stderr = stderr_reader
                .join()
                .map_err(|_| "stderr reader panicked".to_string())??;
            effective_probe_output(stdout, stderr)
        }
        Some(status) => {
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            Err(format!("exit code {}", status.code().unwrap_or(-1)))
        }
        None => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            Err("timeout".to_string())
        }
    }
}

fn read_stream_to_end(mut stream: impl Read, label: &'static str) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    stream
        .read_to_end(&mut output)
        .map(|_| output)
        .map_err(|error| format!("{label} read failed: {error}"))
}

/// Pi 0.75+ may print CLI text to stderr only; prefer stdout when non-empty.
fn effective_probe_output(stdout: Vec<u8>, stderr: Vec<u8>) -> Result<String, String> {
    let stdout =
        String::from_utf8(stdout).map_err(|error| format!("invalid utf8 stdout: {error}"))?;
    let stderr =
        String::from_utf8(stderr).map_err(|error| format!("invalid utf8 stderr: {error}"))?;
    Ok(if stdout.trim().is_empty() {
        stderr
    } else {
        stdout
    })
}

fn first_non_empty_line(output: &str) -> Option<String> {
    output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
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

fn parse_models_output(output: &str) -> HashSet<String> {
    let mut model_slugs = HashSet::new();
    for raw_line in output.lines() {
        let line = strip_ansi(raw_line.trim());
        if line.is_empty() || is_separator_line(&line) {
            continue;
        }

        if let Some((provider, model_id)) = parse_table_row(&line) {
            model_slugs.insert(format!("{provider}/{model_id}"));
            continue;
        }

        if let Some((provider, model_id)) = parse_slug_row(&line) {
            model_slugs.insert(format!("{provider}/{model_id}"));
        }
    }

    model_slugs
}

fn parse_table_row(line: &str) -> Option<(String, String)> {
    let normalized = line.replace('│', "|");
    let has_table_separators = normalized.contains('|');
    let columns: Vec<String> = if has_table_separators {
        normalized
            .split('|')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(str::to_string)
            .collect()
    } else {
        normalized
            .split_whitespace()
            .map(str::to_string)
            .collect::<Vec<_>>()
    };

    if columns.len() < 2 {
        return None;
    }

    let provider = columns[0].trim().to_ascii_lowercase();
    let model_id = columns[1].trim().to_string();
    if is_header_cell(&provider) || is_header_cell(&model_id) {
        return None;
    }
    if provider.is_empty() || model_id.is_empty() {
        return None;
    }
    if model_id.ends_with(':') {
        return None;
    }
    if !provider
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.')
    {
        return None;
    }

    Some((provider, model_id))
}

fn parse_slug_row(line: &str) -> Option<(String, String)> {
    if is_header_cell(line) {
        return None;
    }
    let (provider, model_id) = line.split_once('/')?;
    let provider = provider.trim().to_ascii_lowercase();
    let model_id = model_id.trim().to_string();
    if provider.is_empty() || model_id.is_empty() {
        return None;
    }
    if !provider
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.')
    {
        return None;
    }
    Some((provider, model_id))
}

fn is_header_cell(cell: &str) -> bool {
    matches!(
        cell.trim().to_ascii_lowercase().as_str(),
        "provider" | "providers" | "model" | "models" | "id" | "name"
    )
}

fn is_separator_line(line: &str) -> bool {
    line.chars().all(|ch| {
        ch.is_whitespace()
            || matches!(
                ch,
                '-' | '='
                    | '+'
                    | '|'
                    | '│'
                    | '┌'
                    | '┐'
                    | '└'
                    | '┘'
                    | '├'
                    | '┤'
                    | '┬'
                    | '┴'
                    | '┼'
                    | '─'
                    | '━'
            )
    })
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
