use crate::harness::registry;
use crate::routing::slug;
use mars_schema::HarnessId;

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

    if registry::native_provider(harness).is_some() {
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
