//! Effort tiers folded into a harness's model slugs.
//!
//! A harness whose registry row carries [`SlugTiers`] lists one slug per effort level of a
//! model, spelled `<model>-<level>` (Cursor's `gpt-5.5-high`; Antigravity's
//! `gemini-3.8-flash-low`). The levels a model lists are that harness's level set for the
//! effort rule, and the slug of the resolved level is what the launch carries.

use std::collections::BTreeMap;

use mars_schema::agent::EffortLevel;
use mars_schema::effort::Levels;

use super::probes::Listing;
use crate::harness::registry::SlugTiers;

/// The tiers a harness lists for one model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelTiers {
    /// The slug for each level the model lists. A level with several slugs keeps the first, or a
    /// Claude `-thinking-` one.
    pub slug_by_level: BTreeMap<EffortLevel, String>,
    /// Every slug of the model, for diagnostics.
    pub candidate_slugs: Vec<String>,
}

impl ModelTiers {
    /// The levels the model lists, as the harness's own set for this model.
    pub fn levels(&self) -> Levels {
        Levels::only(self.slug_by_level.keys().copied())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TiersError {
    NoSlugs,
    NoModelPrefixMatch,
}

/// Why a launch could not learn which slug carries an effort.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TierFailure {
    ListingUnavailable,
    ListingFailed { error: Option<String> },
    ListingEmpty,
    NoModelPrefixMatch,
}

/// The tiers `listing` lists for `model_id`, which is the base model: never a slug that
/// already carries a tier.
pub fn tiers_from_listing(
    spec: SlugTiers,
    model_id: &str,
    listing: Option<&Listing>,
) -> Result<ModelTiers, TierFailure> {
    if model_id.trim().is_empty() {
        return Err(TierFailure::NoModelPrefixMatch);
    }
    let listing = listing.ok_or(TierFailure::ListingUnavailable)?;
    if !listing.succeeded {
        return Err(TierFailure::ListingFailed {
            error: listing.error.clone(),
        });
    }
    tiers(spec, model_id, &listing.slugs).map_err(|error| match error {
        TiersError::NoSlugs => TierFailure::ListingEmpty,
        TiersError::NoModelPrefixMatch => TierFailure::NoModelPrefixMatch,
    })
}

/// The tiers `slugs` list for `model_id`, under the id the harness itself spells it with when
/// that differs from models.dev's.
pub fn tiers(spec: SlugTiers, model_id: &str, slugs: &[String]) -> Result<ModelTiers, TiersError> {
    let tiers = tiers_of_id(spec, model_id, slugs);
    match (spec.respell)(model_id) {
        Some(respelled) if tiers.is_err() => tiers_of_id(spec, &respelled, slugs),
        _ => tiers,
    }
}

/// `slug` without the effort tier folded into it (`gpt-5.5-high` is `gpt-5.5`), or `None` when
/// it names no tier. A model's own name can end in a level (`gpt-5.1-codex-max`): the catalog,
/// not this function, says which reading is right.
pub fn without_tier(slug: &str) -> Option<&str> {
    let slug = slug.strip_suffix("-thinking").unwrap_or(slug);
    EffortLevel::LADDER
        .into_iter()
        .filter(|level| *level != EffortLevel::None)
        .find_map(|level| slug.strip_suffix(level.as_str())?.strip_suffix('-'))
}

pub fn normalize_slug(s: &str) -> String {
    s.to_ascii_lowercase().replace('.', "-")
}

/// The slugs that are `model_id` or its tiers.
pub fn prefix_matches<'a>(model_id: &str, slugs: &'a [String]) -> Vec<&'a str> {
    let normalized_model = normalize_slug(model_id);
    slugs
        .iter()
        .filter(|slug| {
            let normalized_slug = normalize_slug(slug);
            normalized_slug == normalized_model
                || normalized_slug.starts_with(&format!("{normalized_model}-"))
        })
        .map(String::as_str)
        .collect()
}

fn tiers_of_id(
    spec: SlugTiers,
    model_id: &str,
    slugs: &[String],
) -> Result<ModelTiers, TiersError> {
    if slugs.is_empty() {
        return Err(TiersError::NoSlugs);
    }
    let prefix_matches = prefix_matches(model_id, slugs);
    if prefix_matches.is_empty() {
        return Err(TiersError::NoModelPrefixMatch);
    }
    let normalized_model = normalize_slug(model_id);
    let mut slug_by_level = BTreeMap::new();
    for level in EffortLevel::LADDER {
        let matches: Vec<&str> = prefix_matches
            .iter()
            .copied()
            .filter(|slug| {
                slug_matches_level(spec, &normalized_model, &normalize_slug(slug), level)
            })
            .collect();
        if !matches.is_empty() {
            slug_by_level.insert(level, choose_slug(&normalized_model, matches).to_string());
        }
    }
    Ok(ModelTiers {
        slug_by_level,
        candidate_slugs: prefix_matches.iter().map(|slug| slug.to_string()).collect(),
    })
}

fn slug_matches_level(
    spec: SlugTiers,
    normalized_model: &str,
    normalized_slug: &str,
    level: EffortLevel,
) -> bool {
    if normalized_slug == normalized_model {
        return spec.bare_level == Some(level);
    }
    let level = level.as_str();
    let Some(suffix) = normalized_slug
        .strip_prefix(normalized_model)
        .and_then(|rest| rest.strip_prefix('-'))
    else {
        return false;
    };
    suffix == level
        || suffix.ends_with(&format!("-{level}"))
        || suffix.contains(&format!("-{level}-"))
}

fn choose_slug<'a>(normalized_model: &str, matches: Vec<&'a str>) -> &'a str {
    if matches.len() == 1 {
        return matches[0];
    }

    if normalized_model.starts_with("claude")
        && let Some(thinking) = matches
            .iter()
            .copied()
            .find(|slug| normalize_slug(slug).contains("-thinking-"))
    {
        return thinking;
    }

    matches[0]
}
