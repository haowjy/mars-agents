// qa-validated: mars-capability-cache-resolver

use std::collections::HashSet;

use crate::build::policy::{
    MatchedModelPolicy, PolicyInput, PolicySource, ResolvedField, matched_policy_string_override,
};
use crate::compiler::agents::HarnessKind;
use crate::config::AgentOverlay;
use crate::error::{ConfigError, MarsError};
use crate::models::ModelAlias;
use crate::routing::{self, RoutingInput};

#[derive(Debug)]
pub(super) struct HarnessResolution {
    pub(super) harness: ResolvedField<String>,
    pub(super) harness_order_position: Option<usize>,
    pub(super) candidates_tried: Vec<String>,
    pub(super) route_trace: routing::RoutingTrace,
    pub(super) model_cleared: bool,
    pub(super) is_experimental: bool,
    pub(super) resolved_harness: HarnessKind,
    pub(super) warnings: Vec<String>,
}

pub(super) struct HarnessEvidence<'a> {
    pub(super) routing: routing::RoutingEvidence<'a>,
    pub(super) model_token: &'a str,
    pub(super) model_source: PolicySource,
}

pub(super) fn resolve_harness(
    input: &PolicyInput<'_>,
    alias: Option<&ModelAlias>,
    overlay: Option<&AgentOverlay>,
    matched_policy: Option<&MatchedModelPolicy>,
    evidence: HarnessEvidence<'_>,
    probe_resolver: &mut dyn routing::ProbeResolver,
) -> Result<HarnessResolution, MarsError> {
    let mut warnings = Vec::new();
    let mut model_cleared = false;

    let profile_harness = input.profile.harness.as_ref().map(harness_kind_to_str);
    let overlay_harness = overlay
        .and_then(|entry| entry.harness.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let policy_harness = matched_policy_string_override(matched_policy, "harness");
    let overlay_policy_harness = policy_harness
        .as_ref()
        .filter(|decision| decision.source == PolicySource::OverlayModelPolicy)
        .cloned();
    let profile_policy_harness = policy_harness
        .as_ref()
        .filter(|decision| decision.source == PolicySource::ProfileModelPolicy)
        .cloned();
    let settings_policy_harness = policy_harness
        .as_ref()
        .filter(|decision| decision.source == PolicySource::SettingsModelPolicy)
        .cloned();
    let alias_harness = alias.and_then(|entry| entry.harness.as_deref());
    let normalized_config_default_harness = routing::normalize_config_default_harness(
        evidence.routing.config_default_harness,
        &mut warnings,
    );
    let model_from_cli = input.model_override.is_some();
    let mut selected_harness_order_position = None;
    let fixed_harness_selection = resolve_fixed_harness_selection(
        input,
        model_from_cli,
        overlay_harness,
        overlay_policy_harness,
        profile_harness,
        profile_policy_harness,
        settings_policy_harness,
        alias_harness,
    );
    let (harness, candidates_tried, route_trace, unavailable_profile_harness) = if let Some(
        selection,
    ) =
        fixed_harness_selection.clone()
    {
        let fixed_provider_for_order = routing::provider_for_order_for_fixed_harness(
            evidence.routing.provider_for_order,
            &selection.value,
        );
        let mut fixed_input =
            routing_input_from_evidence(&evidence, normalized_config_default_harness.as_deref());
        fixed_input.provider_for_order = fixed_provider_for_order;
        let fixed_assessment = routing::evaluate_fixed_harness_with_auth_and_probes(
            &fixed_input,
            &selection.value,
            probe_resolver,
            crate::models::harness::native_harness_authenticated,
        );
        let mut fixed_route_trace = routing::trace_for_fixed_harness(
            route_source_for_policy_source(selection.source),
            &selection.value,
            fixed_assessment.clone(),
            Vec::new(),
        );
        if selection.source == PolicySource::Profile
            && routing::acceptance::accept_route(
                &fixed_route_trace,
                evidence.routing.installed_harnesses,
                routing::acceptance::MatchPolicy::InstalledOnly,
            )
            .is_err()
        {
            warnings.push(format!(
                "profile harness '{}' not installed; pivoting via model-policies",
                selection.value
            ));
            let trace = evaluate_candidates(
                &evidence,
                normalized_config_default_harness.as_deref(),
                probe_resolver,
            );
            selected_harness_order_position = trace.selected_harness_order_position();
            warnings.extend(trace.selected_diagnostics().to_vec());
            let unavailable = routing::acceptance::accept_route(
                &trace,
                evidence.routing.installed_harnesses,
                routing::acceptance::MatchPolicy::InstalledOnly,
            )
            .err()
            .map(|_| selection.value.clone());
            let candidates_tried = trace.candidates_tried.clone();
            (
                ResolvedField {
                    value: trace.harness.clone(),
                    source: trace.source.into(),
                    matched_rule: None,
                },
                candidates_tried,
                trace,
                unavailable,
            )
        } else {
            if let Err(rejection) = routing::acceptance::accept_assessment(&fixed_assessment) {
                if rejection.is_not_installed() {
                    return Err(unavailable_fixed_harness_error(
                        selection.source.label(),
                        &selection.value,
                        evidence.routing.installed_harnesses,
                    ));
                }

                let skip_reason = match rejection {
                    routing::acceptance::RejectionReason::AssessmentFailed {
                        ref skip_reason,
                        ..
                    } => skip_reason.as_deref(),
                    _ => None,
                };
                if let Some(trace) = soft_fail_fixed_harness_no_model_match(
                    selection.source,
                    evidence.model_source,
                    skip_reason,
                    fixed_input,
                    &selection.value,
                    probe_resolver,
                ) {
                    fixed_route_trace = trace;
                    warnings.push(format!(
                        "{} model '{}' cannot run on {} harness '{}'; clearing model (harness override takes precedence).",
                        evidence.model_source.label(),
                        evidence.model_token,
                        selection.source.label(),
                        selection.value
                    ));
                    model_cleared = true;
                } else {
                    return Err(fixed_harness_constraint_error(
                        selection.source.label(),
                        &selection.value,
                        skip_reason,
                    ));
                }
            }
            let route_trace = fixed_route_trace;
            let candidates_tried = route_trace.candidates_tried.clone();
            (selection, candidates_tried, route_trace, None)
        }
    } else {
        let trace = evaluate_candidates(
            &evidence,
            normalized_config_default_harness.as_deref(),
            probe_resolver,
        );
        selected_harness_order_position = trace.selected_harness_order_position();
        warnings.extend(trace.selected_diagnostics().to_vec());
        let candidates_tried = trace.candidates_tried.clone();
        (
            ResolvedField {
                value: trace.harness.clone(),
                source: trace.source.into(),
                matched_rule: None,
            },
            candidates_tried,
            trace,
            None,
        )
    };

    if let Some(profile_harness) = unavailable_profile_harness {
        return Err(unavailable_profile_pivot_error(
            &profile_harness,
            &harness.value,
            evidence.routing.installed_harnesses,
        ));
    }

    let resolved_harness = HarnessKind::from_str(&harness.value).ok_or_else(|| {
        MarsError::Config(ConfigError::Invalid {
            message: format!(
                "resolved harness `{}` is invalid; expected one of: claude, codex, opencode, cursor, pi",
                harness.value
            ),
        })
    })?;

    Ok(HarnessResolution {
        is_experimental: harness.value == "cursor",
        resolved_harness,
        harness,
        harness_order_position: selected_harness_order_position,
        candidates_tried,
        route_trace,
        model_cleared,
        warnings,
    })
}

#[allow(clippy::too_many_arguments)]
fn resolve_fixed_harness_selection(
    input: &PolicyInput<'_>,
    model_from_cli: bool,
    overlay_harness: Option<&str>,
    overlay_policy_harness: Option<ResolvedField<String>>,
    profile_harness: Option<&str>,
    profile_policy_harness: Option<ResolvedField<String>>,
    settings_policy_harness: Option<ResolvedField<String>>,
    alias_harness: Option<&str>,
) -> Option<ResolvedField<String>> {
    if let Some(harness) = input.harness_override {
        return Some(ResolvedField {
            value: harness.to_string(),
            source: PolicySource::Cli,
            matched_rule: None,
        });
    }
    if let Some(harness) = overlay_harness {
        return Some(ResolvedField {
            value: harness.to_string(),
            source: PolicySource::Overlay,
            matched_rule: None,
        });
    }
    if let Some(harness) = overlay_policy_harness {
        return Some(harness);
    }
    if model_from_cli {
        if let Some(harness) = settings_policy_harness {
            return Some(harness);
        }
        if let Some(harness) = alias_harness {
            return Some(ResolvedField {
                value: harness.to_string(),
                source: PolicySource::Alias,
                matched_rule: None,
            });
        }
        return None;
    }

    if let Some(harness) = profile_harness {
        return Some(ResolvedField {
            value: harness.to_string(),
            source: PolicySource::Profile,
            matched_rule: None,
        });
    }
    if let Some(harness) = profile_policy_harness {
        return Some(harness);
    }
    if let Some(harness) = settings_policy_harness {
        return Some(harness);
    }
    alias_harness.map(|harness| ResolvedField {
        value: harness.to_string(),
        source: PolicySource::Alias,
        matched_rule: None,
    })
}

fn routing_input_from_evidence<'a>(
    evidence: &'a HarnessEvidence<'_>,
    normalized_config_default_harness: Option<&'a str>,
) -> RoutingInput<'a> {
    evidence
        .routing
        .routing_input_with_config_default_harness(normalized_config_default_harness)
}

