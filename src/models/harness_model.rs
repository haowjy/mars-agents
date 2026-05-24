use crate::routing::probe_match::select_probe_slug;
use crate::routing::slug;

use super::availability::{ResolvedRunnablePath, RunnableConfidence, RunnablePathSource};
use super::probes::{OpenCodeProbeResult, PiProbeResult};

pub struct HarnessModelInput<'a> {
    pub harness: &'a str,
    pub model_id: &'a str,
    pub provider_constraint: Option<&'a str>,
    pub provider_for_order: Option<&'a str>,
    pub settings_provider_order: Option<&'a [String]>,
    pub opencode_probe: Option<&'a OpenCodeProbeResult>,
    pub pi_probe: Option<&'a PiProbeResult>,
}

pub fn resolve_harness_model(input: HarnessModelInput<'_>) -> ResolvedRunnablePath {
    let model_id = input.model_id.trim();
    if model_id.is_empty() {
        return ResolvedRunnablePath {
            harness_model_id: String::new(),
            source: RunnablePathSource::Passthrough,
            confidence: RunnableConfidence::Unknown,
        };
    }

    if let Some(constraint) = input
        .provider_constraint
        .filter(|provider| !provider.trim().is_empty())
    {
        return ResolvedRunnablePath {
            harness_model_id: format!("{}/{}", constraint.trim(), model_id),
            source: RunnablePathSource::Passthrough,
            confidence: RunnableConfidence::Confirmed,
        };
    }

    let harness = input.harness;
    if harness.eq_ignore_ascii_case("pi") {
        return resolve_pi_harness_model(input);
    }
    if harness.eq_ignore_ascii_case("opencode") {
        return resolve_opencode_harness_model(input);
    }

    let provider = input.provider_for_order.unwrap_or("").trim();
    if !provider.is_empty() && slug::provider_matches_native_harness(provider, harness) {
        return ResolvedRunnablePath {
            harness_model_id: model_id.to_string(),
            source: RunnablePathSource::ProviderMatch,
            confidence: RunnableConfidence::Likely,
        };
    }

    ResolvedRunnablePath {
        harness_model_id: model_id.to_string(),
        source: RunnablePathSource::Passthrough,
        confidence: RunnableConfidence::Unknown,
    }
}

fn resolve_pi_harness_model(input: HarnessModelInput<'_>) -> ResolvedRunnablePath {
    let model_id = input.model_id.trim();
    let Some(pi_probe) = input.pi_probe else {
        return passthrough_bare(model_id);
    };
    if !pi_probe.compatible {
        return passthrough_bare(model_id);
    }

    probe_slug_or_passthrough(
        model_id,
        input.provider_constraint,
        input.provider_for_order,
        input.settings_provider_order,
        pi_probe.model_slugs.iter().map(String::as_str),
    )
}

fn resolve_opencode_harness_model(input: HarnessModelInput<'_>) -> ResolvedRunnablePath {
    let model_id = input.model_id.trim();
    let Some(opencode_probe) = input.opencode_probe else {
        return passthrough_bare(model_id);
    };
    if !opencode_probe.model_probe_success {
        return passthrough_bare(model_id);
    }

    probe_slug_or_passthrough(
        model_id,
        input.provider_constraint,
        input.provider_for_order,
        input.settings_provider_order,
        opencode_probe.model_slugs.iter().map(String::as_str),
    )
}

fn probe_slug_or_passthrough<'a>(
    model_id: &str,
    provider_constraint: Option<&str>,
    provider_for_order: Option<&str>,
    settings_provider_order: Option<&[String]>,
    slugs: impl IntoIterator<Item = &'a str>,
) -> ResolvedRunnablePath {
    let selection = select_probe_slug(
        model_id,
        provider_constraint,
        provider_for_order,
        settings_provider_order,
        slugs,
    );
    if let Some(slug) = selection.chosen_slug {
        return ResolvedRunnablePath {
            harness_model_id: slug,
            source: RunnablePathSource::CachedProbe,
            confidence: RunnableConfidence::Confirmed,
        };
    }

    passthrough_bare(model_id)
}

fn passthrough_bare(model_id: &str) -> ResolvedRunnablePath {
    ResolvedRunnablePath {
        harness_model_id: model_id.to_string(),
        source: RunnablePathSource::Passthrough,
        confidence: RunnableConfidence::Unknown,
    }
}

