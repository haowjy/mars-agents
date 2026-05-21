// qa-validated: mars-capability-cache-resolver

use std::collections::HashSet;

use crate::build::policy::PolicyInput;
use crate::compiler::agents::HarnessKind;
use crate::error::{ConfigError, MarsError};
use crate::models::ModelAlias;
use crate::models::probes::OpenCodeProbeResult;
use crate::models::probes::PiProbeResult;
use crate::routing::{self, RouteConfidence, RoutingInput};

#[derive(Debug)]
pub(super) struct HarnessResolution {
    pub(super) harness: String,
    pub(super) source: &'static str,
    pub(super) harness_order_position: Option<usize>,
    pub(super) route_confidence: RouteConfidence,
    pub(super) candidates_tried: Vec<String>,
    pub(super) is_experimental: bool,
    pub(super) resolved_harness: HarnessKind,
    pub(super) warnings: Vec<String>,
}

pub(super) struct HarnessEvidence<'a> {
    pub(super) model_id: &'a str,
    pub(super) provider: Option<&'a str>,
    pub(super) config_default_harness: Option<&'a str>,
    pub(super) harness_order: Option<&'a [String]>,
    pub(super) installed_harnesses: &'a HashSet<String>,
    pub(super) linked_harnesses: Option<&'a [String]>,
    pub(super) opencode_probe_result: Option<&'a OpenCodeProbeResult>,
    pub(super) pi_probe_result: Option<&'a PiProbeResult>,
}

pub(super) fn resolve_harness(
    input: &PolicyInput<'_>,
    alias: Option<&ModelAlias>,
    evidence: HarnessEvidence<'_>,
) -> Result<HarnessResolution, MarsError> {
    let mut warnings = Vec::new();

    let profile_harness = input.profile.harness.as_ref().map(harness_kind_to_str);
    let alias_harness = alias.and_then(|entry| entry.harness.as_deref());
    let normalized_config_default_harness =
        routing::normalize_config_default_harness(evidence.config_default_harness, &mut warnings);
    let model_from_cli = input.model_override.is_some();
    let mut selected_harness_order_position = None;
    let mut fixed_harness_selection = None;
    let (harness, harness_source, route_confidence, candidates_tried) = if let Some(harness) =
        input.harness_override
    {
        fixed_harness_selection = Some(("cli", harness));
        (
            harness.to_string(),
            "cli",
            RouteConfidence::Explicit,
            vec![harness.to_string()],
        )
    } else if model_from_cli {
        if let Some(harness) = alias_harness {
            fixed_harness_selection = Some(("alias", harness));
            (
                harness.to_string(),
                "alias",
                RouteConfidence::Passthrough,
                Vec::new(),
            )
        } else {
            let trace =
                evaluate_candidates(&evidence, normalized_config_default_harness.as_deref());
            selected_harness_order_position = trace.harness_order_position;
            warnings.extend(trace.diagnostics);
            (
                trace.harness,
                trace.source.label(),
                trace.confidence,
                trace.candidates_tried,
            )
        }
    } else if let Some(harness) = profile_harness {
        if evidence.installed_harnesses.contains(harness) {
            (
                harness.to_string(),
                "profile",
                RouteConfidence::Passthrough,
                Vec::new(),
            )
        } else {
            warnings.push(format!(
                "profile harness '{harness}' not installed; pivoting via model-policies"
            ));
            let trace =
                evaluate_candidates(&evidence, normalized_config_default_harness.as_deref());
            selected_harness_order_position = trace.harness_order_position;
            warnings.extend(trace.diagnostics);

            if !evidence.installed_harnesses.contains(&trace.harness) {
                return Err(unavailable_profile_pivot_error(
                    harness,
                    &trace.harness,
                    evidence.installed_harnesses,
                ));
            }

            (
                trace.harness,
                trace.source.label(),
                trace.confidence,
                trace.candidates_tried,
            )
        }
    } else if let Some(harness) = alias_harness {
        fixed_harness_selection = Some(("alias", harness));
        (
            harness.to_string(),
            "alias",
            RouteConfidence::Passthrough,
            Vec::new(),
        )
    } else {
        let trace = evaluate_candidates(&evidence, normalized_config_default_harness.as_deref());
        selected_harness_order_position = trace.harness_order_position;
        warnings.extend(trace.diagnostics);
        (
            trace.harness,
            trace.source.label(),
            trace.confidence,
            trace.candidates_tried,
        )
    };

    let resolved_harness = HarnessKind::from_str(&harness).ok_or_else(|| {
        MarsError::Config(ConfigError::Invalid {
            message: format!(
                "resolved harness `{harness}` is invalid; expected one of: claude, codex, opencode, cursor, pi"
            ),
        })
    })?;

    if let Some((source, requested_harness)) = fixed_harness_selection
        && !evidence.installed_harnesses.contains(requested_harness)
    {
        return Err(unavailable_fixed_harness_error(
            source,
            requested_harness,
            evidence.installed_harnesses,
        ));
    }

    Ok(HarnessResolution {
        is_experimental: harness == "cursor",
        resolved_harness,
        harness,
        source: harness_source,
        harness_order_position: selected_harness_order_position,
        route_confidence,
        candidates_tried,
        warnings,
    })
}

