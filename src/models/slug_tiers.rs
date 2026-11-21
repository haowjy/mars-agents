//! Effort tiers folded into a harness's model slugs.
//!
//! A harness whose slugs are `SlugShape::Bare` (Antigravity) lists a model's effort levels as
//! separate slugs, spelled `<family>-<level>` (`gemini-3.8-flash-low`). The harness takes model
//! and effort as one value, so a launch resolves both to one listed slug. Only a trailing
//! effort-level name is a tier: `gemini-pro-orchestrator-neon` is a slug with no tiers.

use std::collections::BTreeMap;

use mars_schema::agent::EffortLevel;

use crate::routing::slug::normalize_slug as normalize;

/// What the listing says a requested model is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Listed {
    /// A family and the slug it lists for each level, spelled as listed.
    Family {
        family: String,
        tiers: BTreeMap<EffortLevel, String>,
    },
    /// A slug with no tier, which takes no effort.
    Untiered { slug: String },
}

/// A requested model found in a listing, and the tier its own slug carried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub listed: Listed,
    /// The level the requested slug named (`gemini-3.8-flash-low` carries `low`).
    pub carried: Option<EffortLevel>,
}

impl Found {
    /// The listed slugs the requested model stands for: each tier of its family, or the one slug.
    pub fn slugs(&self) -> Vec<String> {
        match &self.listed {
            Listed::Family { tiers, .. } => tiers.values().cloned().collect(),
            Listed::Untiered { slug } => vec![slug.clone()],
        }
    }
}

/// `slug` as a family and the effort level it ends in.
fn split_tier(slug: &str) -> Option<(&str, EffortLevel)> {
    let (family, level) = slug.rsplit_once('-')?;
    Some((family, EffortLevel::parse(level)?)).filter(|(family, _)| !family.is_empty())
}

fn family_tiers(
    normalized: &str,
    slugs: &[String],
) -> Option<(String, BTreeMap<EffortLevel, String>)> {
    let mut family = None;
    let mut tiers = BTreeMap::new();
    for slug in slugs {
        if let Some((name, level)) = split_tier(slug)
            && normalize(name) == normalized
        {
            family.get_or_insert_with(|| name.to_string());
            tiers.entry(level).or_insert_with(|| slug.clone());
        }
    }
    Some((family?, tiers))
}

/// `model` in `slugs`, by normalized name: a listed slug, or a family whose tiers are listed.
/// This is the one rule for whether such a harness lists a model, for routing and for the slug
/// a launch sends.
pub fn find(model: &str, slugs: &[String]) -> Option<Found> {
    let normalized = normalize(model);
    let exact = slugs.iter().find(|slug| normalize(slug) == normalized);
    let carried = exact
        .and_then(|slug| split_tier(slug))
        .map(|(_, level)| level);
    let family_name = match exact.and_then(|slug| split_tier(slug)) {
        Some((family, _)) => normalize(family),
        None => normalized,
    };
    if let Some((family, tiers)) = family_tiers(&family_name, slugs) {
        return Some(Found {
            listed: Listed::Family { family, tiers },
            carried,
        });
    }
    exact.map(|slug| Found {
        listed: Listed::Untiered { slug: slug.clone() },
        carried: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use EffortLevel::{High, Low, Medium};

    fn slugs() -> Vec<String> {
        [
            "gemini-3.8-flash-high",
            "gemini-3.8-flash-low",
            "gpt-oss-120b-medium",
            "gemini-pro-orchestrator-neon",
        ]
        .map(String::from)
        .to_vec()
    }

    #[test]
    fn a_family_lists_its_tiers_and_a_tiered_slug_carries_one() {
        let found = find("Gemini-3.8-Flash", &slugs()).unwrap();
        assert_eq!(found.carried, None);
        let Listed::Family { family, tiers } = &found.listed else {
            panic!("{found:?}")
        };
        assert_eq!(family, "gemini-3.8-flash");
        assert_eq!(tiers.keys().copied().collect::<Vec<_>>(), [Low, High]);

        let carried = find("gemini-3.8-flash-low", &slugs()).unwrap();
        assert_eq!(carried.carried, Some(Low));
        assert_eq!(carried.listed, found.listed);
    }

    #[test]
    fn a_slug_without_an_effort_suffix_is_untiered() {
        let found = find("gemini-pro-orchestrator-neon", &slugs()).unwrap();
        assert_eq!(found.carried, None);
        assert!(matches!(found.listed, Listed::Untiered { .. }));
        assert_eq!(find("unlisted", &slugs()), None);
    }

    #[test]
    fn a_one_tier_family_has_one_level() {
        let Listed::Family { tiers, .. } = find("gpt-oss-120b", &slugs()).unwrap().listed else {
            panic!()
        };
        assert_eq!(tiers.keys().copied().collect::<Vec<_>>(), [Medium]);
    }
}
