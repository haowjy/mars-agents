use crate::build::policy::{
    MatchedModelPolicy, PolicyInput, PolicySource, ResolvedField, matched_policy_string_override,
};
use crate::compiler::agents::HarnessKind;
use crate::config::AgentOverlay;
use crate::error::{ConfigError, MarsError};
use crate::models::ModelAlias;
use crate::routing;

#[derive(Debug)]
pub(super) struct HarnessResolution {
    pub(super) harness: ResolvedField<String>,
    pub(super) harness_order_position: Option<usize>,
    pub(super) candidates_tried: Vec<String>,
    pub(super) route_trace: routing::RoutingTrace,
    pub(super) warnings: Vec<String>,
}

pub(super) enum HarnessAttempt {
    Selected(HarnessResolution),
    Exhausted(routing::RoutingTrace),
}

pub(super) struct HarnessEvidence<'a> {
    pub(super) routing: routing::RoutingEvidence<'a>,
}

pub(super) fn resolve_harness<F>(
    input: &PolicyInput<'_>,
    alias: Option<&ModelAlias>,
    overlay: Option<&AgentOverlay>,
    matched_policy: Option<&MatchedModelPolicy>,
    evidence: HarnessEvidence<'_>,
    probe_resolver: &mut dyn routing::ProbeResolver,
    auth_check: F,
) -> Result<HarnessAttempt, MarsError>
where
    F: Fn(&str) -> crate::harness::host::AuthState,
{
    let mut warnings = Vec::new();
    let profile_harness = input.profile.harness.as_ref().map(harness_kind_to_str);
    let overlay_harness = overlay
        .and_then(|entry| entry.harness.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let policy_harness = matched_policy_string_override(matched_policy, "harness");
    let mut preference = resolve_harness_preference(
        input,
        overlay_harness,
        policy_harness
            .as_ref()
            .filter(|field| field.source == PolicySource::OverlayModelPolicy)
            .cloned(),
        profile_harness,
        policy_harness
            .as_ref()
            .filter(|field| field.source == PolicySource::ProfileModelPolicy)
            .cloned(),
        policy_harness
            .as_ref()
            .filter(|field| field.source == PolicySource::SettingsModelPolicy)
            .cloned(),
        alias.and_then(|entry| entry.harness.as_deref()),
    );
    if let Some(field) = preference.as_mut() {
        field.value = crate::harness::registry::normalize_name(&field.value).ok_or_else(|| {
            MarsError::Config(ConfigError::Invalid {
                message: format!("invalid {} harness `{}`", field.source.label(), field.value),
            })
        })?;
    }
    let normalized_default = routing::normalize_config_default_harness(
        evidence.routing.config_default_harness,
        &mut warnings,
    );
    let mut routing_input = evidence
        .routing
        .routing_input_with_config_default_harness(normalized_default.as_deref());
    let trace = if let Some(pin) = preference
        .as_ref()
        .filter(|field| field.source == PolicySource::Cli)
    {
        routing_input.provider_for_order = routing::provider_for_order_for_fixed_harness(
            evidence.routing.provider_for_order,
            &pin.value,
        );
        let assessment = routing::evaluate_fixed_harness_with_auth_and_probes(
            &routing_input,
            &pin.value,
            probe_resolver,
            auth_check,
        );
        let rejection = routing::acceptance::accept_assessment(&assessment).err();
        let mut trace = routing::trace_for_fixed_harness(
            routing::RouteSource::Cli,
            &pin.value,
            assessment,
            Vec::new(),
        );
        if let Some(rejection) = rejection {
            trace.diagnostics.push(if rejection.is_not_installed() {
                format!("cli harness `{}` is not installed", pin.value)
            } else {
                format!(
                    "cli harness `{}` cannot run the requested model ({})",
                    pin.value,
                    rejection.skip_reason().unwrap_or("unavailable")
                )
            });
            return Ok(HarnessAttempt::Exhausted(trace));
        }
        trace
    } else {
        routing_input.preferred_harness = preference.as_ref().map(|field| {
            (
                field.value.as_str(),
                route_source_for_policy_source(field.source),
            )
        });
        routing::evaluate_candidates(&routing_input, probe_resolver, auth_check)
    };
    if trace.harness.is_empty() {
        return Ok(HarnessAttempt::Exhausted(trace));
    }
    warnings.extend(trace.selected_diagnostics().iter().cloned());
    let harness = preference
        .filter(|field| field.value == trace.harness)
        .unwrap_or_else(|| ResolvedField {
            value: trace.harness.clone(),
            source: trace.source.into(),
            matched_rule: None,
        });
    Ok(HarnessAttempt::Selected(HarnessResolution {
        harness,
        harness_order_position: trace.harness_order_position,
        candidates_tried: trace.candidates_tried.clone(),
        route_trace: trace,
        warnings,
    }))
}

#[allow(clippy::too_many_arguments)]
fn resolve_harness_preference(
    input: &PolicyInput<'_>,
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

fn route_source_for_policy_source(source: PolicySource) -> routing::RouteSource {
    match source {
        PolicySource::Cli => routing::RouteSource::Cli,
        PolicySource::Overlay => routing::RouteSource::Overlay,
        PolicySource::OverlayModelPolicy => routing::RouteSource::OverlayModelPolicy,
        PolicySource::Profile => routing::RouteSource::Profile,
        PolicySource::ProfileModelPolicy => routing::RouteSource::ProfileModelPolicy,
        PolicySource::SettingsModelPolicy => routing::RouteSource::SettingsModelPolicy,
        PolicySource::Alias => routing::RouteSource::Alias,
        PolicySource::ConfigOrder => routing::RouteSource::ConfigOrder,
        PolicySource::Config => routing::RouteSource::ConfigDefault,
        _ => routing::RouteSource::Provider,
    }
}

pub(super) fn harness_kind_to_str(harness: &HarnessKind) -> &'static str {
    crate::compiler::harness_descriptor::descriptor(*harness).canonical_id
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

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
            user_invocable: true,
            had_model_invocable_field: false,
            had_user_invocable_field: false,
            approval: None,
            sandbox: None,
            effort: None,
            autocompact: None,
            autocompact_pct: None,
            skills: crate::frontmatter::SkillsSpec::default(),
            subagents: Vec::new(),
            tools: Vec::new(),
            tools_denied: Vec::new(),
            disallowed_tools: Vec::new(),
            harness_overrides: HarnessOverrides::default(),
            model_policies: Vec::new(),
            fanout: Vec::new(),
        }
    }

    fn model_alias(harness: Option<&str>) -> ModelAlias {
        ModelAlias {
            harness: harness.map(str::to_string),
            description: None,
            prompting: None,
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
            excluded_harnesses: &[],
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
        _model_token: &'a str,
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
                excluded_harnesses: &[],
                harness_scope: crate::config::targets::HarnessScope::Unrestricted,
                opencode_probe_result: None,
                pi_probe_result: None,
                cursor_probe_result: None,
                catalog_model_slugs: None,
            },
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

    /// Test wrapper: calls `resolve_harness` with auth defaulting to all-OK.
    /// Tests that need custom auth behavior should call `resolve_harness` directly.
    fn resolve_harness_test(
        input: &PolicyInput<'_>,
        alias: Option<&ModelAlias>,
        overlay: Option<&AgentOverlay>,
        matched_policy: Option<&MatchedModelPolicy>,
        evidence: HarnessEvidence<'_>,
        probe_resolver: &mut dyn routing::ProbeResolver,
    ) -> Result<HarnessResolution, String> {
        resolve_harness(
            input,
            alias,
            overlay,
            matched_policy,
            evidence,
            probe_resolver,
            |_| crate::harness::host::AuthState::Authenticated,
        )
        .map_err(|error| error.to_string())
        .and_then(|attempt| match attempt {
            HarnessAttempt::Selected(resolution) => Ok(resolution),
            HarnessAttempt::Exhausted(trace) => Err(format!("{trace:?}")),
        })
    }

    #[test]
    fn cli_override_is_explicit_and_skips_candidate_eval() {
        let installed = installed(&["codex", "pi"]);
        let profile = profile(Some(HarnessKind::Claude));
        let input = policy_input(&profile, None, Some("pi"));
        let mut probe_resolver = TestProbeResolver::default();

        let resolution = resolve_harness_test(
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
    fn missing_profile_preference_uses_order_even_with_model_override() {
        let installed = installed(&["opencode"]);
        let profile = profile(Some(HarnessKind::Claude));
        let input = policy_input(&profile, Some("gptmini"), None);
        let mut probe_resolver = TestProbeResolver::default();

        let resolution = resolve_harness_test(
            &input,
            Some(&model_alias(Some("opencode"))),
            None,
            None,
            evidence(None, None, &installed),
            &mut probe_resolver,
        )
        .expect("harness should resolve");

        assert_eq!(resolution.harness.value, "opencode");
        assert_eq!(resolution.harness.source, PolicySource::Provider);
        assert_eq!(
            resolution.route_trace.selected_match_evidence(),
            MatchEvidence::Passthrough
        );
        assert_eq!(
            resolution.route_trace.selection_kind,
            routing::SelectionKind::Auto
        );
        assert_eq!(
            resolution.candidates_tried,
            vec!["claude", "codex", "pi", "cursor", "opencode"]
        );
    }

    #[test]
    fn profile_harness_precedes_alias_when_model_not_overridden() {
        let installed = installed(&["codex", "pi"]);
        let profile = profile(Some(HarnessKind::Codex));
        let input = policy_input(&profile, None, None);
        let mut probe_resolver = TestProbeResolver::default();

        let resolution = resolve_harness_test(
            &input,
            Some(&model_alias(Some("pi"))),
            None,
            None,
            evidence(None, None, &installed),
            &mut probe_resolver,
        )
        .expect("harness should resolve");

        assert_eq!(resolution.harness.value, "codex");
        assert_eq!(resolution.harness.source, PolicySource::Profile);
        assert_eq!(
            resolution.route_trace.selected_match_evidence(),
            MatchEvidence::Confirmed
        );
        assert_eq!(
            resolution.route_trace.selection_kind,
            routing::SelectionKind::Auto
        );
        assert_eq!(resolution.candidates_tried, vec!["codex".to_string()]);
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
        let evidence = evidence_for_model(
            "gpt-5",
            "gpt-5",
            Some("openai"),
            None,
            &installed,
            None,
            None,
        );

        let resolution =
            resolve_harness_test(&input, None, None, None, evidence, &mut probe_resolver)
                .expect("harness should pivot to opencode");

        assert_eq!(resolution.harness.value, "opencode");
        assert_eq!(resolution.harness.source, PolicySource::Provider);
        assert_eq!(
            resolution.route_trace.selected_match_evidence(),
            MatchEvidence::Confirmed
        );
        assert_eq!(
            resolution.candidates_tried,
            vec!["claude", "codex", "pi", "cursor", "opencode"]
        );
        assert_eq!(
            resolution.route_trace.assessments[0].skip_reason,
            Some("not_installed")
        );
    }

    #[test]
    fn unavailable_profile_harness_retains_unverified_candidate() {
        let installed = installed(&["opencode"]);
        let profile = profile(Some(HarnessKind::Claude));
        let input = policy_input(&profile, None, None);
        let mut probe_resolver = TestProbeResolver::default();

        let resolution = resolve_harness_test(
            &input,
            None,
            None,
            None,
            evidence(None, None, &installed),
            &mut probe_resolver,
        )
        .expect("unverified route remains a candidate");

        assert_eq!(resolution.harness.value, "opencode");
        assert_eq!(
            resolution
                .route_trace
                .assessments
                .last()
                .unwrap()
                .eligibility(),
            routing::Eligibility::Unverified
        );
    }

    #[test]
    fn unavailable_cli_harness_errors_without_pivoting() {
        let installed = installed(&["codex", "opencode"]);
        let profile = profile(Some(HarnessKind::Claude));
        let input = policy_input(&profile, None, Some("claude"));
        let mut probe_resolver = TestProbeResolver::default();

        let error = resolve_harness_test(
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
        assert!(message.contains("installed: false"));
    }

    #[test]
    fn fixed_native_harness_rejects_incompatible_provider_constraint() {
        let installed = installed(&["codex"]);
        let profile = profile(None);
        let input = policy_input(&profile, None, Some("codex"));
        let mut probe_resolver = TestProbeResolver::default();
        let evidence = evidence_for_model(
            "gpt-5",
            "gpt-5",
            Some("openai"),
            Some("anthropic"),
            &installed,
            None,
            None,
        );

        let error = resolve_harness_test(&input, None, None, None, evidence, &mut probe_resolver)
            .expect_err("incompatible provider constraint should fail");
        let message = error.to_string();
        assert!(message.contains("cli harness `codex` cannot run the requested model"));
        assert!(message.contains("provider_constraint_unsatisfied"));
    }

    #[test]
    fn auto_selection_maps_routing_trace_fields() {
        let installed = installed(&["pi"]);
        let order = vec!["pi".to_string(), "codex".to_string()];
        let profile = profile(None);
        let input = policy_input(&profile, None, None);
        let mut probe_resolver = TestProbeResolver::default();

        let resolution = resolve_harness_test(
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
        assert_eq!(
            resolution.candidates_tried,
            vec!["pi", "codex", "claude", "opencode", "cursor"]
        );
    }

    #[test]
    fn invalid_config_default_harness_still_warnings_on_fixed_selection() {
        let installed = installed(&["pi"]);
        let profile = profile(Some(HarnessKind::Pi));
        let input = policy_input(&profile, None, None);
        let mut probe_resolver = TestProbeResolver::default();

        let resolution = resolve_harness_test(
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
    fn cli_fixed_harness_rejects_profile_model_on_no_model_match() {
        let installed = installed(&["opencode"]);
        let profile = profile_with_model(Some(HarnessKind::Claude), Some("opus"));
        let input = policy_input(&profile, None, Some("opencode"));
        let mut probe_resolver = TestProbeResolver {
            opencode: Some(positive_opencode_probe()),
            ..Default::default()
        };
        let evidence = evidence_for_model(
            "claude-opus-4-6",
            "opus",
            Some("anthropic"),
            Some("anthropic"),
            &installed,
            None,
            None,
        );

        let error = resolve_harness_test(&input, None, None, None, evidence, &mut probe_resolver)
            .expect_err("a harness pin must not clear the requested model");

        assert!(error.to_string().contains("no_model_match"));
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
            "claude-opus-4-6",
            "opus",
            Some("anthropic"),
            Some("anthropic"),
            &installed,
            None,
            None,
        );

        let err = resolve_harness_test(&input, None, None, None, evidence, &mut probe_resolver)
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
            "gpt-5",
            "gpt-5",
            Some("openai"),
            Some("anthropic"),
            &installed,
            None,
            None,
        );

        let err = resolve_harness_test(&input, None, None, None, evidence, &mut probe_resolver)
            .expect_err("provider constraint failures must remain hard even when probe matches");
        let message = err.to_string();
        assert!(message.contains("provider_constraint_unsatisfied"));
        assert!(!message.contains("no_model_match"));
    }

    #[test]
    fn empty_linked_constraint_route_errors_instead_of_invalid_harness() {
        let installed = installed(&["claude", "cursor", "codex", "pi"]);
        let linked_harnesses = [
            "claude".to_string(),
            "cursor".to_string(),
            "codex".to_string(),
        ];
        let profile = profile(None);
        let input = policy_input(&profile, None, None);
        let cursor_probe = CursorProbeResult {
            model_probe_success: true,
            slugs: vec!["gpt-5.5".to_string()],
            ..CursorProbeResult::default()
        };
        let mut probe_resolver = TestProbeResolver {
            cursor: Some(cursor_probe.clone()),
            ..TestProbeResolver::default()
        };
        let evidence = HarnessEvidence {
            routing: routing::RoutingEvidence {
                model_id: "deepseekflash",
                provider_for_order: Some("deepseek"),
                provider_constraint: None,
                settings_provider_order: None,
                config_default_harness: None,
                settings_harness_order: None,
                installed_harnesses: &installed,
                excluded_harnesses: &[],
                harness_scope: crate::config::targets::HarnessScope::Only(
                    linked_harnesses
                        .iter()
                        .map(|name| crate::harness::registry::parse(name).unwrap())
                        .collect(),
                ),
                opencode_probe_result: None,
                pi_probe_result: None,
                cursor_probe_result: Some(&cursor_probe),
                catalog_model_slugs: None,
            },
        };

        let error = resolve_harness_test(&input, None, None, None, evidence, &mut probe_resolver)
            .expect_err("empty harness route should fail before HarnessKind validation");

        let message = error.to_string();
        assert!(message.contains("LinkedHarnessConstraints"));
        assert!(message.contains("no_model_match"));
    }

    #[test]
    fn overlay_model_pivots_away_from_profile_harness_on_provider_constraint() {
        let installed = installed(&["codex", "claude"]);
        let profile = profile(Some(HarnessKind::Codex));
        let input = policy_input(&profile, None, None);
        let mut probe_resolver = TestProbeResolver::default();

        let overlay = AgentOverlay {
            model: Some("claude-sonnet-4-6".to_string()),
            ..Default::default()
        };
        let evidence = evidence_for_model(
            "claude-sonnet-4-6",
            "sonnet",
            Some("anthropic"),
            Some("anthropic"),
            &installed,
            None,
            None,
        );

        let resolution = resolve_harness_test(
            &input,
            None,
            Some(&overlay),
            None,
            evidence,
            &mut probe_resolver,
        )
        .expect("should pivot to claude instead of hard-failing");

        assert_eq!(resolution.harness.value, "claude");
        assert_eq!(resolution.candidates_tried, vec!["codex", "claude"]);
        assert_eq!(
            resolution.route_trace.assessments[0].skip_reason,
            Some("provider_constraint_unsatisfied")
        );
    }
}