fn evaluate_candidates(
    evidence: &HarnessEvidence<'_>,
    normalized_config_default_harness: Option<&str>,
) -> routing::RoutingTrace {
    routing::evaluate_candidates(&RoutingInput {
        model_id: evidence.model_id,
        provider: evidence.provider,
        settings_harness_order: evidence.harness_order,
        config_default_harness: normalized_config_default_harness,
        installed_harnesses: evidence.installed_harnesses,
        linked_harnesses: evidence.linked_harnesses,
        opencode_probe_result: evidence.opencode_probe_result,
        pi_probe_result: evidence.pi_probe_result,
    })
}

fn unavailable_profile_pivot_error(
    requested_harness: &str,
    selected_harness: &str,
    installed_harnesses: &HashSet<String>,
) -> MarsError {
    MarsError::Config(ConfigError::Invalid {
        message: format!(
            "profile harness `{requested_harness}` is not installed and no installed fallback harness is available (selected `{selected_harness}`); installed harnesses: {}",
            format_installed_harnesses(installed_harnesses)
        ),
    })
}

fn unavailable_fixed_harness_error(
    source: &str,
    requested_harness: &str,
    installed_harnesses: &HashSet<String>,
) -> MarsError {
    MarsError::Config(ConfigError::Invalid {
        message: format!(
            "{source} harness `{requested_harness}` is not installed; installed harnesses: {}",
            format_installed_harnesses(installed_harnesses)
        ),
    })
}

fn format_installed_harnesses(installed_harnesses: &HashSet<String>) -> String {
    let mut names = installed_harnesses
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    names.sort_unstable();

    if names.is_empty() {
        "(none)".to_string()
    } else {
        names.join(", ")
    }
}

