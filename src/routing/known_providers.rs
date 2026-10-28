//! Which providers Mars has observed, for judging `settings.provider_order` entries.
//!
//! A provider is known when it appears in the model catalog (the configured catalog providers
//! and the providers of cached entries), in a cached harness probe listing (`opencode-go` from
//! `opencode-go/deepseek-v4.1-flash`), or as a harness's native provider. There is no list to
//! maintain. Variants such as `openai-codex` are known through `slug::normalize_provider`.
//!
//! The catalog comes from the caller; probe listings are read from disk caches; this never probes. A probe-backed harness that is
//! installed but has no usable cached listing (fresh cache, offline, probe failed) may still
//! reveal the provider, so while any such harness is pending no entry is reported unknown.

use std::collections::HashSet;

use crate::harness::host::{ExecutableResolver, ExecutableState, PathExecutableResolver};
use crate::harness::registry::{self, HarnessClass};
use crate::models::probes::{cursor_cache, opencode_cache, pi_cache};
use mars_schema::HarnessId;

use super::slug;

/// Warnings for `provider_order` entries Mars has not observed. Empty while catalog or probe
/// data is pending. `catalog_slugs` are the cached catalog's `provider/model` slugs.
pub fn unknown_provider_order_warnings(order: &[String], catalog_slugs: &[String]) -> Vec<String> {
    match observed_providers(catalog_slugs) {
        Some(observed) => unknown_entries(order, &observed),
        None => Vec::new(),
    }
}

/// `None` while the catalog is empty or an installed probe-backed harness has no usable cached
/// listing.
fn observed_providers(catalog_slugs: &[String]) -> Option<HashSet<String>> {
    if catalog_slugs.is_empty() {
        return None;
    }
    let mut observed: HashSet<String> = crate::models::default_catalog_providers()
        .into_iter()
        .collect();
    observed.extend(slug_providers(catalog_slugs));
    observed.extend(
        registry::descriptors()
            .iter()
            .filter_map(|d| registry::native_provider(d.id).map(str::to_string)),
    );

    let mut probes_pending = false;
    for descriptor in registry::descriptors() {
        if !matches!(descriptor.class, HarnessClass::ProbeBacked { .. })
            || !matches!(
                PathExecutableResolver.resolve(descriptor.binary),
                ExecutableState::Found { .. }
            )
        {
            continue;
        }
        let slugs: Option<Vec<String>> = match descriptor.id {
            HarnessId::OpenCode => {
                opencode_cache::read_cached_probe_result_usable().map(|result| result.model_slugs)
            }
            HarnessId::Pi => pi_cache::read_cached_probe_result_usable()
                .map(|result| result.model_slugs.into_iter().collect()),
            HarnessId::Cursor => {
                cursor_cache::read_cached_probe_result_usable().map(|result| result.slugs)
            }
            _ => None,
        };
        match slugs {
            Some(slugs) => observed.extend(slug_providers(&slugs)),
            None => probes_pending = true,
        }
    }
    (!probes_pending).then_some(observed)
}

fn slug_providers(slugs: &[String]) -> impl Iterator<Item = String> + '_ {
    slugs
        .iter()
        .filter_map(|s| s.split_once('/').map(|(provider, _)| provider.to_string()))
}

fn unknown_entries(order: &[String], observed: &HashSet<String>) -> Vec<String> {
    let observed: HashSet<String> = observed
        .iter()
        .map(|provider| slug::normalize_provider(provider))
        .collect();
    order
        .iter()
        .filter(|provider| !provider.trim().is_empty())
        .filter(|provider| !observed.contains(&slug::normalize_provider(provider)))
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

    #[test]
    fn observed_provider_is_known_and_typo_is_not() {
        let observed = HashSet::from(["openai".to_string(), "opencode-go".to_string()]);
        let order = ["opencode-go", "openai-codex", "anthropc"].map(String::from);
        let warnings = unknown_entries(&order, &observed);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("`anthropc`"));
    }
}
