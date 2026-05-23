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

/// Parse `cursor agent --list-models` output into raw slug strings.
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorEffortResolution {
    pub slug: String,
    pub candidate_slugs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorEffortResolutionError {
    NoProbeSlugs,
    NoModelPrefixMatch,
    NoEffortMatch { model_id: String, effort: String },
}

/// Resolve a Cursor catalog slug from base model id + effort tier.
pub fn resolve_cursor_effort_slug(
    model_id: &str,
    effort: &str,
    slugs: &[String],
) -> Result<CursorEffortResolution, CursorEffortResolutionError> {
    if slugs.is_empty() {
        return Err(CursorEffortResolutionError::NoProbeSlugs);
    }

    let prefix_matches = find_cursor_prefix_matches(model_id, slugs);
    if prefix_matches.is_empty() {
        return Err(CursorEffortResolutionError::NoModelPrefixMatch);
    }

    let normalized_model = normalize_slug(model_id);
    let normalized_effort = normalize_slug(effort);

    if cursor_effort_is_default_tier(&normalized_effort) {
        if let Some(bare_slug) = prefix_matches
            .iter()
            .find(|slug| normalize_slug(slug) == normalized_model)
        {
            let candidate_slugs = slugs
                .iter()
                .filter(|slug| {
                    let normalized_slug = normalize_slug(slug);
                    normalized_slug == normalized_model
                        || normalized_slug.starts_with(&format!("{normalized_model}-"))
                })
                .cloned()
                .collect();
            return Ok(CursorEffortResolution {
                slug: (*bare_slug).to_string(),
                candidate_slugs,
            });
        }

        return Err(CursorEffortResolutionError::NoEffortMatch {
            model_id: model_id.to_string(),
            effort: effort.to_string(),
        });
    }

    let effort_matches: Vec<&str> = prefix_matches
        .into_iter()
        .filter(|slug| {
            slug_matches_effort(&normalized_model, &normalize_slug(slug), &normalized_effort)
        })
        .collect();

    if effort_matches.is_empty() {
        return Err(CursorEffortResolutionError::NoEffortMatch {
            model_id: model_id.to_string(),
            effort: effort.to_string(),
        });
    }

    let chosen = choose_cursor_effort_slug(&normalized_model, effort_matches);
    let candidate_slugs = slugs
        .iter()
        .filter(|slug| {
            let normalized_slug = normalize_slug(slug);
            normalized_slug == normalized_model
                || normalized_slug.starts_with(&format!("{normalized_model}-"))
        })
        .cloned()
        .collect();

    Ok(CursorEffortResolution {
        slug: chosen.to_string(),
        candidate_slugs,
    })
}

/// Cursor often exposes the default effort tier as an unsuffixed slug (e.g. `gpt-5.5`),
/// not `gpt-5.5-medium`. Treat medium/none/auto as that default tier.
fn cursor_effort_is_default_tier(normalized_effort: &str) -> bool {
    matches!(normalized_effort, "auto" | "default" | "medium" | "none")
}

fn slug_matches_effort(
    normalized_model: &str,
    normalized_slug: &str,
    normalized_effort: &str,
) -> bool {
    if normalized_slug == normalized_model {
        return cursor_effort_is_default_tier(normalized_effort);
    }

    let Some(suffix) = normalized_slug
        .strip_prefix(normalized_model)
        .and_then(|rest| rest.strip_prefix('-'))
    else {
        return false;
    };

    suffix == normalized_effort
        || suffix.ends_with(&format!("-{normalized_effort}"))
        || suffix.contains(&format!("-{normalized_effort}-"))
}

fn choose_cursor_effort_slug<'a>(normalized_model: &str, matches: Vec<&'a str>) -> &'a str {
    if matches.len() == 1 {
        return matches[0];
    }

    let is_claude = normalized_model.starts_with("claude");
    if is_claude {
        if let Some(thinking) = matches
            .iter()
            .copied()
            .find(|slug| normalize_slug(slug).contains("-thinking-"))
        {
            return thinking;
        }
    }

    matches[0]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_models_basic() {
        let output = r#"gpt-5.5-high - GPT 5.5 (High)
gpt-5.5-low - GPT 5.5 (Low)
claude-opus-4-7-thinking-high - Claude Opus 4.7"#;

        let slugs = parse_cursor_models_output(output);

        assert_eq!(
            slugs,
            vec![
                "gpt-5.5-high".to_string(),
                "gpt-5.5-low".to_string(),
                "claude-opus-4-7-thinking-high".to_string()
            ]
        );
    }

    #[test]
    fn test_parse_models_filters_fast() {
        let output = r#"gpt-5.5-high - GPT 5.5 (High)
gpt-5.5-fast - GPT 5.5 (Fast)"#;
        let slugs = parse_cursor_models_output(output);
        assert_eq!(slugs, vec!["gpt-5.5-high".to_string()]);
    }

    #[test]
    fn test_parse_models_skips_header_and_tip() {
        let output = r#"Available models

gpt-5.5-high - GPT 5.5 (High)

Tip: use --model <id> to select"#;
        let slugs = parse_cursor_models_output(output);
        assert_eq!(slugs, vec!["gpt-5.5-high".to_string()]);
    }

    #[test]
    fn test_parse_models_strips_ansi() {
        let slugs = parse_cursor_models_output("\x1b[32mgpt-5.5-high - GPT 5.5\x1b[0m");
        assert_eq!(slugs, vec!["gpt-5.5-high".to_string()]);
    }

    #[test]
    fn test_find_prefix_matches() {
        let slugs = vec![
            "gpt-5.5-high".to_string(),
            "gpt-5.5-low".to_string(),
            "claude-opus-4-7".to_string(),
        ];
        let matches = find_cursor_prefix_matches("gpt-5.5", &slugs);
        assert_eq!(matches, vec!["gpt-5.5-high", "gpt-5.5-low"]);
    }

    #[test]
    fn test_find_prefix_matches_requires_boundary() {
        let slugs = vec![
            "gpt-5.5-high".to_string(),
            "gpt-55-high".to_string(),
            "gpt-5".to_string(),
        ];

        let matches = find_cursor_prefix_matches("gpt-5", &slugs);

        assert_eq!(matches, vec!["gpt-5.5-high", "gpt-5"]);
    }

    #[test]
    fn test_normalize_slug() {
        assert_eq!(normalize_slug("GPT.5.5-High"), "gpt-5-5-high");
    }

    #[test]
    fn test_resolve_effort_slug_openai() {
        let slugs = vec![
            "gpt-5.5-high".to_string(),
            "gpt-5.5-low".to_string(),
            "gpt-55-high".to_string(),
        ];
        let resolution = resolve_cursor_effort_slug("gpt-5.5", "high", &slugs).unwrap();
        assert_eq!(resolution.slug, "gpt-5.5-high");
    }

    #[test]
    fn test_resolve_effort_slug_prefers_thinking_for_claude() {
        let slugs = vec![
            "claude-opus-4-7-high".to_string(),
            "claude-opus-4-7-thinking-high".to_string(),
        ];
        let resolution = resolve_cursor_effort_slug("claude-opus-4-7", "high", &slugs).unwrap();
        assert_eq!(resolution.slug, "claude-opus-4-7-thinking-high");
    }

    #[test]
    fn test_resolve_effort_slug_medium_uses_unsuffixed_base_slug() {
        let slugs = vec![
            "gpt-5.5".to_string(),
            "gpt-5.5-high".to_string(),
            "gpt-5.5-low".to_string(),
        ];
        for effort in ["medium", "none", "auto"] {
            let resolution = resolve_cursor_effort_slug("gpt-5.5", effort, &slugs).unwrap();
            assert_eq!(
                resolution.slug, "gpt-5.5",
                "effort {effort} should resolve to base slug"
            );
        }
    }

    #[test]
    fn test_resolve_effort_slug_medium_requires_base_slug_in_catalog() {
        let slugs = vec!["gpt-5.5-high".to_string(), "gpt-5.5-low".to_string()];
        let err = resolve_cursor_effort_slug("gpt-5.5", "medium", &slugs).unwrap_err();
        assert_eq!(
            err,
            CursorEffortResolutionError::NoEffortMatch {
                model_id: "gpt-5.5".to_string(),
                effort: "medium".to_string(),
            }
        );
    }

    #[test]
    fn test_resolve_effort_slug_no_effort_match() {
        let slugs = vec!["gpt-5.5-low".to_string()];
        let err = resolve_cursor_effort_slug("gpt-5.5", "high", &slugs).unwrap_err();
        assert_eq!(
            err,
            CursorEffortResolutionError::NoEffortMatch {
                model_id: "gpt-5.5".to_string(),
                effort: "high".to_string(),
            }
        );
    }

    #[test]
    fn test_probe_result_round_trip() {
        let result = CursorProbeResult {
            slugs: vec!["gpt-5.5-high".to_string()],
            model_probe_success: true,
            error: None,
        };
        let json = serde_json::to_string(&result).unwrap();
        let back: CursorProbeResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.slugs, result.slugs);
        assert!(back.model_probe_success);
        assert_eq!(back.error, None);
    }
}
