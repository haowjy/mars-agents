use crate::routing::slug;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlugSelection {
    pub candidate_slugs: Vec<String>,
    pub filtered_slugs: Vec<String>,
    pub chosen_slug: Option<String>,
}

pub fn select_probe_slug<'a>(
    model_id: &str,
    provider_constraint: Option<&str>,
    provider_for_order: Option<&str>,
    provider_order: Option<&[String]>,
    slugs: impl IntoIterator<Item = &'a str>,
) -> SlugSelection {
    let known_provider_for_order = provider_for_order.and_then(|provider| {
        let normalized = provider.trim();
        (!normalized.is_empty() && !normalized.eq_ignore_ascii_case("unknown"))
            .then_some(normalized)
    });
    let model_matches = slug::find_model_matches(model_id, slugs)
        .into_iter()
        .map(|matched| (matched.provider, matched.slug))
        .collect::<Vec<_>>();
    let mut candidate_slugs = model_matches
        .iter()
        .map(|(_, slug)| slug.clone())
        .collect::<Vec<_>>();
    candidate_slugs.sort();

    let mut constrained_matches = model_matches;
    if let Some(constraint) = provider_constraint {
        let normalized_constraint = constraint.trim();
        constrained_matches.retain(|(provider, _)| {
            slug::provider_match_tier(normalized_constraint, provider).is_some()
        });
    }
    let mut filtered_slugs = constrained_matches
        .iter()
        .map(|(_, slug)| slug.clone())
        .collect::<Vec<_>>();
    filtered_slugs.sort();

    let chosen_slug = if constrained_matches.is_empty() {
        None
    } else {
        sort_probe_matches(
            &mut constrained_matches,
            provider_constraint,
            known_provider_for_order,
            provider_order,
        );
        constrained_matches.first().map(|(_, slug)| slug.clone())
    };

    SlugSelection {
        candidate_slugs,
        filtered_slugs,
        chosen_slug,
    }
}

pub(crate) fn provider_exact_match_rank(
    known_provider_for_order: Option<&str>,
    candidate_provider: &str,
) -> u8 {
    if known_provider_for_order
        .is_some_and(|provider| slug::providers_exact_match(provider, candidate_provider))
    {
        0
    } else {
        1
    }
}

pub(crate) fn provider_order_rank(provider: &str, provider_order: &[String]) -> usize {
    let key = slug::normalize_provider(provider);
    provider_order
        .iter()
        .position(|configured| slug::normalize_provider(configured) == key)
        .unwrap_or(usize::MAX)
}

/// Prefer more specific provider variants (e.g. `openai-codex`) over collapsed keys (`openai`).
fn provider_variant_preference(provider: &str, known_provider: Option<&str>) -> u8 {
    let Some(known) = known_provider.filter(|value| !value.trim().is_empty()) else {
        return 1;
    };
    if slug::providers_exact_match(provider, known) {
        return 1;
    }
    if slug::providers_match(provider, known) && provider.trim().len() > known.trim().len() {
        return 0;
    }
    1
}

fn sort_probe_matches(
    matches: &mut [(String, String)],
    provider_constraint: Option<&str>,
    known_provider_for_order: Option<&str>,
    provider_order: Option<&[String]>,
) {
    if let Some(constraint) = provider_constraint {
        matches.sort_by(|(left_provider, left_slug), (right_provider, right_slug)| {
            slug::provider_match_tier(constraint, left_provider)
                .cmp(&slug::provider_match_tier(constraint, right_provider))
                .then_with(|| left_slug.cmp(right_slug))
        });
        return;
    }

    if let Some(provider_order) = provider_order {
        if provider_order.is_empty() {
            matches.sort_by(|(left_provider, left_slug), (right_provider, right_slug)| {
                slug::normalize_provider(left_provider)
                    .cmp(&slug::normalize_provider(right_provider))
                    .then_with(|| {
                        provider_variant_preference(left_provider, known_provider_for_order).cmp(
                            &provider_variant_preference(right_provider, known_provider_for_order),
                        )
                    })
                    .then_with(|| {
                        provider_exact_match_rank(known_provider_for_order, left_provider).cmp(
                            &provider_exact_match_rank(known_provider_for_order, right_provider),
                        )
                    })
                    .then_with(|| left_slug.cmp(right_slug))
            });
        } else {
            matches.sort_by(|(left_provider, left_slug), (right_provider, right_slug)| {
                provider_order_rank(left_provider, provider_order)
                    .cmp(&provider_order_rank(right_provider, provider_order))
                    .then_with(|| {
                        provider_variant_preference(left_provider, known_provider_for_order).cmp(
                            &provider_variant_preference(right_provider, known_provider_for_order),
                        )
                    })
                    .then_with(|| {
                        provider_exact_match_rank(known_provider_for_order, left_provider).cmp(
                            &provider_exact_match_rank(known_provider_for_order, right_provider),
                        )
                    })
                    .then_with(|| left_slug.cmp(right_slug))
            });
        }
        return;
    }

    matches.sort_by(|(left_provider, left_slug), (right_provider, right_slug)| {
        slug::normalize_provider(left_provider)
            .cmp(&slug::normalize_provider(right_provider))
            .then_with(|| {
                provider_variant_preference(left_provider, known_provider_for_order).cmp(
                    &provider_variant_preference(right_provider, known_provider_for_order),
                )
            })
            .then_with(|| {
                provider_exact_match_rank(known_provider_for_order, left_provider).cmp(
                    &provider_exact_match_rank(known_provider_for_order, right_provider),
                )
            })
            .then_with(|| left_slug.cmp(right_slug))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_constraint_prefers_exact_provider_tier() {
        let selection = select_probe_slug(
            "gpt-5.4-mini",
            Some("openai-codex"),
            None,
            None,
            ["openai/gpt-5.4-mini", "openai-codex/gpt-5.4-mini"],
        );
        assert_eq!(
            selection.chosen_slug.as_deref(),
            Some("openai-codex/gpt-5.4-mini")
        );
    }

    #[test]
    fn stable_sort_picks_first_slug_when_no_constraint() {
        let selection = select_probe_slug(
            "gpt-5.4-mini",
            None,
            None,
            None,
            ["openai-codex/gpt-5.4-mini", "openai/gpt-5.4-mini"],
        );
        assert_eq!(
            selection.chosen_slug.as_deref(),
            Some("openai-codex/gpt-5.4-mini")
        );
    }
}