fn evaluate_candidates(
    evidence: &HarnessEvidence<'_>,
    normalized_config_default_harness: Option<&str>,
    probe_resolver: &mut dyn routing::ProbeResolver,
) -> routing::RoutingTrace {
    routing::evaluate_candidates_with_auth_and_probes(
        &routing_input_from_evidence(evidence, normalized_config_default_harness),
        probe_resolver,
        crate::models::harness::native_harness_authenticated,
    )
}

fn route_source_for_policy_source(source: PolicySource) -> routing::RouteSource {
    match source {
        PolicySource::Cli => routing::RouteSource::Cli,
        PolicySource::Profile => routing::RouteSource::Profile,
        PolicySource::Alias => routing::RouteSource::Alias,
        PolicySource::ConfigOrder => routing::RouteSource::ConfigOrder,
        PolicySource::Config => routing::RouteSource::ConfigDefault,
        PolicySource::Default => routing::RouteSource::HardcodedDefault,
        PolicySource::Provider => routing::RouteSource::Provider,
        _ => routing::RouteSource::Provider,
    }
}

fn soft_fail_fixed_harness_no_model_match(
    harness_source: PolicySource,
    model_source: PolicySource,
    skip_reason: Option<&str>,
    fixed_input: RoutingInput<'_>,
    requested_harness: &str,
    probe_resolver: &mut dyn routing::ProbeResolver,
) -> Option<routing::RoutingTrace> {
    let should_retry = skip_reason == Some("no_model_match")
        && harness_source.precedence_rank() > model_source.precedence_rank();
    if !should_retry {
        return None;
    }

    let mut passthrough_input = fixed_input;
    passthrough_input.model_id = "";
    let assessment = routing::evaluate_fixed_harness_with_auth_and_probes(
        &passthrough_input,
        requested_harness,
        probe_resolver,
        crate::models::harness::native_harness_authenticated,
    );
    let route_trace = routing::trace_for_fixed_harness(
        route_source_for_policy_source(harness_source),
        requested_harness,
        assessment.clone(),
        Vec::new(),
    );
    routing::acceptance::accept_assessment(&assessment)
        .ok()
        .map(|_| route_trace)
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

fn fixed_harness_constraint_error(
    source: &str,
    requested_harness: &str,
    skip_reason: Option<&str>,
) -> MarsError {
    let detail = skip_reason.unwrap_or("unavailable");
    MarsError::Config(ConfigError::Invalid {
        message: format!(
            "{source} harness `{requested_harness}` cannot run requested model under model-first routing ({detail})",
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

    use indexmap::IndexMap;
    use std::path::Path;
    use std::sync::LazyLock;

    use crate::compiler::agents::AgentProfile;
    use crate::compiler::agents::HarnessOverrides;
    use crate::models::ModelSpec;
    use crate::models::probes::{CursorProbeResult, OpenCodeProbeResult, PiProbeResult};
    use crate::routing::MatchEvidence;

    static EMPTY_RUNTIME_ALIASES: LazyLock<IndexMap<String, ModelAlias>> =
        LazyLock::new(IndexMap::new);

    fn installed(names: &[&str]) -> HashSet<String> {
        names.iter().map(|name| (*name).to_string()).collect()
    }

    fn profile(harness: Option<HarnessKind>) -> AgentProfile {
        profile_with_model(harness, None)
    }

    fn profile_with_model(harness: Option<HarnessKind>, model: Option<&str>) -> AgentProfile {
        AgentProfile {
            name: None,
            description: None,
            harness,
            model: model.map(str::to_string),
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
            runtime_aliases: &EMPTY_RUNTIME_ALIASES,
            agent: None,
            profile,
            model_override,
            harness_override,
            effort_override: None,
            approval_override: None,
            sandbox_override: None,
            models_refresh: crate::models::ModelsRefreshControl::auto(),
        }
    }

    fn evidence<'a>(
        config_default_harness: Option<&'a str>,
        harness_order: Option<&'a [String]>,
        installed_harnesses: &'a HashSet<String>,
    ) -> HarnessEvidence<'a> {
        evidence_for_model(
            "gpt-5",
            "gpt-5",
            PolicySource::Alias,
            Some("openai"),
            None,
            installed_harnesses,
            config_default_harness,
            harness_order,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn evidence_for_model<'a>(
        model_id: &'a str,
        model_token: &'a str,
        model_source: PolicySource,
        provider_for_order: Option<&'a str>,
        provider_constraint: Option<&'a str>,
        installed_harnesses: &'a HashSet<String>,
        config_default_harness: Option<&'a str>,
        harness_order: Option<&'a [String]>,
    ) -> HarnessEvidence<'a> {
        HarnessEvidence {
            routing: routing::RoutingEvidence {
                model_id,
                provider_for_order,
                provider_constraint,
                settings_provider_order: None,
                config_default_harness,
                settings_harness_order: harness_order,
                installed_harnesses,
                linked_harnesses: None,
                opencode_probe_result: None,
                pi_probe_result: None,
                cursor_probe_result: None,
                catalog_model_slugs: None,
            },
            model_token,
            model_source,
        }
    }

    fn positive_opencode_probe() -> OpenCodeProbeResult {
        OpenCodeProbeResult {
            model_slugs: vec!["openai/gpt-5".to_string()],
            model_probe_success: true,
            error: None,
        }
    }

    #[derive(Default)]
    struct TestProbeResolver {
        opencode: Option<OpenCodeProbeResult>,
        pi: Option<PiProbeResult>,
        cursor: Option<CursorProbeResult>,
    }

    impl routing::ProbeResolver for TestProbeResolver {
        fn opencode_probe_result(&mut self) -> Option<OpenCodeProbeResult> {
            self.opencode.clone()
        }

        fn pi_probe_result(&mut self) -> Option<PiProbeResult> {
            self.pi.clone()
        }

        fn cursor_probe_result(&mut self) -> Option<CursorProbeResult> {
            self.cursor.clone()
        }
    }

    #[test]
    fn cli_override_is_explicit_and_skips_candidate_eval() {
        let installed = installed(&["codex", "pi"]);
        let profile = profile(Some(HarnessKind::Claude));
        let input = policy_input(&profile, None, Some("pi"));
        let mut probe_resolver = TestProbeResolver::default();

        let resolution = resolve_harness(
            &input,
            Some(&model_alias(Some("codex"))),
            None,
            None,
            evidence(None, None, &installed),
            &mut probe_resolver,
        )
        .expect("harness should resolve");

        assert_eq!(resolution.harness.value, "pi");
        assert_eq!(resolution.harness.source, PolicySource::Cli);
        assert_eq!(
            resolution.route_trace.selected_match_evidence(),
            MatchEvidence::Passthrough
        );
        assert_eq!(
            resolution.route_trace.selection_kind,
            routing::SelectionKind::Fixed
        );
        assert_eq!(resolution.candidates_tried, vec!["pi".to_string()]);
        assert_eq!(resolution.harness_order_position, None);
    }

    #[test]
    fn model_override_prefers_alias_harness() {
        let installed = installed(&["opencode"]);
        let profile = profile(Some(HarnessKind::Claude));
        let input = policy_input(&profile, Some("gptmini"), None);
        let mut probe_resolver = TestProbeResolver::default();

        let resolution = resolve_harness(
            &input,
            Some(&model_alias(Some("opencode"))),
            None,
            None,
            evidence(None, None, &installed),
            &mut probe_resolver,
        )
        .expect("harness should resolve");

        assert_eq!(resolution.harness.value, "opencode");
        assert_eq!(resolution.harness.source, PolicySource::Alias);
        assert_eq!(
            resolution.route_trace.selected_match_evidence(),
            MatchEvidence::Passthrough
        );
        assert_eq!(
            resolution.route_trace.selection_kind,
            routing::SelectionKind::Fixed
        );
        assert_eq!(resolution.candidates_tried, vec!["opencode".to_string()]);
    }

    #[test]
    fn profile_harness_precedes_alias_when_model_not_overridden() {
        let installed = installed(&["codex", "pi"]);
        let profile = profile(Some(HarnessKind::Pi));
        let input = policy_input(&profile, None, None);
        let mut probe_resolver = TestProbeResolver::default();

        let resolution = resolve_harness(
            &input,
            Some(&model_alias(Some("codex"))),
            None,
            None,
            evidence(None, None, &installed),
            &mut probe_resolver,
        )
        .expect("harness should resolve");

        assert_eq!(resolution.harness.value, "pi");
        assert_eq!(resolution.harness.source, PolicySource::Profile);
        assert_eq!(
            resolution.route_trace.selected_match_evidence(),
            MatchEvidence::Passthrough
        );
        assert_eq!(
            resolution.route_trace.selection_kind,
            routing::SelectionKind::Fixed
        );
        assert_eq!(resolution.candidates_tried, vec!["pi".to_string()]);
    }

    #[test]
    fn unavailable_profile_harness_pivots_to_candidate_evaluation() {
        let installed = installed(&["opencode"]);
        let profile = profile(Some(HarnessKind::Claude));
        let input = policy_input(&profile, None, None);
        let mut probe_resolver = TestProbeResolver {
            opencode: Some(positive_opencode_probe()),
            ..Default::default()
        };
        let evidence =
            evidence_for_model("gpt-5", "gpt-5", PolicySource::Alias, Some("openai"), None, &installed, None, None);

        let resolution = resolve_harness(&input, None, None, None, evidence, &mut probe_resolver)
            .expect("harness should pivot to opencode");

        assert_eq!(resolution.harness.value, "opencode");
        assert_eq!(resolution.harness.source, PolicySource::Provider);
        assert_eq!(
            resolution.route_trace.selected_match_evidence(),
            MatchEvidence::Confirmed
        );
        assert_eq!(resolution.candidates_tried, vec!["codex", "pi", "opencode"]);
        assert!(resolution.warnings.iter().any(|warning| {
            warning == "profile harness 'claude' not installed; pivoting via model-policies"
        }));
    }

    #[test]
    fn unavailable_profile_harness_pivots_when_installed_candidate_remains() {
        let installed = installed(&["opencode"]);
        let profile = profile(Some(HarnessKind::Claude));
        let input = policy_input(&profile, None, None);
        let mut probe_resolver = TestProbeResolver::default();

        let resolution = resolve_harness(
            &input,
            None,
            None,
            None,
            evidence(None, None, &installed),
            &mut probe_resolver,
        )
        .expect("profile harness should pivot to available candidates");
        assert_eq!(resolution.harness.value, "opencode");
        assert_eq!(resolution.harness.source, PolicySource::Provider);
        assert_eq!(
            resolution.route_trace.selected_match_evidence(),
            MatchEvidence::Passthrough
        );
    }

    #[test]
    fn unavailable_cli_harness_errors_without_pivoting() {
        let installed = installed(&["codex", "opencode"]);
        let profile = profile(Some(HarnessKind::Claude));
        let input = policy_input(&profile, None, Some("claude"));
        let mut probe_resolver = TestProbeResolver::default();

        let error = resolve_harness(
            &input,
            Some(&model_alias(Some("codex"))),
            None,
            None,
            evidence(None, None, &installed),
            &mut probe_resolver,
        )
        .expect_err("unavailable explicit harness should fail");
        let message = error.to_string();

        assert!(message.contains("cli harness `claude` is not installed"));
        assert!(message.contains("installed harnesses: codex, opencode"));
    }

    #[test]
    fn fixed_native_harness_rejects_incompatible_provider_constraint() {
        let installed = installed(&["codex"]);
        let profile = profile(None);
        let input = policy_input(&profile, None, Some("codex"));
        let mut probe_resolver = TestProbeResolver::default();
        let evidence = evidence_for_model(
            "gpt-5", "gpt-5", PolicySource::Alias, Some("openai"), Some("anthropic"), &installed, None, None,
        );

        let error = resolve_harness(&input, None, None, None, evidence, &mut probe_resolver)
            .expect_err("incompatible provider constraint should fail");
        let message = error.to_string();
        assert!(message.contains("cli harness `codex` cannot run requested model"));
        assert!(message.contains("provider_constraint_unsatisfied"));
    }

    #[test]
    fn auto_selection_maps_routing_trace_fields() {
        let installed = installed(&["pi"]);
        let order = vec!["pi".to_string(), "codex".to_string()];
        let profile = profile(None);
        let input = policy_input(&profile, None, None);
        let mut probe_resolver = TestProbeResolver::default();

        let resolution = resolve_harness(
            &input,
            None,
            None,
            None,
            evidence(None, Some(&order), &installed),
            &mut probe_resolver,
        )
        .expect("harness should resolve");

        assert_eq!(resolution.harness.value, "pi");
        assert_eq!(resolution.harness.source, PolicySource::ConfigOrder);
        assert_eq!(
            resolution.route_trace.selected_match_evidence(),
            MatchEvidence::Passthrough
        );
        assert_eq!(resolution.harness_order_position, Some(0));
        assert_eq!(resolution.candidates_tried, vec!["pi", "codex"]);
    }

    #[test]
    fn invalid_config_default_harness_still_warnings_on_fixed_selection() {
        let installed = installed(&["pi"]);
        let profile = profile(Some(HarnessKind::Pi));
        let input = policy_input(&profile, None, None);
        let mut probe_resolver = TestProbeResolver::default();

        let resolution = resolve_harness(
            &input,
            None,
            None,
            None,
            evidence(Some("bogus"), None, &installed),
            &mut probe_resolver,
        )
        .expect("harness should resolve");

        assert!(
            resolution
                .warnings
                .iter()
                .any(|warning| warning.contains("settings.default_harness `bogus` is invalid"))
        );
    }

    #[test]
    fn cli_fixed_harness_clears_lower_precedence_profile_model_on_no_model_match() {
        let installed = installed(&["opencode"]);
        let profile = profile_with_model(Some(HarnessKind::Claude), Some("opus"));
        let input = policy_input(&profile, None, Some("opencode"));
        let mut probe_resolver = TestProbeResolver {
            opencode: Some(positive_opencode_probe()),
            ..Default::default()
        };
        let evidence = evidence_for_model(
            "claude-opus-4-6", "opus", PolicySource::Profile,
            Some("anthropic"), Some("anthropic"), &installed, None, None,
        );

        let resolution = resolve_harness(&input, None, None, None, evidence, &mut probe_resolver)
            .expect("cli harness should soft-fail model mismatch and continue");

        assert_eq!(resolution.harness.value, "opencode");
        assert!(resolution.warnings.iter().any(|warning| warning.contains(
            "profile model 'opus' cannot run on cli harness 'opencode'; clearing model"
        )));
        assert!(resolution.model_cleared);
    }

    #[test]
    fn cli_fixed_harness_and_cli_model_no_model_match_is_hard_error() {
        let installed = installed(&["opencode"]);
        let profile = profile(None);
        let input = policy_input(&profile, Some("opus"), Some("opencode"));
        let mut probe_resolver = TestProbeResolver {
            opencode: Some(positive_opencode_probe()),
            ..Default::default()
        };
        let evidence = evidence_for_model(
            "claude-opus-4-6", "opus", PolicySource::Cli,
            Some("anthropic"), Some("anthropic"), &installed, None, None,
        );

        let err = resolve_harness(&input, None, None, None, evidence, &mut probe_resolver)
            .expect_err("same-precedence model mismatch must remain hard error");
        assert!(err.to_string().contains("no_model_match"));
    }

    #[test]
    fn fixed_harness_provider_constraint_unsatisfied_stays_hard_with_probe_match() {
        let installed = installed(&["opencode"]);
        let profile = profile_with_model(None, Some("gpt-5"));
        let input = policy_input(&profile, None, Some("opencode"));
        let mut probe_resolver = TestProbeResolver {
            opencode: Some(positive_opencode_probe()),
            ..Default::default()
        };
        let evidence = evidence_for_model(
            "gpt-5", "gpt-5", PolicySource::Profile,
            Some("openai"), Some("anthropic"), &installed, None, None,
        );

        let err = resolve_harness(&input, None, None, None, evidence, &mut probe_resolver)
            .expect_err("provider constraint failures must remain hard even when probe matches");
        let message = err.to_string();
        assert!(message.contains("provider_constraint_unsatisfied"));
        assert!(!message.contains("no_model_match"));
    }

}
