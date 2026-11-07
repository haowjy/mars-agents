use crate::harness::registry;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlugParts<'a> {
    pub provider: &'a str,
    pub model_id: &'a str,
    pub full: &'a str,
}

pub fn parse(slug: &str) -> Option<SlugParts<'_>> {
    let (provider, model_id) = slug.split_once('/')?;
    if provider.is_empty() || model_id.is_empty() {
        return None;
    }
    Some(SlugParts {
        provider,
        model_id,
        full: slug,
    })
}

pub fn normalize_model_id(id: &str) -> String {
    id.to_lowercase().replace('.', "-")
}

pub fn model_ids_match(a: &str, b: &str) -> bool {
    normalize_model_id(a) == normalize_model_id(b)
}

pub fn normalize_provider(provider: &str) -> String {
    let normalized = provider.trim().to_ascii_lowercase();
    if let Some(base) = normalized.strip_suffix("-codex")
        && base == "openai"
    {
        return base.to_string();
    }
    if let Some(base) = normalized.strip_suffix("-claude")
        && base == "anthropic"
    {
        return base.to_string();
    }
    normalized
}

pub fn providers_match(a: &str, b: &str) -> bool {
    normalize_provider(a) == normalize_provider(b)
}

/// `unknown` (any case) and blank are resolution sentinels, not providers.
pub fn provider_is_resolved(provider: &str) -> bool {
    let trimmed = provider.trim();
    !trimmed.is_empty() && !trimmed.eq_ignore_ascii_case("unknown")
}

pub fn providers_exact_match(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

pub fn provider_matches_native_harness(provider: &str, harness: &str) -> bool {
    mars_schema::harness::parse(harness)
        .and_then(registry::native_provider)
        .is_some_and(|native| providers_match(provider, native))
}

pub fn provider_match_tier(target_provider: &str, candidate_provider: &str) -> Option<u8> {
    if providers_exact_match(target_provider, candidate_provider) {
        Some(0)
    } else if providers_match(target_provider, candidate_provider) {
        Some(1)
    } else {
        None
    }
}

#[derive(Debug, Clone)]
pub struct SlugMatch {
    pub slug: String,
    pub provider: String,
}

pub fn find_model_matches<'a>(
    target_model_id: &str,
    slugs: impl IntoIterator<Item = &'a str>,
) -> Vec<SlugMatch> {
    slugs
        .into_iter()
        .filter_map(parse)
        .filter(|parts| {
            model_ids_match(target_model_id, parts.model_id)
                || model_ids_match(target_model_id, parts.full)
        })
        .map(|parts| SlugMatch {
            slug: parts.full.to_string(),
            provider: parts.provider.to_string(),
        })
        .collect()
}

pub fn find_exact_match<'a>(
    target_model_id: &str,
    target_provider: &str,
    slugs: impl IntoIterator<Item = &'a str>,
) -> Option<SlugMatch> {
    let mut matches = find_model_matches(target_model_id, slugs)
        .into_iter()
        .filter(|entry| provider_match_tier(target_provider, &entry.provider).is_some())
        .collect::<Vec<_>>();

    matches.sort_by(|left, right| {
        provider_match_tier(target_provider, &left.provider)
            .cmp(&provider_match_tier(target_provider, &right.provider))
            .then_with(|| left.slug.cmp(&right.slug))
    });
    matches.into_iter().next()
}
