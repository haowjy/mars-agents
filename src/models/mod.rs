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
use serde::{Deserialize, Serialize};

use crate::diagnostic::DiagnosticCollector;

pub mod availability;
mod catalog_api;
mod catalog_cache;
mod dependencies;
pub mod harness;
pub mod harness_model;
pub mod possible;
pub mod probes;

pub use availability::ModelAvailability;
pub use catalog_api::{
    CachedModel, default_catalog_providers, fetch_models, fetch_models_with_providers,
};
pub use catalog_cache::{
    BackgroundRefresh, ModelsCache, RefreshMode, RefreshOutcome, ensure_fresh,
    ensure_fresh_with_catalog_providers, is_mars_offline, now_unix_secs, now_unix_secs_value,
    read_cache, refresh_warning, run_background_refresh,
};
pub(crate) use dependencies::{declaration_ordered_dep_models, merged_model_aliases};

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// A model alias — either pinned to a specific model ID or auto-resolved
/// against the models cache at resolution time.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModelAlias {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompting: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub autocompact: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub autocompact_pct: Option<u8>,
    #[serde(flatten)]
    pub spec: ModelSpec,
}

impl ModelAlias {
    pub fn pinned_model_id(&self) -> Option<&str> {
        match &self.spec {
            ModelSpec::Pinned { model, .. } | ModelSpec::PinnedWithMatch { model, .. } => {
                Some(model.as_str())
            }
            ModelSpec::AutoResolve { .. } => None,
        }
    }
}

/// How a model alias resolves to a concrete model ID.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelSpec {
    /// Explicit model ID — no resolution needed.
    Pinned {
        model: String,
        provider: Option<String>,
    },
    /// Explicit model ID for resolution, plus discovery filters for list/all views.
    PinnedWithMatch {
        model: String,
        provider: Option<String>,
        match_patterns: Vec<String>,
        exclude_patterns: Vec<String>,
    },
    /// Pattern-based resolution against models cache.
    AutoResolve {
        provider: Option<String>,
        match_patterns: Vec<String>,
        exclude_patterns: Vec<String>,
    },
}

/// How the harness was determined.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessSource {
    Explicit,
    AutoDetected,
    Unavailable,
}

/// Fully resolved model alias — everything a consumer needs to launch.
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
    pub default_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub autocompact: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub autocompact_pct: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub availability: Option<ModelAvailability>,
}

// Custom Serialize for ModelSpec to flatten into parent
impl Serialize for ModelSpec {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        match self {
            ModelSpec::Pinned { model, provider } => {
                let mut count = 1;
                if provider.is_some() {
                    count += 1;
                }
                let mut map = serializer.serialize_map(Some(count))?;
                map.serialize_entry("model", model)?;
                if let Some(provider) = provider {
                    map.serialize_entry("provider", provider)?;
                }
                map.end()
            }
            ModelSpec::PinnedWithMatch {
                model,
                provider,
                match_patterns,
                exclude_patterns,
            } => {
                let mut count = 2; // model + match
                if provider.is_some() {
                    count += 1;
                }
                if !exclude_patterns.is_empty() {
                    count += 1;
                }
                let mut map = serializer.serialize_map(Some(count))?;
                map.serialize_entry("model", model)?;
                map.serialize_entry("match", match_patterns)?;
                if let Some(provider) = provider {
                    map.serialize_entry("provider", provider)?;
                }
                if !exclude_patterns.is_empty() {
                    map.serialize_entry("exclude", exclude_patterns)?;
                }
                map.end()
            }
            ModelSpec::AutoResolve {
                provider,
                match_patterns,
                exclude_patterns,
            } => {
                let mut count = 1; // match
                if provider.is_some() {
                    count += 1;
                }
                if !exclude_patterns.is_empty() {
                    count += 1;
                }
                let mut map = serializer.serialize_map(Some(count))?;
                if let Some(provider) = provider {
                    map.serialize_entry("provider", provider)?;
                }
                map.serialize_entry("match", match_patterns)?;
                if !exclude_patterns.is_empty() {
                    map.serialize_entry("exclude", exclude_patterns)?;
                }
                map.end()
            }
        }
    }
}