pub(super) fn harness_kind_to_str(harness: &HarnessKind) -> &'static str {
    match harness {
        HarnessKind::Claude => "claude",
        HarnessKind::Codex => "codex",
        HarnessKind::OpenCode => "opencode",
        HarnessKind::Cursor => "cursor",
        HarnessKind::Pi => "pi",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap;
    use std::path::Path;

    use crate::compiler::agents::AgentProfile;
    use crate::compiler::agents::HarnessOverrides;
    use crate::models::ModelSpec;

    fn installed(names: &[&str]) -> HashSet<String> {
        names.iter().map(|name| (*name).to_string()).collect()
    }

    fn profile(harness: Option<HarnessKind>) -> AgentProfile {
        AgentProfile {
            name: None,
            description: None,
            harness,
            model: None,
            mode: None,
            model_invocable: false,
            approval: None,
            sandbox: None,
            effort: None,
            autocompact: None,
            autocompact_pct: None,
            skills: Vec::new(),
            tools: Vec::new(),
            tools_denied: Vec::new(),
            disallowed_tools: Vec::new(),
            mcp_tools: Vec::new(),
            harness_overrides: HarnessOverrides::default(),
            model_policies: Vec::new(),
            fanout: Vec::new(),
        }
    }

    fn model_alias(harness: Option<&str>) -> ModelAlias {
        ModelAlias {
            harness: harness.map(str::to_string),
            description: None,
            default_effort: None,
            autocompact: None,
            autocompact_pct: None,
            spec: ModelSpec::Pinned {
                model: "gpt-5".to_string(),
                provider: Some("openai".to_string()),
            },
        }
    }

    fn policy_input<'a>(
        profile: &'a AgentProfile,
        model_override: Option<&'a str>,
        harness_override: Option<&'a str>,
    ) -> PolicyInput<'a> {
        PolicyInput {
            project_root: Path::new("."),
            profile,
            model_override,
            config_default_model: None,
            harness_override,
            effort_override: None,
            approval_override: None,
            sandbox_override: None,
        }
    }

    fn evidence<'a>(
        config_default_harness: Option<&'a str>,
        harness_order: Option<&'a [String]>,
        installed_harnesses: &'a HashSet<String>,
    ) -> HarnessEvidence<'a> {
        HarnessEvidence {
            model_id: "gpt-5",
            provider: Some("openai"),
            config_default_harness,
            harness_order,
            installed_harnesses,
            linked_harnesses: None,
            opencode_probe_result: None,
            pi_probe_result: None,
        }
    }

    fn positive_opencode_probe() -> OpenCodeProbeResult {
        OpenCodeProbeResult {
            providers: HashMap::from([("openai".to_string(), true)]),
            model_slugs: vec!["openai/gpt-5".to_string()],
            provider_probe_success: true,
            model_probe_success: true,
            error: None,
        }
    }

    #[test]
    fn cli_override_is_explicit_and_skips_candidate_eval() {
        let installed = installed(&["codex", "pi"]);
        let profile = profile(Some(HarnessKind::Claude));
        let input = policy_input(&profile, None, Some("pi"));

        let resolution = resolve_harness(
            &input,
            Some(&model_alias(Some("codex"))),
            evidence(None, None, &installed),
        )
        .expect("harness should resolve");

        assert_eq!(resolution.harness, "pi");
        assert_eq!(resolution.source, "cli");
        assert_eq!(resolution.route_confidence, RouteConfidence::Explicit);
        assert_eq!(resolution.candidates_tried, vec!["pi".to_string()]);
        assert_eq!(resolution.harness_order_position, None);
    }

    #[test]
    fn model_override_prefers_alias_harness() {
        let installed = installed(&["codex"]);
        let profile = profile(Some(HarnessKind::Claude));
        let input = policy_input(&profile, Some("gptmini"), None);

        let resolution = resolve_harness(
            &input,
            Some(&model_alias(Some("codex"))),
            evidence(None, None, &installed),
        )
        .expect("harness should resolve");

        assert_eq!(resolution.harness, "codex");
        assert_eq!(resolution.source, "alias");
        assert_eq!(resolution.route_confidence, RouteConfidence::Passthrough);
        assert!(resolution.candidates_tried.is_empty());
    }

    #[test]
    fn profile_harness_precedes_alias_when_model_not_overridden() {
        let installed = installed(&["codex", "pi"]);
        let profile = profile(Some(HarnessKind::Pi));
        let input = policy_input(&profile, None, None);

        let resolution = resolve_harness(
            &input,
            Some(&model_alias(Some("codex"))),
            evidence(None, None, &installed),
        )
        .expect("harness should resolve");

        assert_eq!(resolution.harness, "pi");
        assert_eq!(resolution.source, "profile");
        assert_eq!(resolution.route_confidence, RouteConfidence::Passthrough);
        assert!(resolution.candidates_tried.is_empty());
    }

    #[test]
    fn unavailable_profile_harness_pivots_to_candidate_evaluation() {
        let installed = installed(&["opencode"]);
        let profile = profile(Some(HarnessKind::Claude));
        let input = policy_input(&profile, None, None);
        let opencode_probe = positive_opencode_probe();
        let evidence = HarnessEvidence {
            model_id: "gpt-5",
            provider: Some("openai"),
            config_default_harness: None,
            harness_order: None,
            installed_harnesses: &installed,
            linked_harnesses: None,
            opencode_probe_result: Some(&opencode_probe),
            pi_probe_result: None,
        };

        let resolution =
            resolve_harness(&input, None, evidence).expect("harness should pivot to opencode");

        assert_eq!(resolution.harness, "opencode");
        assert_eq!(resolution.source, "provider");
        assert_eq!(resolution.route_confidence, RouteConfidence::Likely);
        assert_eq!(resolution.candidates_tried, vec!["codex", "pi", "opencode"]);
        assert!(resolution.warnings.iter().any(|warning| {
            warning == "profile harness 'claude' not installed; pivoting via model-policies"
        }));
    }

    #[test]
    fn unavailable_profile_harness_errors_when_no_installed_fallback_is_available() {
        let installed = installed(&["opencode"]);
        let profile = profile(Some(HarnessKind::Claude));
        let input = policy_input(&profile, None, None);

        let error = resolve_harness(&input, None, evidence(None, None, &installed))
            .expect_err("unavailable profile harness should fail without an installed fallback");
        let message = error.to_string();

        assert!(message.contains("profile harness `claude` is not installed"));
        assert!(message.contains("selected `claude`"));
        assert!(message.contains("installed harnesses: opencode"));
    }

    #[test]
    fn unavailable_cli_harness_errors_without_pivoting() {
        let installed = installed(&["codex", "opencode"]);
        let profile = profile(Some(HarnessKind::Claude));
        let input = policy_input(&profile, None, Some("claude"));

        let error = resolve_harness(
            &input,
            Some(&model_alias(Some("codex"))),
            evidence(None, None, &installed),
        )
        .expect_err("unavailable explicit harness should fail");
        let message = error.to_string();

        assert!(message.contains("cli harness `claude` is not installed"));
        assert!(message.contains("installed harnesses: codex, opencode"));
    }

    #[test]
    fn auto_selection_maps_routing_trace_fields() {
        let installed = installed(&["pi"]);
        let order = vec!["pi".to_string(), "codex".to_string()];
        let profile = profile(None);
        let input = policy_input(&profile, None, None);

        let resolution = resolve_harness(&input, None, evidence(None, Some(&order), &installed))
            .expect("harness should resolve");

        assert_eq!(resolution.harness, "pi");
        assert_eq!(resolution.source, "config-order");
        assert_eq!(resolution.route_confidence, RouteConfidence::Passthrough);
        assert_eq!(resolution.harness_order_position, Some(0));
        assert_eq!(resolution.candidates_tried, vec!["pi".to_string()]);
    }

    #[test]
    fn invalid_config_default_harness_still_warnings_on_fixed_selection() {
        let installed = installed(&["pi"]);
        let profile = profile(Some(HarnessKind::Pi));
        let input = policy_input(&profile, None, None);

        let resolution = resolve_harness(&input, None, evidence(Some("bogus"), None, &installed))
            .expect("harness should resolve");

        assert!(
            resolution
                .warnings
                .iter()
                .any(|warning| warning.contains("settings.default_harness `bogus` is invalid"))
        );
    }
}
