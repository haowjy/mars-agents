//! Model catalog — aliases with direct model pinning and optional discovery filters,
//! dependency-tree config merge, and models.dev ingestion. Persisted catalog
//! refresh lifecycle lives in `catalog_cache`.
//!
//! Model aliases map short names (opus, sonnet, codex) to concrete model IDs.
//! Two modes:
//! - **Pinned**: explicit model ID, with optional `match`/`exclude` discovery filters.
//! - **AutoResolve**: pattern-based resolution against a cached model catalog.
//!
//! Merge precedence: consumer > deps (declaration order).

use indexmap::IndexMap;
use serde::Serialize;

use crate::diagnostic::DiagnosticCollector;
use mars_schema::agent::EffortRequest;

pub mod availability;
pub(crate) mod catalog;
mod catalog_api;
mod catalog_cache;
mod dependencies;
pub mod harness;
pub mod harness_model;
pub mod possible;
pub mod probes;
pub mod reasoning;

pub use availability::ModelAvailability;
pub use catalog_api::default_catalog_providers;
pub use catalog_cache::{
    BackgroundRefresh, ModelsCache, RefreshMode, RefreshOutcome,
    ensure_fresh_with_catalog_providers, is_mars_offline, now_unix_secs, now_unix_secs_value,
    read_cache, refresh_warning, run_background_refresh,
};
pub(crate) use dependencies::{declaration_ordered_dep_models, merged_model_aliases};
pub use mars_schema::alias::{ModelAlias, ModelSpec};
pub use mars_schema::models::{
    CachedModel, auto_resolve_all, catalog_provider_canonical, glob_match,
    infer_provider_from_model_id, sort_best_first,
};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessSource {
    Explicit,
    AutoDetected,
    Unavailable,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResolvedAlias {
    pub name: String,
    pub model_id: String,
    pub provider: String,
    pub harness: Option<String>,
    pub harness_source: HarnessSource,
    pub harness_candidates: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompting: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<EffortRequest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub autocompact: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub autocompact_pct: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub availability: Option<ModelAvailability>,
}

pub fn catalog_model_slugs(cache: &ModelsCache) -> Vec<String> {
    cache
        .models
        .iter()
        .map(|model| {
            format!(
                "{}/{}",
                crate::routing::slug::normalize_provider(&model.provider),
                model.id
            )
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelsRefreshControl {
    pub catalog_mode: RefreshMode,
    pub probe_refresh: crate::models::probes::ProbeRefreshMode,
}

impl ModelsRefreshControl {
    pub fn background() -> Self {
        Self {
            catalog_mode: RefreshMode::Background,
            probe_refresh: crate::models::probes::ProbeRefreshMode::Background,
        }
    }
}

pub fn resolve_models_refresh_control(
    refresh_models: bool,
    no_refresh_models: bool,
) -> Result<ModelsRefreshControl, crate::error::MarsError> {
    use crate::error::ConfigError;
    use crate::models::probes::ProbeRefreshMode;

    if refresh_models && no_refresh_models {
        return Err(crate::error::MarsError::Config(ConfigError::Invalid {
            message: "--refresh-models and --no-refresh-models cannot be used together".to_string(),
        }));
    }

    Ok(if no_refresh_models {
        ModelsRefreshControl {
            catalog_mode: RefreshMode::Offline,
            probe_refresh: ProbeRefreshMode::Skip,
        }
    } else if refresh_models {
        ModelsRefreshControl {
            catalog_mode: RefreshMode::Force,
            probe_refresh: ProbeRefreshMode::Synchronous,
        }
    } else {
        ModelsRefreshControl::background()
    })
}

pub fn dependency_alias_snapshot(deps: &[ResolvedDepModels]) -> IndexMap<String, ModelAlias> {
    let mut merged = IndexMap::new();
    for dep in deps {
        for (name, alias) in &dep.models {
            if !merged.contains_key(name) {
                merged.insert(name.clone(), alias.clone());
            }
        }
    }
    merged
}

pub fn merged_runtime_aliases(
    dependency_aliases: &IndexMap<String, ModelAlias>,
    project_aliases: Option<&IndexMap<String, ModelAlias>>,
) -> IndexMap<String, ModelAlias> {
    let has_project_aliases = project_aliases.is_some_and(|aliases| !aliases.is_empty());
    let mut merged = if dependency_aliases.is_empty() && !has_project_aliases {
        builtin_aliases()
    } else {
        IndexMap::new()
    };
    for (name, alias) in dependency_aliases {
        merged.insert(name.clone(), alias.clone());
    }
    if let Some(project_aliases) = project_aliases {
        for (name, alias) in project_aliases {
            merged.insert(name.clone(), alias.clone());
        }
    }
    merged
}

pub fn auto_resolve(
    provider: Option<&str>,
    match_patterns: &[String],
    exclude_patterns: &[String],
    cache: &ModelsCache,
) -> Option<String> {
    auto_resolve_all(provider, match_patterns, exclude_patterns, &cache.models)
        .first()
        .map(|model| model.id.clone())
}
pub fn resolve_with_alias_prefix_static(
    input: &str,
    aliases: &IndexMap<String, ModelAlias>,
    cache: &ModelsCache,
) -> Option<ResolvedAlias> {
    let pattern = if input.contains('*') {
        input.to_string()
    } else {
        format!("*{}*", input)
    };
    let base_alias = alias_prefix_base(input, aliases);
    let mut deduped: IndexMap<String, CachedModel> = IndexMap::new();

    if let Some(alias) = base_alias
        && let Some((model, provider)) = match &alias.spec {
            ModelSpec::Pinned { model, provider } => Some((model, provider)),
            ModelSpec::PinnedWithMatch {
                model, provider, ..
            } => Some((model, provider)),
            ModelSpec::AutoResolve { .. } => None,
        }
    {
        let provider_filter = provider
            .as_deref()
            .or_else(|| infer_provider_from_model_id(model));
        for candidate in &cache.models {
            if !glob_match(&pattern, &candidate.id) {
                continue;
            }
            if let Some(provider_filter) = provider_filter
                && catalog_provider_canonical(&candidate.provider_key)
                    != catalog_provider_canonical(provider_filter)
            {
                continue;
            }
            deduped
                .entry(candidate.id.clone())
                .or_insert_with(|| candidate.clone());
        }
    }

    for (_alias_name, alias) in aliases {
        match &alias.spec {
            ModelSpec::AutoResolve {
                provider,
                match_patterns,
                exclude_patterns,
            } => {
                for candidate in auto_resolve_all(
                    provider.as_deref(),
                    match_patterns,
                    exclude_patterns,
                    &cache.models,
                ) {
                    if glob_match(&pattern, &candidate.id) {
                        deduped
                            .entry(candidate.id.clone())
                            .or_insert_with(|| candidate.clone());
                    }
                }
            }
            ModelSpec::PinnedWithMatch {
                model,
                provider,
                match_patterns,
                exclude_patterns,
            } => {
                let provider = provider
                    .as_deref()
                    .or_else(|| infer_provider_from_model_id(model));
                for candidate in
                    auto_resolve_all(provider, match_patterns, exclude_patterns, &cache.models)
                {
                    if glob_match(&pattern, &candidate.id) {
                        deduped
                            .entry(candidate.id.clone())
                            .or_insert_with(|| candidate.clone());
                    }
                }
            }
            ModelSpec::Pinned { .. } => {}
        }
    }

    let mut candidates: Vec<&CachedModel> = deduped.values().collect();
    sort_best_first(&mut candidates);

    let winner = candidates.into_iter().next()?;
    let provider = winner.provider.to_ascii_lowercase();
    let (default_effort, autocompact, autocompact_pct) = match base_alias {
        Some(ModelAlias {
            default_effort,
            autocompact,
            autocompact_pct,
            spec: ModelSpec::Pinned { .. } | ModelSpec::PinnedWithMatch { .. },
            ..
        }) => (*default_effort, *autocompact, *autocompact_pct),
        _ => (None, None, None),
    };
    Some(ResolvedAlias {
        name: input.to_string(),
        model_id: winner.id.clone(),
        provider: provider.clone(),
        harness: None,
        harness_source: HarnessSource::Unavailable,
        harness_candidates: harness::harness_candidates_for_provider(&provider),
        description: winner.description.clone(),
        prompting: base_alias.and_then(|a| a.prompting.clone()),
        default_effort,
        autocompact,
        autocompact_pct,
        availability: None,
    })
}

pub(crate) fn alias_prefix_base<'a>(
    input: &str,
    aliases: &'a IndexMap<String, ModelAlias>,
) -> Option<&'a ModelAlias> {
    aliases
        .iter()
        .filter(|(name, _)| {
            !name.is_empty()
                && input.len() > name.len()
                && input.starts_with(name.as_str())
                && input.as_bytes().get(name.len()) == Some(&b'-')
        })
        .max_by_key(|(name, _)| name.len())
        .map(|(_, alias)| alias)
}

pub fn builtin_aliases() -> IndexMap<String, ModelAlias> {
    let mut m = IndexMap::new();
    let add = |m: &mut IndexMap<String, ModelAlias>,
               name: &str,
               provider: &str,
               match_patterns: &[&str],
               exclude: &[&str]| {
        m.insert(
            name.to_string(),
            ModelAlias {
                harness: None,
                description: None,
                prompting: None,
                default_effort: None,
                autocompact: None,
                autocompact_pct: None,
                spec: ModelSpec::AutoResolve {
                    provider: Some(provider.to_string()),
                    match_patterns: match_patterns.iter().map(|s| s.to_string()).collect(),
                    exclude_patterns: exclude.iter().map(|s| s.to_string()).collect(),
                },
            },
        );
    };
    add(&mut m, "opus", "anthropic", &["*opus*"], &[]);
    add(&mut m, "sonnet", "anthropic", &["*sonnet*"], &[]);
    add(&mut m, "haiku", "anthropic", &["*haiku*"], &[]);
    add(
        &mut m,
        "codex",
        "openai",
        &["*codex*"],
        &["*-mini", "*-spark", "*-max"],
    );
    add(
        &mut m,
        "gpt",
        "openai",
        &["gpt-5*"],
        &["*codex*", "*-mini", "*-nano", "*-chat", "*-turbo"],
    );
    add(
        &mut m,
        "gemini",
        "google",
        &["gemini*", "*pro*"],
        &["*-customtools"],
    );
    m
}

pub struct ResolvedDepModels {
    pub source_name: String,
    pub models: IndexMap<String, ModelAlias>,
}

pub fn merge_model_config(
    consumer: &IndexMap<String, ModelAlias>,
    deps: &[ResolvedDepModels],
    diag: &mut DiagnosticCollector,
    cache: Option<&ModelsCache>,
) -> IndexMap<String, ModelAlias> {
    #[derive(Clone)]
    struct DepWinner {
        source_name: String,
        alias: ModelAlias,
    }

    let has_dep_aliases = deps.iter().any(|dep| !dep.models.is_empty());
    let mut merged = if consumer.is_empty() && !has_dep_aliases {
        builtin_aliases()
    } else {
        IndexMap::new()
    };

    let mut dep_provided: std::collections::HashMap<String, DepWinner> =
        std::collections::HashMap::new();

    for dep in deps {
        for (name, alias) in &dep.models {
            if consumer.contains_key(name) {
                continue;
            }
            if let Some(winner) = dep_provided.get(name) {
                let message = if let Some(cache) = cache {
                    let (winner_formatted, winner_model_id) =
                        format_alias_resolution_for_diag(&winner.alias, &winner.source_name, cache);
                    let (loser_formatted, loser_model_id) =
                        format_alias_resolution_for_diag(alias, &dep.source_name, cache);
                    if winner_model_id.is_some() && winner_model_id == loser_model_id {
                        format!(
                            "model alias `{name}` defined by both `{}` and `{}` — using {} (declared first)\n  both resolve to {}\n  → add [models.{name}] to your mars.toml to resolve explicitly",
                            winner.source_name,
                            dep.source_name,
                            winner.source_name,
                            winner_model_id.unwrap_or_default(),
                        )
                    } else {
                        format!(
                            "model alias `{name}` defined by both `{}` and `{}` — using {} (declared first)\n  {winner_formatted}, {loser_formatted}\n  → add [models.{name}] to your mars.toml to resolve explicitly",
                            winner.source_name, dep.source_name, winner.source_name,
                        )
                    }
                } else {
                    format!(
                        "model alias `{name}` defined by both `{}` and `{}` — using {} (declared first)\n  → add [models.{name}] to your mars.toml to resolve explicitly",
                        winner.source_name, dep.source_name, winner.source_name,
                    )
                };
                diag.warn_with_context("model-alias-conflict", message, dep.source_name.clone());
            } else {
                merged.insert(name.clone(), alias.clone());
                dep_provided.insert(
                    name.clone(),
                    DepWinner {
                        source_name: dep.source_name.clone(),
                        alias: alias.clone(),
                    },
                );
            }
        }
    }

    for (name, alias) in consumer {
        merged.insert(name.clone(), alias.clone());
    }

    merged
}
pub fn resolve_all_static(
    aliases: &IndexMap<String, ModelAlias>,
    cache: &ModelsCache,
) -> IndexMap<String, ResolvedAlias> {
    aliases
        .keys()
        .filter_map(|name| {
            resolve_one_static(name, aliases, cache).map(|resolved| (name.clone(), resolved))
        })
        .collect()
}

pub fn resolve_one_static(
    name: &str,
    aliases: &IndexMap<String, ModelAlias>,
    cache: &ModelsCache,
) -> Option<ResolvedAlias> {
    let alias = aliases.get(name)?;
    let (model_id, provider) = resolve_model_and_provider(alias, cache)?;
    Some(ResolvedAlias {
        name: name.to_string(),
        model_id,
        provider,
        harness: None,
        harness_source: HarnessSource::Unavailable,
        harness_candidates: Vec::new(),
        description: alias.description.clone(),
        prompting: alias.prompting.clone(),
        default_effort: alias.default_effort,
        autocompact: alias.autocompact,
        autocompact_pct: alias.autocompact_pct,
        availability: None,
    })
}

pub fn resolve_model_id_for_alias(alias: &ModelAlias, cache: &ModelsCache) -> Option<String> {
    resolve_model_and_provider(alias, cache).map(|(model_id, _provider)| model_id)
}

pub fn resolve_provider_for_alias(alias: &ModelAlias, cache: &ModelsCache) -> Option<String> {
    let provider = resolve_model_and_provider(alias, cache)
        .map(|(_model_id, provider)| provider)
        .or_else(|| provider_from_alias_spec(alias));

    provider.filter(|value| crate::routing::slug::provider_is_resolved(value))
}

fn resolve_model_and_provider(alias: &ModelAlias, cache: &ModelsCache) -> Option<(String, String)> {
    match &alias.spec {
        ModelSpec::Pinned { model, .. } | ModelSpec::PinnedWithMatch { model, .. } => {
            let provider = provider_from_alias_spec(alias).unwrap_or_else(|| "unknown".to_string());
            Some((model.clone(), provider))
        }
        ModelSpec::AutoResolve {
            provider,
            match_patterns,
            exclude_patterns,
        } => {
            let model_id =
                auto_resolve(provider.as_deref(), match_patterns, exclude_patterns, cache)?;
            // When provider is known from the alias, use it; otherwise look up
            // the resolved model's provider in the cache.
            let resolved_provider = provider
                .clone()
                .or_else(|| {
                    cache
                        .models
                        .iter()
                        .find(|m| m.id == model_id)
                        .map(|m| m.provider.clone())
                })
                .unwrap_or_else(|| "unknown".to_string());
            Some((model_id, resolved_provider))
        }
    }
}

/// Authored provider restriction, never inferred from a preferred harness.
/// A provider-qualified pinned model supplies a restriction when the field is absent.
pub(crate) fn provider_constraint_for_alias(alias: &ModelAlias) -> Option<String> {
    let (provider, model) = match &alias.spec {
        ModelSpec::Pinned { model, provider }
        | ModelSpec::PinnedWithMatch {
            model, provider, ..
        } => (provider.as_deref(), Some(model.as_str())),
        ModelSpec::AutoResolve { provider, .. } => (provider.as_deref(), None),
    };
    provider
        .map(|provider| provider.trim().to_ascii_lowercase())
        .or_else(|| model.and_then(|model| split_provider_constrained_model_token(model).1))
}

fn provider_from_alias_spec(alias: &ModelAlias) -> Option<String> {
    match &alias.spec {
        ModelSpec::Pinned { model, provider }
        | ModelSpec::PinnedWithMatch {
            model, provider, ..
        } => provider
            .clone()
            .or_else(|| provider_constraint_for_alias(alias))
            .or_else(|| infer_provider_from_model_id(model).map(str::to_string)),
        ModelSpec::AutoResolve { provider, .. } => provider.clone(),
    }
}

fn format_alias_resolution_for_diag(
    alias: &ModelAlias,
    source_name: &str,
    cache: &ModelsCache,
) -> (String, Option<String>) {
    match &alias.spec {
        ModelSpec::Pinned { model, .. } => (
            format!("{source_name} → {model} (pinned)"),
            Some(model.clone()),
        ),
        ModelSpec::PinnedWithMatch { model, .. } => (
            format!("{source_name} → {model} (pinned+match)"),
            Some(model.clone()),
        ),
        ModelSpec::AutoResolve {
            provider,
            match_patterns,
            exclude_patterns,
        } => {
            let resolved =
                auto_resolve(provider.as_deref(), match_patterns, exclude_patterns, cache);
            match resolved {
                Some(model_id) => (format!("{source_name} → {model_id}"), Some(model_id)),
                None => (format!("{source_name} → <unresolvable>"), None),
            }
        }
    }
}

pub fn split_provider_constrained_model_token(token: &str) -> (String, Option<String>) {
    let trimmed = token.trim();
    let Some((provider, model_name)) = trimmed.split_once('/') else {
        return (trimmed.to_string(), None);
    };
    let provider = provider.trim();
    let model_name = model_name.trim();
    if provider.is_empty() || model_name.is_empty() {
        return (trimmed.to_string(), None);
    }
    (model_name.to_string(), Some(provider.to_ascii_lowercase()))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