/// Raw deserialization helper — distinguished by field presence.
#[derive(Debug, Deserialize)]
struct RawModelAlias {
    harness: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    prompting: Option<String>,
    #[serde(default)]
    native: Option<toml::Value>,
    #[serde(default)]
    default_effort: Option<String>,
    #[serde(default)]
    autocompact: Option<toml::Value>,
    #[serde(default)]
    autocompact_pct: Option<toml::Value>,
    // Pinned mode
    #[serde(default)]
    model: Option<String>,
    // AutoResolve mode
    #[serde(default)]
    provider: Option<String>,
    #[serde(default, rename = "match")]
    match_patterns: Option<Vec<String>>,
    #[serde(default)]
    exclude: Option<Vec<String>>,
}

impl<'de> Deserialize<'de> for ModelAlias {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = RawModelAlias::deserialize(deserializer)?;
        let normalized_harness = if let Some(ref harness_name) = raw.harness {
            Some(
                harness::normalize_harness_name(harness_name).ok_or_else(|| {
                    serde::de::Error::custom(format!(
                        "invalid harness '{harness_name}'; valid harnesses: {}",
                        mars_schema::harness::names()
                    ))
                })?,
            )
        } else {
            None
        };
        if raw.native.is_some() {
            return Err(serde::de::Error::custom(
                "[models.<alias>.native] is no longer supported; Cursor model adaptation is internal",
            ));
        }
        let default_effort = raw.default_effort.filter(|value| !value.trim().is_empty());
        if let Some(ref effort) = default_effort {
            const VALID_EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max", "auto"];
            if !VALID_EFFORTS.contains(&effort.as_str()) {
                return Err(serde::de::Error::custom(format!(
                    "invalid default_effort '{effort}'; accepted values: {}",
                    VALID_EFFORTS.join(", ")
                )));
            }
        }
        let autocompact: Option<u32> = match raw.autocompact {
            Some(toml::Value::Integer(value)) => match u32::try_from(value) {
                Ok(v) => Some(v),
                Err(_) => {
                    return Err(serde::de::Error::custom(format!(
                        "autocompact {value} is out of u32 range (0–4294967295)"
                    )));
                }
            },
            Some(other) => {
                return Err(serde::de::Error::custom(format!(
                    "autocompact must be an integer (token count), got {other:?}"
                )));
            }
            None => None,
        };
        let autocompact_pct: Option<u8> = match raw.autocompact_pct {
            Some(toml::Value::Integer(value)) if (1..=100).contains(&value) => Some(value as u8),
            Some(toml::Value::Integer(value)) => {
                return Err(serde::de::Error::custom(format!(
                    "autocompact_pct {value} is out of range 1-100"
                )));
            }
            Some(other) => {
                return Err(serde::de::Error::custom(format!(
                    "autocompact_pct must be an integer 1-100, got {other:?}"
                )));
            }
            None => None,
        };

        let has_match = raw.match_patterns.is_some();

        let spec = if let Some(model) = raw.model {
            if !has_match && raw.exclude.is_some() {
                return Err(serde::de::Error::custom(
                    "model alias with 'exclude' must also include 'match'",
                ));
            }
            if let Some(match_patterns) = raw.match_patterns {
                ModelSpec::PinnedWithMatch {
                    model,
                    provider: raw.provider,
                    match_patterns,
                    exclude_patterns: raw.exclude.unwrap_or_default(),
                }
            } else {
                ModelSpec::Pinned {
                    model,
                    provider: raw.provider,
                }
            }
        } else if let Some(match_patterns) = raw.match_patterns {
            ModelSpec::AutoResolve {
                provider: raw.provider,
                match_patterns,
                exclude_patterns: raw.exclude.unwrap_or_default(),
            }
        } else {
            return Err(serde::de::Error::custom(
                "model alias must have either 'model' (pinned) or 'match' (auto-resolve)",
            ));
        };

        Ok(ModelAlias {
            harness: normalized_harness,
            description: raw.description,
            prompting: raw.prompting,
            default_effort,
            autocompact,
            autocompact_pct,
            spec,
        })
    }
}

// ---------------------------------------------------------------------------
// Models cache
// ---------------------------------------------------------------------------

/// Provider/model slugs from the models.dev catalog for harness routing comparisons.
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

/// Catalog + harness probe refresh intent from CLI flags.
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

