/// Decomposed slug — the shared representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlugParts<'a> {
    pub provider: &'a str,
    pub model_id: &'a str,
    pub full: &'a str,
}

/// Parse a slug string into provider/model_id parts.
/// Uses split_once('/') — provider is everything before first '/',
/// model_id is everything after (may contain more '/' for nested slugs).
/// Returns None for empty provider or empty model_id.
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

/// Normalize a model ID for comparison: lowercase + replace '.' with '-'.
pub fn normalize_model_id(id: &str) -> String {
    id.to_lowercase().replace('.', "-")
}

/// Case-insensitive model ID comparison with dot-dash normalization.
pub fn model_ids_match(a: &str, b: &str) -> bool {
    normalize_model_id(a) == normalize_model_id(b)
}

/// Normalize a provider key for comparisons and ranking.
/// Trims whitespace, lowercases, and collapses known provider variants.
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

/// Provider comparison through normalized provider keys.
pub fn providers_match(a: &str, b: &str) -> bool {
    normalize_provider(a) == normalize_provider(b)
}

/// Exact provider-name match (case-insensitive, no variant collapsing).
pub fn providers_exact_match(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// Whether a provider string maps to a native harness.
/// Native mappings:
/// - `claude` ↔ `anthropic` (including variants like `anthropic-claude`)
/// - `codex` ↔ `openai` (including variants like `openai-codex`)
pub fn provider_matches_native_harness(provider: &str, harness: &str) -> bool {
    let harness = harness.trim().to_ascii_lowercase();
    match harness.as_str() {
        "claude" => providers_match(provider, "anthropic"),
        "codex" => providers_match(provider, "openai"),
        _ => false,
    }
}

/// Match tier for provider matching.
/// - 0: exact provider match
/// - 1: normalized-provider variant match
/// - None: no match
pub fn provider_match_tier(target_provider: &str, candidate_provider: &str) -> Option<u8> {
    if providers_exact_match(target_provider, candidate_provider) {
        Some(0)
    } else if providers_match(target_provider, candidate_provider) {
        Some(1)
    } else {
        None
    }
}

/// Result of matching a model against a set of slugs.
#[derive(Debug, Clone)]
pub struct SlugMatch {
    pub slug: String,
    pub provider: String,
    pub model_id: String,
}

/// Find all slugs whose model_id matches `target_model_id`.
/// Works with any iterator of slug strings (Vec<String>, HashSet<String>, etc.).
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
            model_id: parts.model_id.to_string(),
        })
        .collect()
}

/// Find first slug matching both provider and model_id.
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

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn parse_two_segments() {
        let slug = parse("openai/gpt-5.4").unwrap();
        assert_eq!(slug.provider, "openai");
        assert_eq!(slug.model_id, "gpt-5.4");
        assert_eq!(slug.full, "openai/gpt-5.4");
    }

    #[test]
    fn parse_nested_slug_keeps_tail() {
        let slug = parse("openrouter/anthropic/claude-opus-4.7").unwrap();
        assert_eq!(slug.provider, "openrouter");
        assert_eq!(slug.model_id, "anthropic/claude-opus-4.7");
        assert_eq!(slug.full, "openrouter/anthropic/claude-opus-4.7");
    }

    #[test]
    fn parse_invalid_returns_none() {
        assert!(parse("gpt-5").is_none());
        assert!(parse("openai/").is_none());
        assert!(parse("/gpt-5").is_none());
    }

    #[test]
    fn normalize_and_match_model_ids() {
        assert_eq!(normalize_model_id("Claude-Opus-4.7"), "claude-opus-4-7");
        assert!(model_ids_match("claude-opus-4-7", "Claude-Opus-4.7"));
        assert!(!model_ids_match("claude-opus-4-7", "claude-sonnet-4-7"));
    }

    #[test]
    fn provider_matching_is_case_insensitive() {
        assert!(providers_match("Anthropic", "anthropic"));
        assert!(!providers_match("Anthropic", "openai"));
    }

    #[test]
    fn normalize_provider_collapses_known_variants() {
        assert_eq!(normalize_provider(" openai-codex "), "openai");
        assert_eq!(normalize_provider("ANTHROPIC-CLAUDE"), "anthropic");
        assert_eq!(normalize_provider(" OpenRouter "), "openrouter");
    }

    #[test]
    fn provider_matching_uses_normalized_provider_keys() {
        assert!(providers_match("openai-codex", "openai"));
        assert!(providers_match("anthropic", "anthropic-claude"));
    }

    #[test]
    fn provider_matches_native_harness_accepts_provider_variants() {
        assert!(provider_matches_native_harness("openai-codex", "codex"));
        assert!(provider_matches_native_harness(
            "anthropic-claude",
            "claude"
        ));
        assert!(!provider_matches_native_harness("openai-codex", "claude"));
    }

    #[test]
    fn find_model_matches_filters_invalid_and_non_matching() {
        let slugs = vec![
            "openai/gpt-5.4-mini",
            "bad-slug",
            "openrouter/anthropic/claude-opus-4.7",
            "google/gemini-2.5-pro",
        ];

        let matches = find_model_matches("gpt-5-4-mini", slugs);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].slug, "openai/gpt-5.4-mini");
        assert_eq!(matches[0].provider, "openai");
        assert_eq!(matches[0].model_id, "gpt-5.4-mini");
    }

    #[test]
    fn find_model_matches_accepts_exact_full_slug_token() {
        let matches = find_model_matches("openai/gpt-5", vec!["openai/gpt-5", "openai/gpt-5.4"]);

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].slug, "openai/gpt-5");
        assert_eq!(matches[0].provider, "openai");
        assert_eq!(matches[0].model_id, "gpt-5");
    }

    #[test]
    fn find_exact_match_works_with_hashset_iterators() {
        let slugs = HashSet::from([
            "openai-codex/gpt-5.4-mini".to_string(),
            "openai/gpt-5.4-mini".to_string(),
        ]);

        let matched =
            find_exact_match("gpt-5-4-mini", "openai", slugs.iter().map(String::as_str)).unwrap();
        assert_eq!(matched.provider, "openai");
    }

    #[test]
    fn find_exact_match_prefers_exact_provider_before_variant() {
        let slugs = vec!["openai-codex/gpt-5.4-mini", "openai/gpt-5.4-mini"];
        let matched = find_exact_match("gpt-5-4-mini", "openai", slugs).unwrap();
        assert_eq!(matched.slug, "openai/gpt-5.4-mini");
    }

    #[test]
    fn find_exact_match_requires_provider_match() {
        let slugs = vec!["openai-codex/gpt-5.4-mini"];
        assert!(find_exact_match("gpt-5-4-mini", "anthropic", slugs).is_none());
    }
}
