//! Which providers Mars has observed, for judging `settings.provider_order` entries.
//!
//! A provider is known when it appears in the model catalog (the default catalog providers and
//! the providers of cached entries), in an OpenCode or Pi probe listing (`opencode-go` from
//! `opencode-go/deepseek-v4.1-flash`), or as a harness's native provider. There is no list to
//! maintain. Variants such as `openai-codex` are known through `slug::normalize_provider`.
//!
//! Pure: the caller (the capability session) supplies the catalog slugs and probe outcomes and
//! passes the result in as `RoutingInput::known_providers`. Cursor is not a source: its slugs
//! carry no provider prefix.

use std::collections::HashSet;

use crate::harness::registry;

use super::slug;

/// What an installed probe-backed harness (OpenCode, Pi) has told us.
pub enum ProbeListing {
    /// Never probed: a listing may still reveal the provider.
    NeverProbed,
    /// The latest attempt failed and left no listing. Adds nothing, blocks nothing.
    Failed,
    Listed(Vec<String>),
}

/// The known providers, or `None` (pending) while the check could still be wrong: the catalog is
/// empty or an installed probe-backed harness has never been probed. Only installed harnesses
/// appear in `probes`.
pub fn known_providers(
    catalog_slugs: &[String],
    probes: &[ProbeListing],
) -> Option<HashSet<String>> {
    if catalog_slugs.is_empty()
        || probes
            .iter()
            .any(|p| matches!(p, ProbeListing::NeverProbed))
    {
        return None;
    }
    let mut known: HashSet<String> = crate::models::default_catalog_providers()
        .into_iter()
        .collect();
    known.extend(slug_providers(catalog_slugs));
    known.extend(
        registry::descriptors()
            .iter()
            .filter_map(|d| registry::native_provider(d.id).map(str::to_string)),
    );
    for probe in probes {
        if let ProbeListing::Listed(slugs) = probe {
            known.extend(slug_providers(slugs));
        }
    }
    Some(known)
}

fn slug_providers(slugs: &[String]) -> impl Iterator<Item = String> + '_ {
    slugs
        .iter()
        .filter_map(|s| s.split_once('/').map(|(provider, _)| provider.to_string()))
}

/// Warnings for `provider_order` entries absent from `known`.
pub fn unknown_provider_order_warnings(order: &[String], known: &HashSet<String>) -> Vec<String> {
    let known: HashSet<String> = known
        .iter()
        .map(|provider| slug::normalize_provider(provider))
        .collect();
    order
        .iter()
        .filter(|provider| !provider.trim().is_empty())
        .filter(|provider| !known.contains(&slug::normalize_provider(provider)))
        .map(|provider| {
            format!(
                "settings.provider_order contains unknown provider `{provider}`; keeping it for forward-compat routing preferences"
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typo_warnings(probes: &[ProbeListing]) -> Option<usize> {
        let catalog = ["openai/gpt-5".to_string()];
        let order = ["opencode-go", "anthropc"].map(String::from);
        known_providers(&catalog, probes).map(|k| unknown_provider_order_warnings(&order, &k).len())
    }

    #[test]
    fn suppression_rule() {
        // Never probed: pending, so silent.
        assert_eq!(typo_warnings(&[ProbeListing::NeverProbed]), None);
        // Failed probe adds nothing; both entries are unknown.
        assert_eq!(typo_warnings(&[ProbeListing::Failed]), Some(2));
        // Observed in a listing: known, only the typo warns.
        let listed = ProbeListing::Listed(vec!["opencode-go/deepseek".to_string()]);
        assert_eq!(typo_warnings(&[listed]), Some(1));
    }
}
