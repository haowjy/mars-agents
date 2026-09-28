use crate::harness::registry::HarnessId;
use crate::routing::slug;

use super::availability::{ResolvedRunnablePath, RunnableConfidence, RunnablePathSource};

/// Project the launch ID from selected routing evidence. Catalog model names
/// may be normalized; only the requested spelling is safe for native launches.
pub fn resolve_harness_model(
    harness: HarnessId,
    requested: &str,
    chosen_slug: Option<&str>,
    chosen_model: Option<&str>,
    provider_constraint: Option<&str>,
    provider_for_order: Option<&str>,
) -> ResolvedRunnablePath {
    let requested = requested.trim();
    let mars_provider = chosen_slug
        .and_then(slug::parse)
        .map(|parts| parts.provider.to_string())
        .or_else(|| provider_constraint.map(str::to_string))
        .or_else(|| provider_for_order.map(str::to_string));
    if requested.is_empty() {
        return passthrough(requested, mars_provider);
    }

    if harness.native_provider().is_some() {
        let provider_matches = |provider: &str| {
            !provider.trim().is_empty()
                && slug::provider_matches_native_harness(provider, harness.as_str())
        };
        let matched = chosen_model.is_some()
            || provider_constraint.is_some_and(provider_matches)
            || provider_for_order.is_some_and(provider_matches);
        return ResolvedRunnablePath {
            harness_model_id: requested.to_string(),
            mars_provider,
            source: if matched {
                RunnablePathSource::ProviderMatch
            } else {
                RunnablePathSource::Passthrough
            },
            confidence: if matched {
                RunnableConfidence::Likely
            } else {
                RunnableConfidence::Unknown
            },
        };
    }

    if let Some(selected) = chosen_slug.or(chosen_model) {
        return ResolvedRunnablePath {
            harness_model_id: selected.to_string(),
            mars_provider,
            source: RunnablePathSource::CachedProbe,
            confidence: RunnableConfidence::Confirmed,
        };
    }

    // Pi and OpenCode accept provider/model in constrained passthrough. Cursor
    // takes an unqualified model/effort slug even when its provider is known.
    if matches!(harness, HarnessId::Pi | HarnessId::OpenCode)
        && !requested.contains('/')
        && let Some(constraint) = provider_constraint.filter(|value| !value.trim().is_empty())
    {
        return ResolvedRunnablePath {
            harness_model_id: format!("{}/{}", constraint.trim(), requested),
            mars_provider,
            source: RunnablePathSource::Passthrough,
            confidence: RunnableConfidence::Confirmed,
        };
    }
    passthrough(requested, mars_provider)
}

fn passthrough(model_id: &str, mars_provider: Option<String>) -> ResolvedRunnablePath {
    ResolvedRunnablePath {
        harness_model_id: model_id.to_string(),
        mars_provider,
        source: RunnablePathSource::Passthrough,
        confidence: RunnableConfidence::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_keeps_requested_punctuation_and_case_not_catalog_spelling() {
        for requested in ["claude-opus-4.6", "Claude-Opus-4-6"] {
            let resolved = resolve_harness_model(
                HarnessId::Claude,
                requested,
                Some("anthropic/claude-opus-4-6"),
                Some("claude-opus-4-6"),
                None,
                Some("anthropic"),
            );
            assert_eq!(resolved.harness_model_id, requested);
            assert_eq!(resolved.source, RunnablePathSource::ProviderMatch);
        }
    }

    #[test]
    fn probe_backed_prefers_selected_slug_then_model_then_requested() {
        for (slug, model, expected) in [
            (Some("openai/gpt-5"), Some("gpt-5"), "openai/gpt-5"),
            (None, Some("gpt-5"), "gpt-5"),
            (None, None, "GPT-5"),
        ] {
            let resolved = resolve_harness_model(HarnessId::Pi, "GPT-5", slug, model, None, None);
            assert_eq!(resolved.harness_model_id, expected);
        }
    }

    #[test]
    fn selected_slug_owns_provider_and_launch_id_together() {
        let resolved = resolve_harness_model(
            HarnessId::OpenCode,
            "gpt-5",
            Some("openai-codex/gpt-5"),
            Some("gpt-5"),
            None,
            Some("openai"),
        );
        assert_eq!(resolved.harness_model_id, "openai-codex/gpt-5");
        assert_eq!(resolved.mars_provider.as_deref(), Some("openai-codex"));
    }

    #[test]
    fn constrained_passthrough_qualifies_only_pi_and_opencode() {
        for harness in [HarnessId::Pi, HarnessId::OpenCode] {
            let resolved =
                resolve_harness_model(harness, "gpt-5", None, None, Some("openai"), None);
            assert_eq!(resolved.harness_model_id, "openai/gpt-5");
        }
        let cursor = resolve_harness_model(
            HarnessId::Cursor,
            "composer-2.5",
            None,
            None,
            Some("cursor"),
            None,
        );
        assert_eq!(cursor.harness_model_id, "composer-2.5");
    }
}
