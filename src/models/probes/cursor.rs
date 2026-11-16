use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use mars_schema::agent::EffortLevel;
use mars_schema::effort::Levels;
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

pub fn normalize_slug(s: &str) -> String {
    s.to_ascii_lowercase().replace('.', "-")
}

pub fn find_cursor_prefix_matches<'a>(model_id: &str, slugs: &'a [String]) -> Vec<&'a str> {
    let normalized_model = normalize_slug(model_id);
    slugs
        .iter()
        .filter(|slug| {
            let normalized_slug = normalize_slug(slug);
            normalized_slug == normalized_model
                || normalized_slug.starts_with(&format!("{normalized_model}-"))
        })
        .map(String::as_str)
        .collect()
}

/// The effort tiers Cursor lists for one model: each slug folds one reasoning level into the
/// model name (`gpt-5.5-high`), and the bare slug (`gpt-5.5`) stands for `medium`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorTiers {
    /// The slug for each level the model lists. A level with several slugs keeps the first, or a
    /// Claude `-thinking-` one.
    pub slug_by_level: BTreeMap<EffortLevel, String>,
    /// Every slug of the model, for diagnostics.
    pub candidate_slugs: Vec<String>,
}

impl CursorTiers {
    /// The levels the model lists, as the harness's own set for this model.
    pub fn levels(&self) -> Levels {
        Levels::only(self.slug_by_level.keys().copied())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorTiersError {
    NoProbeSlugs,
    NoModelPrefixMatch,
}

/// The tiers `slugs` list for `model_id`.
pub fn cursor_tiers(model_id: &str, slugs: &[String]) -> Result<CursorTiers, CursorTiersError> {
    if slugs.is_empty() {
        return Err(CursorTiersError::NoProbeSlugs);
    }
    let prefix_matches = find_cursor_prefix_matches(model_id, slugs);
    if prefix_matches.is_empty() {
        return Err(CursorTiersError::NoModelPrefixMatch);
    }
    let normalized_model = normalize_slug(model_id);
    let mut slug_by_level = BTreeMap::new();
    for level in EffortLevel::LADDER {
        let matches: Vec<&str> = prefix_matches
            .iter()
            .copied()
            .filter(|slug| slug_matches_level(&normalized_model, &normalize_slug(slug), level))
            .collect();
        if !matches.is_empty() {
            slug_by_level.insert(
                level,
                choose_cursor_slug(&normalized_model, matches).to_string(),
            );
        }
    }
    Ok(CursorTiers {
        slug_by_level,
        candidate_slugs: prefix_matches.iter().map(|slug| slug.to_string()).collect(),
    })
}

fn slug_matches_level(normalized_model: &str, normalized_slug: &str, level: EffortLevel) -> bool {
    if normalized_slug == normalized_model {
        return level == EffortLevel::Medium;
    }
    let level = level.as_str();
    let Some(suffix) = normalized_slug
        .strip_prefix(normalized_model)
        .and_then(|rest| rest.strip_prefix('-'))
    else {
        return false;
    };
    suffix == level
        || suffix.ends_with(&format!("-{level}"))
        || suffix.contains(&format!("-{level}-"))
}

fn choose_cursor_slug<'a>(normalized_model: &str, matches: Vec<&'a str>) -> &'a str {
    if matches.len() == 1 {
        return matches[0];
    }

    if normalized_model.starts_with("claude")
        && let Some(thinking) = matches
            .iter()
            .copied()
            .find(|slug| normalize_slug(slug).contains("-thinking-"))
    {
        return thinking;
    }

    matches[0]
}
