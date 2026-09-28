//! Human model inventory. Curation is confined to this display command.

use crate::config::routing_settings::ResolvedRoutingSettings;
use crate::curation::{CuratedRow, CuratedRules, Decision};
use crate::error::{ConfigError, MarsError};
use crate::harness::host::{CapabilityCollectionOptions, CapabilitySession, NativeAuthCache};
use crate::harness::registry;
use crate::models::harness_model::resolve_harness_model;
use crate::models::possible::{Provenance, SessionPossibleSource};
use crate::models::{self, ModelsCache};
use crate::routing::{self, RoutingInput, slug};
use crate::types::MarsContext;

use super::models::{CatalogViewArgs, ListArgs, SessionProbeResolver};
use super::models_common::{
    catalog_providers, load_merged_aliases, load_project_config_layers_optional,
    models_cache_ttl_hours,
};

fn catalog(
    ctx: &MarsContext,
    config: Option<&crate::config::LoadedProjectConfig>,
    refresh_models: bool,
    no_refresh_models: bool,
) -> Result<(ModelsCache, models::RefreshOutcome), MarsError> {
    let refresh = models::resolve_models_refresh_control(refresh_models, no_refresh_models)?;
    models::ensure_fresh_with_catalog_providers(
        &ctx.project_root.join(".mars"),
        models_cache_ttl_hours(config),
        refresh.catalog_mode,
        &catalog_providers(config),
    )
}

pub(super) fn run_catalog(
    args: &CatalogViewArgs,
    ctx: &MarsContext,
    json: bool,
) -> Result<i32, MarsError> {
    let config = load_project_config_layers_optional(&ctx.project_root)?;
    let (cache, outcome) = catalog(
        ctx,
        config.as_ref(),
        args.refresh_models,
        args.no_refresh_models,
    )?;
    if json {
        let entries = cache
            .models
            .iter()
            .map(|model| {
                serde_json::json!({
                    "id": model.id,
                    "provider": model.provider,
                    "release_date": model.release_date,
                    "description": model.description,
                    "context_window": model.context_window,
                    "max_output": model.max_output,
                    "cost_input": model.cost_input,
                    "cost_output": model.cost_output,
                    "cost_cache_read": model.cost_cache_read,
                    "cost_cache_write": model.cost_cache_write,
                    "cost_reasoning": model.cost_reasoning,
                })
            })
            .collect::<Vec<_>>();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "catalog": entries,
                "fetched_at": cache.fetched_at,
                "cache_available": cache.fetched_at.is_some(),
                "cache_warning": warning(&outcome),
            }))
            .unwrap()
        );
    } else {
        print_warning(&outcome);
        println!(
            "{:<18} {:<36} {:<12} {:>10} {:>10} DESCRIPTION",
            "PROVIDER", "MODEL", "RELEASE", "INPUT", "OUTPUT"
        );
        for model in &cache.models {
            println!(
                "{:<18} {:<36} {:<12} {:>10} {:>10} {}",
                model.provider,
                model.id,
                model.release_date.as_deref().unwrap_or("—"),
                cost(model.cost_input),
                cost(model.cost_output),
                model.description.as_deref().unwrap_or("")
            );
        }
    }
    Ok(0)
}

pub(super) fn run_aliases(
    args: &CatalogViewArgs,
    ctx: &MarsContext,
    json: bool,
) -> Result<i32, MarsError> {
    let config = load_project_config_layers_optional(&ctx.project_root)?;
    let aliases = load_merged_aliases(&ctx.project_root, config.as_ref())?;
    let (cache, outcome) = catalog(
        ctx,
        config.as_ref(),
        args.refresh_models,
        args.no_refresh_models,
    )?;
    let resolved = models::resolve_all_static(&aliases, &cache);
    if json {
        let rows = resolved
            .values()
            .map(|alias| {
                let mut value = serde_json::to_value(alias).unwrap();
                // Static aliases have no routing assessment. The shared
                // ResolvedAlias default `harness_source=unavailable` is not an
                // authored source and would mislead machine consumers here.
                value.as_object_mut().unwrap().remove("harness_source");
                value["resolved_model"] = serde_json::json!(alias.model_id);
                value["description"] = serde_json::json!(alias.description);
                value["mode"] = serde_json::json!(alias_mode(aliases.get(&alias.name)));
                value
            })
            .collect::<Vec<_>>();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "aliases": rows,
                "cache_available": cache.fetched_at.is_some(),
                "cache_warning": warning(&outcome),
            }))
            .unwrap()
        );
    } else {
        print_warning(&outcome);
        println!(
            "{:<18} {:<14} {:<36} DESCRIPTION",
            "ALIAS", "MODE", "RESOLVED"
        );
        for alias in resolved.values() {
            let mode = alias_mode(aliases.get(&alias.name));
            println!(
                "{:<18} {:<14} {:<36} {}",
                alias.name,
                mode,
                alias.model_id,
                alias.description.as_deref().unwrap_or("")
            );
        }
    }
    Ok(0)
}

