// qa-validated: mars-capability-cache-resolver

use std::collections::HashSet;

use crate::build::policy::PolicyInput;
use crate::compiler::agents::HarnessKind;
use crate::error::{ConfigError, MarsError};
use crate::models::ModelAlias;
use crate::models::probes::OpenCodeProbeResult;
use crate::models::probes::PiProbeResult;
use crate::routing::{self, RouteConfidence, RoutingInput};

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
    let (harness, harness_source, route_confidence, candidates_tried) =
        if let Some(harness) = input.harness_override {
            (
                harness.to_string(),
                "cli",
                RouteConfidence::Explicit,
                vec![harness.to_string()],
            )
        } else if model_from_cli {
            if let Some(harness) = alias_harness {
                (
                    harness.to_string(),
                    "alias",
                    RouteConfidence::Passthrough,
                    Vec::new(),
                )
            } else {
                let trace = routing::evaluate_candidates(&RoutingInput {
                    model_id: evidence.model_id,
                    provider: evidence.provider,
                    settings_harness_order: evidence.harness_order,
                    config_default_harness: normalized_config_default_harness.as_deref(),
                    installed_harnesses: evidence.installed_harnesses,
                    linked_harnesses: evidence.linked_harnesses,
                    opencode_probe_result: evidence.opencode_probe_result,
                    pi_probe_result: evidence.pi_probe_result,
                });
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
            (
                harness.to_string(),
                "profile",
                RouteConfidence::Passthrough,
                Vec::new(),
            )
        } else if let Some(harness) = alias_harness {
            (
                harness.to_string(),
                "alias",
                RouteConfidence::Passthrough,
                Vec::new(),
            )
        } else {
            let trace = routing::evaluate_candidates(&RoutingInput {
                model_id: evidence.model_id,
                provider: evidence.provider,
                settings_harness_order: evidence.harness_order,
                config_default_harness: normalized_config_default_harness.as_deref(),
                installed_harnesses: evidence.installed_harnesses,
                linked_harnesses: evidence.linked_harnesses,
                opencode_probe_result: evidence.opencode_probe_result,
                pi_probe_result: evidence.pi_probe_result,
            });
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