// ---------------------------------------------------------------------------
// Auto-resolve algorithm
// ---------------------------------------------------------------------------

/// Resolve an auto-resolve spec against the models cache.
///
/// Algorithm:
/// 1. Filter by provider (case-insensitive) when specified
/// 2. All match patterns must hit (AND)
/// 3. No exclude patterns may hit (OR)
/// 4. Skip entries ending with `-latest` (synthetic aliases)
/// 5. Sort by newest release_date, then shortest ID, then lexical ID
/// 6. Return all candidates
pub fn auto_resolve_all<'a>(
    provider: Option<&str>,
    match_patterns: &[String],
    exclude_patterns: &[String],
    cache: &'a ModelsCache,
) -> Vec<&'a CachedModel> {
    let mut candidates: Vec<&CachedModel> = cache
        .models
        .iter()
        .filter(|m| {
            // Provider match (case-insensitive) — skip filter when provider is None
            provider.is_none_or(|p| m.provider.eq_ignore_ascii_case(p))
        })
        .filter(|m| {
            // Skip -latest suffix (synthetic aliases)
            !m.id.ends_with("-latest")
        })
        .filter(|m| {
            // All match patterns must hit (AND)
            match_patterns.iter().all(|p| glob_match(p, &m.id))
        })
        .filter(|m| {
            // No exclude patterns may hit (OR)
            !exclude_patterns.iter().any(|p| glob_match(p, &m.id))
        })
        .collect();

    // Sort: newest release_date first, then shortest ID, then lexical ID.
    candidates.sort_by(|a, b| {
        let date_cmp = b
            .release_date
            .as_deref()
            .unwrap_or("")
            .cmp(a.release_date.as_deref().unwrap_or(""));
        date_cmp
            .then_with(|| a.id.len().cmp(&b.id.len()))
            .then_with(|| a.id.cmp(&b.id))
    });

    candidates
}