pub(super) fn run_list(args: &ListArgs, ctx: &MarsContext, json: bool) -> Result<i32, MarsError> {
    let harness = args
        .harness
        .as_deref()
        .map(|name| {
            registry::parse(name).ok_or_else(|| {
                MarsError::Config(ConfigError::Invalid {
                    message: format!(
                        "unknown harness `{name}`; valid harnesses: {}",
                        registry::names().join(", ")
                    ),
                })
            })
        })
        .transpose()?;
    let config = load_project_config_layers_optional(&ctx.project_root)?;
    let defaults = crate::config::Settings::default();
    let settings = config
        .as_ref()
        .map(|c| &c.effective.settings)
        .unwrap_or(&defaults);
    let mut routing_settings = ResolvedRoutingSettings::from_settings(settings);
    routing_settings.target_source = config
        .as_ref()
        .map(|c| c.effective.target_source.clone())
        .unwrap_or_default();
    let routing_diagnostics = routing_settings.diagnostic_messages();
    if !json {
        for message in &routing_diagnostics {
            eprintln!("warning: {message}");
        }
    }
    let scope = &routing_settings.harness_scope;
    let rules = CuratedRules::load(&ctx.project_root)?;
    let aliases = load_merged_aliases(&ctx.project_root, config.as_ref())?;
    let (cache, outcome) = match catalog(
        ctx,
        config.as_ref(),
        args.refresh_models,
        args.no_refresh_models,
    ) {
        Ok(value) => value,
        Err(err @ MarsError::ModelCacheUnavailable { .. }) if json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "error": {"code": "model_cache_unavailable", "message": err.to_string()},
                    "routing_diagnostics": routing_diagnostics,
                }))
                .unwrap()
            );
            return Ok(1);
        }
        Err(err) => return Err(err),
    };
    let refresh =
        models::resolve_models_refresh_control(args.refresh_models, args.no_refresh_models)?;
    let mut session = CapabilitySession::collect(&CapabilityCollectionOptions {
        offline: models::is_mars_offline(),
        probe_refresh: refresh.probe_refresh,
    });
    let installed = session.installed_harnesses();
    let (possible, listing_diagnostics) = {
        let mut source = SessionPossibleSource::new(&cache, &mut session, scope);
        let rows = source.all_rows();
        let diagnostics = source
            .listing_failures()
            .into_iter()
            .map(|(harness, error)| match error {
                Some(error) => format!("{harness}: listing unavailable: {error}"),
                None => format!("{harness}: listing unavailable"),
            })
            .collect::<Vec<_>>();
        (rows, diagnostics)
    };
    let view = rules.project(&possible, scope);
    let diagnostics = view
        .diagnostics
        .iter()
        .cloned()
        .chain(listing_diagnostics)
        .collect::<Vec<_>>();
    let resolved = models::resolve_all_static(&aliases, &cache);
    let native_auth = NativeAuthCache::default();
    let catalog_slugs = models::catalog_model_slugs(&cache);
    let mut rows = Vec::new();
    for row in &view.rows {
        if !args.all && !matches!(row.decision, Decision::Shown { .. }) {
            continue;
        }
        if harness.is_some_and(|id| id != row.key.harness) {
            continue;
        }
        if args.r#match.as_ref().is_some_and(|pattern| {
            !models::glob_match(
                &pattern.to_lowercase(),
                &row.key.harness_model_id.to_lowercase(),
            )
        }) {
            continue;
        }
        let alias_names = resolved
            .values()
            .filter(|alias| {
                slug::model_ids_match(
                    slug::parse(&alias.model_id)
                        .map_or(alias.model_id.as_str(), |parts| parts.model_id),
                    &row.key.model_id,
                ) && row
                    .key
                    .provider
                    .as_deref()
                    .is_none_or(|provider| slug::providers_match(&alias.provider, provider))
            })
            .map(|alias| alias.name.clone())
            .collect::<Vec<_>>();
        let assessment = args
            .live
            .then(|| {
                assess(
                    row,
                    &installed,
                    &routing_settings,
                    &catalog_slugs,
                    &mut session,
                    &native_auth,
                )
            })
            .flatten();
        let (eligibility, reason) =
            assessment.map_or((None, None), |(state, reason)| (Some(state), reason));
        let (decision, tier) = match row.decision {
            Decision::Shown { tier } => ("shown", tier),
            Decision::Hidden { tier } => ("hidden", tier),
            Decision::Unmatched => ("unmatched", None),
        };
        let entry = serde_json::json!({
            "harness": row.key.harness,
            "harness_model_id": row.key.harness_model_id,
            "model_id": row.key.model_id,
            "provider": row.key.provider,
            "origin": row.origin,
            "provenance": row.possible.as_ref().map(|p| &p.provenance),
            "via": via(row),
            "aliases": alias_names,
            "curated": {"decision": decision, "tier": tier},
            "eligibility": eligibility,
            "reason": reason,
        });
        rows.push(entry);
    }
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "models": rows,
                "diagnostics": diagnostics,
                "routing_diagnostics": routing_diagnostics,
                "cache_warning": warning(&outcome),
            }))
            .unwrap()
        );
    } else {
        print_warning(&outcome);
        for diagnostic in &diagnostics {
            eprintln!("warning: {diagnostic}");
        }
        println!(
            "{:<10} {:<36} {:<16} {:<10} {:<18} {:<18} {:<12} ALIASES",
            "HARNESS", "MODEL", "PROVIDER", "ORIGIN", "VIA", "CURATION", "ELIGIBILITY"
        );
        for row in &rows {
            println!(
                "{:<10} {:<36} {:<16} {:<10} {:<18} {:<18} {:<12} {}",
                row["harness"].as_str().unwrap_or(""),
                row["harness_model_id"].as_str().unwrap_or(""),
                row["provider"].as_str().unwrap_or("—"),
                row["origin"].as_str().unwrap_or(""),
                row["via"].as_str().unwrap_or(""),
                curated_text(row),
                row["eligibility"].as_str().unwrap_or("—"),
                row["aliases"]
                    .as_array()
                    .map(|a| a
                        .iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join(","))
                    .unwrap_or_default()
            );
        }
    }
    Ok(0)
}