pub fn pi_harness_model_requires_probe_slug(
    harness: &str,
    selection_kind: &str,
    model_id: &str,
    resolved: &ResolvedRunnablePath,
) -> bool {
    harness.eq_ignore_ascii_case("pi")
        && selection_kind.eq_ignore_ascii_case("fixed")
        && !model_id.trim().is_empty()
        && resolved.source == RunnablePathSource::Passthrough
        && !resolved.harness_model_id.contains('/')
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::models::probes::PiProbeResult;

    #[test]
    fn qualified_provider_constraint_passthrough_without_probe() {
        let resolved = resolve_harness_model(HarnessModelInput {
            harness: "pi",
            model_id: "gpt-5.4-mini",
            provider_constraint: Some("openai-codex"),
            provider_for_order: Some("openai-codex"),
            settings_provider_order: None,
            opencode_probe: None,
            pi_probe: None,
        });

        assert_eq!(resolved.harness_model_id, "openai-codex/gpt-5.4-mini");
        assert_eq!(resolved.source, RunnablePathSource::Passthrough);
        assert_eq!(resolved.confidence, RunnableConfidence::Confirmed);
    }

    #[test]
    fn pi_bare_model_uses_probe_slug() {
        let mut model_slugs = HashSet::new();
        model_slugs.insert("openai-codex/gpt-5.4-mini".to_string());
        model_slugs.insert("openai/gpt-5.4-mini".to_string());
        let pi_probe = PiProbeResult {
            compatible: true,
            model_slugs,
            ..PiProbeResult::default()
        };

        let resolved = resolve_harness_model(HarnessModelInput {
            harness: "pi",
            model_id: "gpt-5.4-mini",
            provider_constraint: None,
            provider_for_order: Some("openai"),
            settings_provider_order: None,
            opencode_probe: None,
            pi_probe: Some(&pi_probe),
        });

        assert_eq!(resolved.harness_model_id, "openai-codex/gpt-5.4-mini");
        assert_eq!(resolved.source, RunnablePathSource::CachedProbe);
        assert_eq!(resolved.confidence, RunnableConfidence::Confirmed);
    }

    #[test]
    fn pi_constraint_prefers_matching_provider_slug() {
        let mut model_slugs = HashSet::new();
        model_slugs.insert("openai-codex/gpt-5.4-mini".to_string());
        model_slugs.insert("openai/gpt-5.4-mini".to_string());
        let pi_probe = PiProbeResult {
            compatible: true,
            model_slugs,
            ..PiProbeResult::default()
        };

        let resolved = resolve_harness_model(HarnessModelInput {
            harness: "pi",
            model_id: "gpt-5.4-mini",
            provider_constraint: Some("openai-codex"),
            provider_for_order: Some("openai-codex"),
            settings_provider_order: None,
            opencode_probe: None,
            pi_probe: Some(&pi_probe),
        });

        assert_eq!(resolved.harness_model_id, "openai-codex/gpt-5.4-mini");
    }

    #[test]
    fn opencode_uses_probe_slug_without_provider_constraint() {
        let opencode_probe = OpenCodeProbeResult {
            model_slugs: vec![
                "openai/gpt-5.4-mini".to_string(),
                "openai/gpt-5.5".to_string(),
            ],
            model_probe_success: true,
            error: None,
        };

        let resolved = resolve_harness_model(HarnessModelInput {
            harness: "opencode",
            model_id: "gpt-5.4-mini",
            provider_constraint: None,
            provider_for_order: Some("openai"),
            settings_provider_order: None,
            opencode_probe: Some(&opencode_probe),
            pi_probe: None,
        });

        assert_eq!(resolved.harness_model_id, "openai/gpt-5.4-mini");
        assert_eq!(resolved.source, RunnablePathSource::CachedProbe);
    }

    #[test]
    fn codex_native_provider_match_returns_bare_model() {
        let resolved = resolve_harness_model(HarnessModelInput {
            harness: "codex",
            model_id: "gpt-5.4-mini",
            provider_constraint: None,
            provider_for_order: Some("openai"),
            settings_provider_order: None,
            opencode_probe: None,
            pi_probe: None,
        });

        assert_eq!(resolved.harness_model_id, "gpt-5.4-mini");
        assert_eq!(resolved.source, RunnablePathSource::ProviderMatch);
        assert_eq!(resolved.confidence, RunnableConfidence::Likely);
    }

    #[test]
    fn pi_fixed_without_probe_slug_is_detected() {
        let resolved = passthrough_bare("gpt-5.4-mini");
        assert!(pi_harness_model_requires_probe_slug(
            "pi",
            "fixed",
            "gpt-5.4-mini",
            &resolved
        ));
    }
}