/// Resolve an auto-resolve spec against the models cache.
///
/// Algorithm:
/// 1. Filter by provider (case-insensitive) when specified
/// 2. All match patterns must hit (AND)
/// 3. No exclude patterns may hit (OR)
/// 4. Skip entries ending with `-latest` (synthetic aliases)
/// 5. Sort by newest release_date, then shortest ID, then lexical ID
/// 6. Pick first
pub fn auto_resolve(
    provider: Option<&str>,
    match_patterns: &[String],
    exclude_patterns: &[String],
    cache: &ModelsCache,
) -> Option<String> {
    auto_resolve_all(provider, match_patterns, exclude_patterns, cache)
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
                && !candidate.provider.eq_ignore_ascii_case(provider_filter)
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
                for candidate in
                    auto_resolve_all(provider.as_deref(), match_patterns, exclude_patterns, cache)
                {
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
                for candidate in auto_resolve_all(provider, match_patterns, exclude_patterns, cache)
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

    let mut candidates: Vec<CachedModel> = deduped.into_values().collect();
    candidates.sort_by(|a, b| {
        let date_cmp = b
            .release_date
            .as_deref()
            .unwrap_or("")
            .cmp(a.release_date.as_deref().unwrap_or(""));
        date_cmp
            .then_with(|| a.id.len().cmp(&b.id.len()))
            .then_with(|| a.id.cmp(&b.id))
    });

    let winner = candidates.into_iter().next()?;
    let provider = winner.provider.to_ascii_lowercase();
    let (default_effort, autocompact, autocompact_pct) = match base_alias {
        Some(ModelAlias {
            default_effort,
            autocompact,
            autocompact_pct,
            spec: ModelSpec::Pinned { .. } | ModelSpec::PinnedWithMatch { .. },
            ..
        }) => (default_effort.clone(), *autocompact, *autocompact_pct),
        _ => (None, None, None),
    };
    Some(ResolvedAlias {
        name: input.to_string(),
        model_id: winner.id,
        provider: provider.clone(),
        harness: None,
        harness_source: HarnessSource::Unavailable,
        harness_candidates: harness::harness_candidates_for_provider(&provider),
        description: winner.description,
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

/// Simple glob matching: `*` matches any sequence of characters.
/// Everything else is literal. Case-sensitive.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    // Split pattern on '*' and match segments in order
    let segments: Vec<&str> = pattern.split('*').collect();

    if segments.len() == 1 {
        // No wildcards — exact match
        return pattern == text;
    }

    let mut pos = 0;

    // First segment must be a prefix
    if let Some(first) = segments.first()
        && !first.is_empty()
    {
        if !text.starts_with(first) {
            return false;
        }
        pos = first.len();
    }

    // Last segment must be a suffix
    if let Some(last) = segments.last()
        && !last.is_empty()
        && !text[pos..].ends_with(last)
    {
        return false;
    }

    // Middle segments must appear in order
    let end = if let Some(last) = segments.last() {
        if !last.is_empty() {
            text.len() - last.len()
        } else {
            text.len()
        }
    } else {
        text.len()
    };

    for segment in &segments[1..segments.len().saturating_sub(1)] {
        if segment.is_empty() {
            continue;
        }
        if let Some(idx) = text[pos..end].find(segment) {
            pos += idx + segment.len();
        } else {
            return false;
        }
    }

    pos <= end
}

// ---------------------------------------------------------------------------
// Builtin aliases — bare convenience mappings, no descriptions
// ---------------------------------------------------------------------------

/// Minimal builtin aliases so common model names work out of the box.
/// Suppressed as soon as consumer or dependency aliases exist.
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

// ---------------------------------------------------------------------------
// Dependency-tree merge
// ---------------------------------------------------------------------------

/// Info about a resolved dependency's model config.
pub struct ResolvedDepModels {
    pub source_name: String,
    pub models: IndexMap<String, ModelAlias>,
}

/// Merge model aliases from dependency tree.
///
/// Precedence: consumer > deps (declaration order).
/// Builtins appear only when consumer and dependency aliases are both empty.
/// When two deps define the same alias, first in declaration order wins
/// with a diagnostic warning.
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

    // Track which dep won each alias
    let mut dep_provided: std::collections::HashMap<String, DepWinner> =
        std::collections::HashMap::new();

    // Dependencies: first dep wins on conflicts
    for dep in deps {
        for (name, alias) in &dep.models {
            if consumer.contains_key(name) {
                // Consumer will override — skip dep's version silently
                continue;
            }
            if let Some(winner) = dep_provided.get(name) {
                // Two deps define same alias — first dep wins, warn
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

    // Consumer config overrides dependency aliases.
    for (name, alias) in consumer {
        merged.insert(name.clone(), alias.clone());
    }

    merged
}
/// Resolve aliases without any harness detection or probe/auth checks.
///
/// This is intended for static list views where only alias -> model/provider
/// resolution is needed.
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

/// Resolve model identity only; callers supply scoped routing evidence separately.
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
        default_effort: alias.default_effort.clone(),
        autocompact: alias.autocompact,
        autocompact_pct: alias.autocompact_pct,
        availability: None,
    })
}

/// Resolve a concrete model id for one alias.
///
/// Used by build-time launch routing so model resolution logic stays shared
/// with `mars models resolve`.
pub fn resolve_model_id_for_alias(alias: &ModelAlias, cache: &ModelsCache) -> Option<String> {
    resolve_model_and_provider(alias, cache).map(|(model_id, _provider)| model_id)
}

/// Resolve provider identity for one alias.
///
/// Returns `None` when provider cannot be inferred.
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

/// Best-effort provider inference from model ID prefixes.
/// Returns None for unrecognized patterns.
pub fn infer_provider_from_model_id(model_id: &str) -> Option<&'static str> {
    let id = model_id.to_lowercase();
    if id.starts_with("claude-") {
        return Some("anthropic");
    }
    if id.starts_with("gpt-")
        || id.starts_with("o1")
        || id.starts_with("o3")
        || id.starts_with("o4")
        || id.starts_with("codex-")
    {
        return Some("openai");
    }
    if id.starts_with("gemini") {
        return Some("google");
    }
    if id.starts_with("llama") {
        return Some("meta");
    }
    if id.starts_with("mistral") || id.starts_with("codestral") {
        return Some("mistral");
    }
    if id.starts_with("deepseek") {
        return Some("deepseek");
    }
    if id.starts_with("command") {
        return Some("cohere");
    }
    None
}

/// Split a token shaped like `provider/model` into `(model, provider_constraint)`.
///
/// Returns `(trimmed_token, None)` when the token is not a valid constrained slug.
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