fn assess(
    row: &CuratedRow,
    installed: &std::collections::HashSet<String>,
    settings: &ResolvedRoutingSettings,
    catalog_slugs: &[String],
    session: &mut CapabilitySession,
    auth: &NativeAuthCache,
) -> Option<(String, Option<&'static str>)> {
    if !installed.contains(row.key.harness.as_str()) {
        return Some(("blocked".into(), Some("not_installed")));
    }
    // Probe-backed inventories are keyed by exact launch slug. A broad
    // provider constraint intentionally groups variants for ordinary routing,
    // but would score the wrong displayed row here.
    let model_id = if row.key.harness.native_provider().is_none() {
        &row.key.harness_model_id
    } else {
        &row.key.model_id
    };
    let evidence = routing::RoutingSettingsEvidence::new(
        model_id,
        row.key.provider.as_deref(),
        row.key.provider.as_deref(),
        installed,
        None,
        None,
        None,
        Some(catalog_slugs),
        settings,
    );
    let input: RoutingInput<'_> = evidence.routing_input();
    let mut probes = SessionProbeResolver { session };
    let result = routing::evaluate_fixed_harness_with_auth_and_probes(
        &input,
        row.key.harness.as_str(),
        &mut probes,
        |harness| auth.state(harness),
    );
    associated_verdict(row, &result)
}

fn associated_verdict(
    row: &CuratedRow,
    result: &routing::CandidateAssessment,
) -> Option<(String, Option<&'static str>)> {
    let projected = resolve_harness_model(
        row.key.harness,
        &row.key.model_id,
        result.chosen_slug.as_deref(),
        result.chosen_model.as_deref(),
        row.key.provider.as_deref(),
        row.key.provider.as_deref(),
    );
    if !slug::model_ids_match(&projected.harness_model_id, &row.key.harness_model_id) {
        return None;
    }
    Some((
        result.eligibility().label().into(),
        result.eligibility_reason(),
    ))
}

fn via(row: &CuratedRow) -> String {
    match row.possible.as_ref().map(|p| &p.provenance) {
        Some(Provenance::Enumerated {
            observed_at,
            latest_attempt_ok,
            ..
        }) => {
            let age = models::now_unix_secs_value().saturating_sub(*observed_at);
            let age = if age < 60 {
                format!("{age}s")
            } else if age < 3600 {
                format!("{}m", age / 60)
            } else {
                format!("{}h", age / 3600)
            };
            if *latest_attempt_ok {
                format!("listed {age}")
            } else {
                format!("listed {age}, refresh failed")
            }
        }
        Some(Provenance::Inferred { .. }) => "catalog".into(),
        None => "—".into(),
    }
}

fn curated_text(row: &serde_json::Value) -> String {
    let decision = row["curated"]["decision"].as_str().unwrap_or("unmatched");
    match row["curated"]["tier"].as_str() {
        Some(tier) => format!("{decision} ({tier})"),
        None => decision.to_string(),
    }
}

fn warning(outcome: &models::RefreshOutcome) -> Option<String> {
    match outcome {
        models::RefreshOutcome::StaleFallback { reason } => Some(format!(
            "models cache refresh failed: {reason}; using stale cache"
        )),
        _ => None,
    }
}
fn print_warning(outcome: &models::RefreshOutcome) {
    if let Some(value) = warning(outcome) {
        eprintln!("warning: {value}");
    }
}
fn cost(value: Option<f64>) -> String {
    value
        .map(|v| format!("${v:.2}"))
        .unwrap_or_else(|| "—".into())
}

fn alias_mode(alias: Option<&models::ModelAlias>) -> &'static str {
    match alias.map(|value| &value.spec) {
        Some(models::ModelSpec::Pinned { .. } | models::ModelSpec::PinnedWithMatch { .. }) => {
            "pinned"
        }
        Some(models::ModelSpec::AutoResolve { .. }) => "auto-resolve",
        None => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curation::{RowKey, RowOrigin};
    use crate::routing::MatchEvidence;

    #[test]
    fn live_verdict_cannot_cross_provider_or_model_spelling() {
        let row = CuratedRow {
            key: RowKey {
                harness: registry::HarnessId::OpenCode,
                harness_model_id: "openai/gpt-5".into(),
                provider: Some("openai".into()),
                model_id: "gpt-5".into(),
            },
            possible: None,
            origin: RowOrigin::Declared,
            decision: Decision::Shown { tier: None },
        };
        let mut assessment = routing::CandidateAssessment {
            auth: Some(crate::harness::host::AuthState::Unchecked),
            harness: "opencode".into(),
            installed: true,
            candidate_slugs: vec![],
            filtered_slugs: vec![],
            chosen_slug: Some("xai/gpt-5".into()),
            chosen_model: Some("gpt-5".into()),
            match_evidence: Some(MatchEvidence::Confirmed),
            skip_reason: None,
        };
        assert!(associated_verdict(&row, &assessment).is_none());
        assessment.chosen_slug = Some("openai/gpt-6".into());
        assert!(associated_verdict(&row, &assessment).is_none());
        assessment.chosen_slug = Some("openai/GPT.5".into());
        assert_eq!(
            associated_verdict(&row, &assessment).unwrap().0,
            "unverified"
        );
    }
}
