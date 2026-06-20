//! CLI handlers for `mars models` subcommands.
#![allow(clippy::print_literal)]

use clap::{Parser, Subcommand};
use indexmap::IndexMap;
use std::collections::HashSet;

use crate::build::policy::{PolicyInput, resolve_policy};
use crate::compiler::agents::{AgentProfile, parse_agent_content};
use crate::config::routing_settings::ResolvedRoutingSettings;
use crate::diagnostic::{Diagnostic, DiagnosticCollector, DiagnosticLevel};
use crate::error::{ConfigError, MarsError};
use crate::harness::host::{
    CapabilityCollectionOptions, CapabilitySession, CapabilitySnapshot, collect_capability_snapshot,
};
use crate::lock::ItemKind;
use crate::models::availability::{AvailabilityStatus, ModelAvailability};
use crate::models::probes::CursorProbeResult;
use crate::models::probes::OpenCodeProbeResult;
use crate::models::probes::PiProbeResult;
use crate::models::probes::ProbeRefreshMode;
use crate::models::probes::cursor_cache;
use crate::models::probes::opencode_cache::{self, CachedProbeOutcome};
use crate::models::probes::pi_cache;
use crate::models::{self, HarnessSource, ModelAlias, ModelSpec};
use crate::types::MarsContext;

/// Manage model aliases and the models cache.
#[derive(Debug, Parser)]
pub struct ModelsArgs {
    #[command(subcommand)]
    pub command: ModelsCommand,
}

#[derive(Debug, Subcommand)]
pub enum ModelsCommand {
    /// Fetch models from API and update the local cache.
    Refresh,
    /// List all model aliases (consumer + deps) with resolved IDs.
    List(ListArgs),
    /// Show resolution chain for a specific alias.
    Resolve(ResolveAliasArgs),
    /// Show prompting guidance for an agent or model alias.
    Prompting(PromptingArgs),
    /// Quick-add a pinned alias to mars.toml [models].
    Alias(AddAliasArgs),
    #[command(name = "__refresh-probe", hide = true)]
    RefreshProbe(RefreshProbeArgs),
}

#[derive(Debug, Parser)]
pub struct ListArgs {
    /// Show all alias candidates. Does NOT show raw catalog - use --catalog for that.
    #[arg(long, conflicts_with = "catalog", conflicts_with = "unavailable")]
    all: bool,
    /// Enable routed live availability details (selected harness, availability, runnable paths).
    #[arg(long)]
    live: bool,
    /// Refresh models.dev catalog and harness probes synchronously before running (blocks until complete).
    #[arg(long, conflicts_with = "no_refresh_models")]
    refresh_models: bool,
    /// Skip automatic models-cache refresh; use whatever's on disk (equivalent to MARS_OFFLINE=1).
    #[arg(long, conflicts_with = "refresh_models")]
    no_refresh_models: bool,
    /// Only show aliases matching these patterns (overrides config).
    #[arg(long, value_delimiter = ',')]
    include: Option<Vec<String>>,
    /// Hide aliases matching these patterns (overrides config).
    #[arg(long, value_delimiter = ',')]
    exclude: Option<Vec<String>>,
    /// Show raw models.dev cache entries (diagnostic view). Ignores aliases.
    #[arg(long, conflicts_with = "all")]
    catalog: bool,
    /// Include unavailable models in output (only affects --live output).
    #[arg(long)]
    unavailable: bool,
}

#[derive(Debug, Parser)]
pub struct ResolveAliasArgs {
    /// Alias name to resolve.
    pub name: String,
    /// Refresh models.dev catalog and harness probes synchronously before running (blocks until complete).
    #[arg(long, conflicts_with = "no_refresh_models")]
    refresh_models: bool,
    /// Skip automatic models-cache refresh; use whatever's on disk (equivalent to MARS_OFFLINE=1).
    #[arg(long, conflicts_with = "refresh_models")]
    no_refresh_models: bool,
}

#[derive(Debug, Parser)]
pub struct RefreshProbeArgs {
    #[arg(long)]
    target: String,
}

#[derive(Debug, Parser)]
#[command(
    after_help = "Examples:\n  mars models prompting @explorer\n  mars models prompting gpt55"
)]
pub struct PromptingArgs {
    /// Agent name/ref or model alias to look up prompting guidance for.
    pub reference: String,
    /// Refresh models.dev catalog and harness probes synchronously before resolving an agent.
    #[arg(long, conflicts_with = "no_refresh_models")]
    refresh_models: bool,
    /// Skip automatic models-cache refresh; use whatever is on disk.
    #[arg(long, conflicts_with = "refresh_models")]
    no_refresh_models: bool,
}

#[derive(Debug, Parser)]
pub struct AddAliasArgs {
    /// Alias name.
    pub name: String,
    /// Model ID to pin.
    pub model_id: String,
    /// Harness for this alias (default: claude).
    #[arg(long, default_value = "claude")]
    pub harness: String,
    /// Optional description.
    #[arg(long)]
    pub description: Option<String>,
}

pub fn run(args: &ModelsArgs, ctx: &MarsContext, json: bool) -> Result<i32, MarsError> {
    match &args.command {
        ModelsCommand::Refresh => run_refresh(ctx, json),
        ModelsCommand::List(args) => run_list(args, ctx, json),
        ModelsCommand::Resolve(a) => run_resolve(a, ctx, json),
        ModelsCommand::Prompting(a) => run_prompting(a, ctx, json),
        ModelsCommand::Alias(a) => run_alias(a, ctx, json),
        ModelsCommand::RefreshProbe(a) => run_refresh_probe(a),
    }
}

fn mars_dir(ctx: &MarsContext) -> std::path::PathBuf {
    ctx.project_root.join(".mars")
}

fn collect_models_capability_snapshot(
    refresh: &models::ModelsRefreshControl,
) -> CapabilitySnapshot {
    collect_capability_snapshot(&CapabilityCollectionOptions {
        offline: models::is_mars_offline(),
        probe_refresh: refresh.probe_refresh,
    })
}

fn run_refresh(ctx: &MarsContext, json: bool) -> Result<i32, MarsError> {
    let mars = mars_dir(ctx);
    let project_config = load_project_config_layers_optional(&ctx.project_root)?;
    let ttl = models_cache_ttl_hours(project_config.as_ref());
    eprint!("Fetching models catalog... ");

    let (cache, outcome) = models::ensure_fresh(&mars, ttl, models::RefreshMode::Force)?;
    let count = cache.models.len();
    let cache_warning = cache_warning(&outcome);

    if let Some(warning) = cache_warning.as_deref() {
        eprintln!("warning: {warning}");
    } else if !json {
        eprintln!("done.");
    }

    if json {
        let out = serde_json::json!({
            "status": "ok",
            "models_count": count,
            "fetched_at": cache.fetched_at,
        });
        let mut out = out;
        if let Some(warning) = cache_warning.as_deref() {
            out["cache_warning"] = serde_json::json!(warning);
        }
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else {
        if cache_warning.is_some() {
            println!(
                "Using stale models cache with {} models in .mars/models-cache.json",
                count
            );
        } else {
            println!("Cached {} models in .mars/models-cache.json", count);
        }
    }

    Ok(0)
}

fn run_list(args: &ListArgs, ctx: &MarsContext, json: bool) -> Result<i32, MarsError> {
    let mars = mars_dir(ctx);
    let project_config = load_project_config_layers_optional(&ctx.project_root)?;
    let ttl = models_cache_ttl_hours(project_config.as_ref());
    let refresh =
        models::resolve_models_refresh_control(args.refresh_models, args.no_refresh_models)?;
    let mode = refresh.catalog_mode;
    let default_settings = crate::config::Settings::default();
    let settings = project_config
        .as_ref()
        .map(|loaded| &loaded.effective.settings)
        .unwrap_or(&default_settings);
    let routing_settings = ResolvedRoutingSettings::from_settings(settings);
    let routing_diagnostics = routing_settings.diagnostic_messages();
    let visibility = effective_visibility(project_config.as_ref(), args);
    if !json {
        emit_routing_settings_warnings(&routing_diagnostics);
    }

    // Load runtime aliases before cache refresh so legacy locks that predate
    // dependency alias authority fail with an explicit sync remediation instead
    // of surfacing an unrelated cache error first.
    let merged = (!args.catalog)
        .then(|| load_merged_aliases(&ctx.project_root, project_config.as_ref()))
        .transpose()?;

    let (cache, outcome) = match ensure_fresh_or_json_error(&mars, ttl, mode, json)? {
        FreshOrJsonError::Fresh(cache, outcome) => (cache, outcome),
        FreshOrJsonError::JsonError(error_message) => {
            let mut out = serde_json::json!({
                "error": error_message,
            });
            add_routing_diagnostics_json(&mut out, &routing_diagnostics);
            println!("{}", serde_json::to_string_pretty(&out).unwrap());
            return Ok(1);
        }
    };
    if args.catalog {
        if !args.live {
            return run_list_catalog_static(ListCatalogStaticInput {
                cache: &cache,
                outcome: &outcome,
                visibility: &visibility,
                routing_diagnostics: &routing_diagnostics,
                json,
            });
        }
        let capability_snapshot = collect_models_capability_snapshot(&refresh);
        return run_list_catalog(ListCatalogInput {
            cache: &cache,
            outcome: &outcome,
            args,
            visibility: &visibility,
            routing_settings: &routing_settings,
            routing_diagnostics: &routing_diagnostics,
            capability_snapshot: &capability_snapshot,
            json,
        });
    }

    let merged = merged.expect("non-catalog models list loaded runtime aliases");
    if args.all {
        if !args.live {
            return run_list_all_static(
                &merged,
                &cache,
                &outcome,
                &visibility,
                &routing_diagnostics,
                json,
            );
        }
        let capability_snapshot = collect_models_capability_snapshot(&refresh);
        let installed = capability_snapshot.installed_harnesses();
        let is_offline = capability_snapshot.offline;
        let opencode_probe_result = capability_snapshot.opencode.result().cloned();
        let pi_probe_result = capability_snapshot.pi.result().cloned();
        let cursor_probe_result = capability_snapshot.cursor.result().cloned();
        let catalog_slugs = models::catalog_model_slugs(&cache);
        let availability_ctx = AvailabilityContext {
            installed: &installed,
            opencode_probe_result: opencode_probe_result.as_ref(),
            pi_probe_result: pi_probe_result.as_ref(),
            cursor_probe_result: cursor_probe_result.as_ref(),
            catalog_model_slugs: Some(catalog_slugs.as_slice()),
            is_offline,
            routing_settings: &routing_settings,
        };
        return run_list_all(
            &merged,
            &cache,
            &outcome,
            &visibility,
            availability_ctx,
            &routing_diagnostics,
            json,
        );
    }

    if !args.live {
        return run_list_aliases_static(
            &merged,
            &cache,
            &outcome,
            &visibility,
            &routing_diagnostics,
            json,
        );
    }

    let capability_snapshot = collect_models_capability_snapshot(&refresh);
    let installed = capability_snapshot.installed_harnesses();
    let is_offline = capability_snapshot.offline;
    let opencode_probe_result = capability_snapshot.opencode.result().cloned();
    let pi_probe_result = capability_snapshot.pi.result().cloned();
    let cursor_probe_result = capability_snapshot.cursor.result().cloned();
    let cache_warning = cache_warning(&outcome);
    let mut diag = DiagnosticCollector::new();
    let catalog_slugs = models::catalog_model_slugs(&cache);

    let mut resolved = models::resolve_all_with_probe(
        &merged,
        &cache,
        &mut diag,
        opencode_probe_result.as_ref(),
        pi_probe_result.as_ref(),
        cursor_probe_result.as_ref(),
    );
    apply_routing_settings_to_resolved_aliases(
        &mut resolved,
        &merged,
        &installed,
        opencode_probe_result.as_ref(),
        pi_probe_result.as_ref(),
        cursor_probe_result.as_ref(),
        Some(catalog_slugs.as_slice()),
        &routing_settings,
    );
    annotate_resolved_availability(
        &mut resolved,
        &installed,
        opencode_probe_result.as_ref(),
        pi_probe_result.as_ref(),
        cursor_probe_result.as_ref(),
        is_offline,
    );
    if !args.unavailable {
        prune_unavailable(&mut resolved);
    }

    // Build effective visibility: CLI overrides config entirely.
    let resolved = models::filter_by_visibility(resolved, &visibility);

    if json {
        let entries: Vec<serde_json::Value> = resolved
            .values()
            .map(|r| {
                let mode = mode_for_alias(merged.get(&r.name).map(|a| &a.spec));
                let mut obj = serde_json::json!({
                    "name": r.name,
                    "harness": r.harness,
                    "harness_source": r.harness_source,
                    "harness_candidates": r.harness_candidates,
                    "provider": r.provider,
                    "mode": mode,
                    "model_id": r.model_id,
                    "resolved_model": r.model_id,
                    "description": r.description,
                });
                if let Some(error) = unavailable_harness_error(r) {
                    obj["error"] = serde_json::json!(error);
                }
                if let Some(default_effort) = &r.default_effort {
                    obj["default_effort"] = serde_json::json!(default_effort);
                }
                if let Some(autocompact) = r.autocompact {
                    obj["autocompact"] = serde_json::json!(autocompact);
                }
                if let Some(autocompact_pct) = r.autocompact_pct {
                    obj["autocompact_pct"] = serde_json::json!(autocompact_pct);
                }
                if let Some(model) = cache.models.iter().find(|model| model.id == r.model_id) {
                    add_cost_json_fields(&mut obj, model);
                }
                add_availability_json_fields(&mut obj, r.availability.as_ref());
                obj
            })
            .collect();
        let mut out = serde_json::json!({
            "aliases": entries,
            "cache_available": cache.fetched_at.is_some(),
        });
        add_probe_results_json(
            &mut out,
            opencode_probe_result.as_ref(),
            pi_probe_result.as_ref(),
            cursor_probe_result.as_ref(),
        );
        if let Some(warning) = cache_warning.as_deref() {
            out["cache_warning"] = serde_json::json!(warning);
        }
        if let Some(diagnostics) = drain_diagnostics_json(&mut diag) {
            out["diagnostics"] = diagnostics;
        }
        add_routing_diagnostics_json(&mut out, &routing_diagnostics);
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else {
        if let Some(warning) = cache_warning.as_deref() {
            eprintln!("warning: {warning}");
        }
        // Table output
        println!(
            "{:<12} {:<10} {:<14} {:<30} {:<12} {}",
            "ALIAS", "HARNESS", "MODE", "RESOLVED", "AVAILABILITY", "DESCRIPTION"
        );
        for r in resolved.values() {
            let harness = r.harness.as_deref().unwrap_or("—");
            let mode = mode_for_alias(merged.get(&r.name).map(|a| &a.spec));
            let availability = availability_status_label(r.availability.as_ref());
            let desc = r.description.clone().unwrap_or_default();
            println!(
                "{:<12} {:<10} {:<14} {:<30} {:<12} {}",
                r.name, harness, mode, r.model_id, availability, desc
            );
        }
        emit_text_diagnostics(&mut diag);
    }

    Ok(0)
}

#[derive(Debug, Clone)]
struct ListModelEntry {
    id: String,
    provider: String,
    release_date: Option<String>,
    harness: Option<String>,
    harness_source: HarnessSource,
    harness_candidates: Vec<String>,
    description: Option<String>,
    cost_input: Option<f64>,
    cost_output: Option<f64>,
    cost_cache_read: Option<f64>,
    cost_cache_write: Option<f64>,
    cost_reasoning: Option<f64>,
    matched_aliases: Vec<String>,
    availability: Option<ModelAvailability>,
}

#[derive(Clone, Copy)]
struct AvailabilityContext<'a> {
    installed: &'a HashSet<String>,
    opencode_probe_result: Option<&'a OpenCodeProbeResult>,
    pi_probe_result: Option<&'a PiProbeResult>,
    cursor_probe_result: Option<&'a CursorProbeResult>,
    catalog_model_slugs: Option<&'a [String]>,
    is_offline: bool,
    routing_settings: &'a ResolvedRoutingSettings,
}

struct ResolveRuntime<'a> {
    cache: &'a models::ModelsCache,
    catalog_model_slugs: &'a [String],
    outcome: &'a models::RefreshOutcome,
    installed: &'a HashSet<String>,
    routing_settings: &'a ResolvedRoutingSettings,
    probe_refresh: ProbeRefreshMode,
}

struct RouteTraceInput<'a> {
    model_id: &'a str,
    provider_for_order: &'a str,
    provider_constraint: Option<&'a str>,
    installed: &'a HashSet<String>,
    opencode_probe_result: Option<&'a OpenCodeProbeResult>,
    pi_probe_result: Option<&'a PiProbeResult>,
    cursor_probe_result: Option<&'a CursorProbeResult>,
    catalog_model_slugs: Option<&'a [String]>,
    routing_settings: &'a ResolvedRoutingSettings,
}

struct SessionProbeResolver<'a> {
    session: &'a mut CapabilitySession,
}

impl crate::routing::ProbeResolver for SessionProbeResolver<'_> {
    fn opencode_probe_result(&mut self) -> Option<OpenCodeProbeResult> {
        self.session.opencode_probe_result()
    }

    fn pi_probe_result(&mut self) -> Option<PiProbeResult> {
        self.session.pi_probe_result()
    }

    fn cursor_probe_result(&mut self) -> Option<CursorProbeResult> {
        self.session.cursor_probe_result()
    }
}

struct ListCatalogInput<'a> {
    cache: &'a models::ModelsCache,
    outcome: &'a models::RefreshOutcome,
    args: &'a ListArgs,
    visibility: &'a crate::config::ModelVisibility,
    routing_settings: &'a ResolvedRoutingSettings,
    routing_diagnostics: &'a [String],
    capability_snapshot: &'a CapabilitySnapshot,
    json: bool,
}

struct ListCatalogStaticInput<'a> {
    cache: &'a models::ModelsCache,
    outcome: &'a models::RefreshOutcome,
    visibility: &'a crate::config::ModelVisibility,
    routing_diagnostics: &'a [String],
    json: bool,
}

struct OutputResolvedInput<'a> {
    name: &'a str,
    resolved: &'a models::ResolvedAlias,
    source: &'a str,
    route_trace: &'a crate::routing::RoutingTrace,
    outcome: &'a models::RefreshOutcome,
    cache_outcome: &'a CachedProbeOutcome,
    probe_refresh: ProbeRefreshMode,
    routing_diagnostics: &'a [String],
    json: bool,
}

struct OutputPassthroughInput<'a> {
    name: &'a str,
    outcome: &'a models::RefreshOutcome,
    is_offline: bool,
    installed: &'a HashSet<String>,
    capability_session: &'a mut CapabilitySession,
    catalog_model_slugs: Option<&'a [String]>,
    routing_settings: &'a ResolvedRoutingSettings,
    cache_error: Option<&'a str>,
    routing_diagnostics: &'a [String],
    json: bool,
}

fn run_list_all(
    merged: &IndexMap<String, ModelAlias>,
    cache: &models::ModelsCache,
    outcome: &models::RefreshOutcome,
    visibility: &crate::config::ModelVisibility,
    availability_ctx: AvailabilityContext<'_>,
    routing_diagnostics: &[String],
    json: bool,
) -> Result<i32, MarsError> {
    let cache_warning = cache_warning(outcome);
    let models = collect_all_model_entries(merged, cache, availability_ctx);
    let models = filter_model_entries_by_visibility(models, visibility);

    if json {
        let entries: Vec<serde_json::Value> = models
            .into_iter()
            .map(|model| {
                let mut obj = serde_json::json!({
                    "id": model.id,
                    "provider": model.provider,
                    "release_date": model.release_date,
                    "harness": model.harness,
                    "harness_source": model.harness_source,
                    "harness_candidates": model.harness_candidates,
                    "description": model.description,
                    "cost_input": model.cost_input,
                    "cost_output": model.cost_output,
                    "cost_cache_read": model.cost_cache_read,
                    "cost_cache_write": model.cost_cache_write,
                    "cost_reasoning": model.cost_reasoning,
                    "matched_aliases": model.matched_aliases,
                });
                add_availability_json_fields(&mut obj, model.availability.as_ref());
                obj
            })
            .collect();
        let mut out = serde_json::json!({
            "models": entries,
            "cache_available": cache.fetched_at.is_some(),
        });
        add_probe_results_json(
            &mut out,
            availability_ctx.opencode_probe_result,
            availability_ctx.pi_probe_result,
            availability_ctx.cursor_probe_result,
        );
        if let Some(warning) = cache_warning.as_deref() {
            out["cache_warning"] = serde_json::json!(warning);
        }
        add_routing_diagnostics_json(&mut out, routing_diagnostics);
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else {
        if let Some(warning) = cache_warning.as_deref() {
            eprintln!("warning: {warning}");
        }
        println!(
            "{:<10} {:<34} {:<12} {:<10} {:<12} {}",
            "PROVIDER", "MODEL ID", "RELEASE", "HARNESS", "AVAILABILITY", "ALIASES"
        );
        for model in models {
            let release = model.release_date.as_deref().unwrap_or("—");
            let harness = model.harness.as_deref().unwrap_or("—");
            let availability = availability_status_label(model.availability.as_ref());
            println!(
                "{:<10} {:<34} {:<12} {:<10} {:<12} {}",
                model.provider,
                model.id,
                release,
                harness,
                availability,
                model.matched_aliases.join(",")
            );
        }
    }

    Ok(0)
}

fn run_list_aliases_static(
    merged: &IndexMap<String, ModelAlias>,
    cache: &models::ModelsCache,
    outcome: &models::RefreshOutcome,
    visibility: &crate::config::ModelVisibility,
    routing_diagnostics: &[String],
    json: bool,
) -> Result<i32, MarsError> {
    let cache_warning = cache_warning(outcome);
    let resolved = models::resolve_all_static(merged, cache);
    let resolved = models::filter_by_visibility(resolved, visibility);

    if json {
        let entries: Vec<serde_json::Value> = resolved
            .values()
            .map(|r| {
                let mode = mode_for_alias(merged.get(&r.name).map(|a| &a.spec));
                serde_json::json!({
                    "name": r.name,
                    "provider": r.provider,
                    "mode": mode,
                    "model_id": r.model_id,
                    "resolved_model": r.model_id,
                    "description": r.description,
                })
            })
            .collect();
        let mut out = serde_json::json!({
            "aliases": entries,
            "cache_available": cache.fetched_at.is_some(),
        });
        if let Some(warning) = cache_warning.as_deref() {
            out["cache_warning"] = serde_json::json!(warning);
        }
        add_routing_diagnostics_json(&mut out, routing_diagnostics);
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
        return Ok(0);
    }

    if let Some(warning) = cache_warning.as_deref() {
        eprintln!("warning: {warning}");
    }
    println!(
        "{:<12} {:<14} {:<30} {}",
        "ALIAS", "MODE", "RESOLVED", "DESCRIPTION"
    );
    for r in resolved.values() {
        let mode = mode_for_alias(merged.get(&r.name).map(|a| &a.spec));
        let desc = r.description.clone().unwrap_or_default();
        println!("{:<12} {:<14} {:<30} {}", r.name, mode, r.model_id, desc);
    }
    Ok(0)
}

fn run_list_all_static(
    merged: &IndexMap<String, ModelAlias>,
    cache: &models::ModelsCache,
    outcome: &models::RefreshOutcome,
    visibility: &crate::config::ModelVisibility,
    routing_diagnostics: &[String],
    json: bool,
) -> Result<i32, MarsError> {
    let cache_warning = cache_warning(outcome);
    let models = collect_all_model_entries_static(merged, cache);
    let models = filter_model_entries_by_visibility(models, visibility);

    if json {
        let entries: Vec<serde_json::Value> = models
            .into_iter()
            .map(|model| {
                serde_json::json!({
                    "id": model.id,
                    "provider": model.provider,
                    "release_date": model.release_date,
                    "description": model.description,
                    "cost_input": model.cost_input,
                    "cost_output": model.cost_output,
                    "cost_cache_read": model.cost_cache_read,
                    "cost_cache_write": model.cost_cache_write,
                    "cost_reasoning": model.cost_reasoning,
                    "matched_aliases": model.matched_aliases,
                })
            })
            .collect();
        let mut out = serde_json::json!({
            "models": entries,
            "cache_available": cache.fetched_at.is_some(),
        });
        if let Some(warning) = cache_warning.as_deref() {
            out["cache_warning"] = serde_json::json!(warning);
        }
        add_routing_diagnostics_json(&mut out, routing_diagnostics);
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
        return Ok(0);
    }

    if let Some(warning) = cache_warning.as_deref() {
        eprintln!("warning: {warning}");
    }
    println!(
        "{:<10} {:<34} {:<12} {}",
        "PROVIDER", "MODEL ID", "RELEASE", "ALIASES"
    );
    for model in models {
        let release = model.release_date.as_deref().unwrap_or("—");
        println!(
            "{:<10} {:<34} {:<12} {}",
            model.provider,
            model.id,
            release,
            model.matched_aliases.join(",")
        );
    }
    Ok(0)
}

fn run_list_catalog_static(input: ListCatalogStaticInput<'_>) -> Result<i32, MarsError> {
    let ListCatalogStaticInput {
        cache,
        outcome,
        visibility,
        routing_diagnostics,
        json,
    } = input;
    let cache_warning = cache_warning(outcome);
    let models = collect_catalog_model_entries_static(cache);
    let models = filter_model_entries_by_visibility(models, visibility);

    if json {
        let entries: Vec<serde_json::Value> = models
            .into_iter()
            .map(|model| {
                serde_json::json!({
                    "provider": model.provider,
                    "id": model.id,
                    "release_date": model.release_date,
                    "description": model.description,
                    "cost_input": model.cost_input,
                    "cost_output": model.cost_output,
                    "cost_cache_read": model.cost_cache_read,
                    "cost_cache_write": model.cost_cache_write,
                    "cost_reasoning": model.cost_reasoning,
                })
            })
            .collect();
        let mut out = serde_json::json!({
            "catalog": entries,
            "cache_available": cache.fetched_at.is_some(),
        });
        if let Some(warning) = cache_warning.as_deref() {
            out["cache_warning"] = serde_json::json!(warning);
        }
        add_routing_diagnostics_json(&mut out, routing_diagnostics);
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
        return Ok(0);
    }

    if let Some(warning) = cache_warning.as_deref() {
        eprintln!("warning: {warning}");
    }
    println!("{:<10} {:<34} {:<12}", "PROVIDER", "MODEL ID", "RELEASE");
    for model in models {
        let release = model.release_date.as_deref().unwrap_or("—");
        println!("{:<10} {:<34} {:<12}", model.provider, model.id, release);
    }
    Ok(0)
}

fn run_list_catalog(input: ListCatalogInput<'_>) -> Result<i32, MarsError> {
    let ListCatalogInput {
        cache,
        outcome,
        args,
        visibility,
        routing_settings,
        routing_diagnostics,
        capability_snapshot,
        json,
    } = input;
    let cache_warning = cache_warning(outcome);
    let installed = capability_snapshot.installed_harnesses();
    let is_offline = capability_snapshot.offline || args.no_refresh_models;
    let probe_result = capability_snapshot.opencode.result().cloned();
    let pi_probe_result = capability_snapshot.pi.result().cloned();
    let cursor_probe_result = capability_snapshot.cursor.result().cloned();
    let catalog_slugs = models::catalog_model_slugs(cache);
    let availability_ctx = AvailabilityContext {
        installed: &installed,
        opencode_probe_result: probe_result.as_ref(),
        pi_probe_result: pi_probe_result.as_ref(),
        cursor_probe_result: cursor_probe_result.as_ref(),
        catalog_model_slugs: Some(catalog_slugs.as_slice()),
        is_offline,
        routing_settings,
    };
    let models = collect_catalog_model_entries(cache, availability_ctx);
    let models = filter_model_entries_by_visibility(models, visibility);

    if json {
        let entries: Vec<serde_json::Value> = models
            .into_iter()
            .map(|model| {
                let mut obj = serde_json::json!({
                    "id": model.id,
                    "provider": model.provider,
                    "release_date": model.release_date,
                    "harness": model.harness,
                    "harness_source": model.harness_source,
                    "harness_candidates": model.harness_candidates,
                    "description": model.description,
                    "cost_input": model.cost_input,
                    "cost_output": model.cost_output,
                    "cost_cache_read": model.cost_cache_read,
                    "cost_cache_write": model.cost_cache_write,
                    "cost_reasoning": model.cost_reasoning,
                });
                add_availability_json_fields(&mut obj, model.availability.as_ref());
                obj
            })
            .collect();
        let mut out = serde_json::json!({
            "models": entries,
            "cache_available": cache.fetched_at.is_some(),
        });
        add_probe_results_json(
            &mut out,
            probe_result.as_ref(),
            pi_probe_result.as_ref(),
            cursor_probe_result.as_ref(),
        );
        if let Some(warning) = cache_warning.as_deref() {
            out["cache_warning"] = serde_json::json!(warning);
        }
        add_routing_diagnostics_json(&mut out, routing_diagnostics);
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else {
        if let Some(warning) = cache_warning.as_deref() {
            eprintln!("warning: {warning}");
        }
        println!(
            "{:<10} {:<34} {:<12} {:<10} {:<12}",
            "PROVIDER", "MODEL ID", "RELEASE", "HARNESS", "AVAILABILITY"
        );
        for model in models {
            let release = model.release_date.as_deref().unwrap_or("—");
            let harness = model.harness.as_deref().unwrap_or("—");
            let availability = availability_status_label(model.availability.as_ref());
            println!(
                "{:<10} {:<34} {:<12} {:<10} {:<12}",
                model.provider, model.id, release, harness, availability
            );
        }
    }

    Ok(0)
}

fn collect_all_model_entries(
    merged: &IndexMap<String, ModelAlias>,
    cache: &models::ModelsCache,
    availability_ctx: AvailabilityContext<'_>,
) -> Vec<ListModelEntry> {
    let mut by_model_id: IndexMap<String, ListModelEntry> = IndexMap::new();

    for (alias_name, alias) in merged {
        match &alias.spec {
            ModelSpec::AutoResolve {
                provider,
                match_patterns,
                exclude_patterns,
            } => {
                for matched in models::auto_resolve_all(
                    provider.as_deref(),
                    match_patterns,
                    exclude_patterns,
                    cache,
                ) {
                    append_alias_match(&mut by_model_id, matched, availability_ctx, alias_name);
                }
            }
            ModelSpec::Pinned {
                model, provider, ..
            } => {
                if let Some(matched) = cache
                    .models
                    .iter()
                    .find(|cache_model| cache_model.id == *model)
                {
                    append_alias_match(&mut by_model_id, matched, availability_ctx, alias_name);
                } else {
                    append_pinned_alias_match(
                        &mut by_model_id,
                        model,
                        provider.as_deref(),
                        alias.description.as_deref(),
                        availability_ctx,
                        alias_name,
                    );
                }
            }
            ModelSpec::PinnedWithMatch {
                model,
                provider,
                match_patterns,
                exclude_patterns,
            } => {
                if let Some(matched) = cache
                    .models
                    .iter()
                    .find(|cache_model| cache_model.id == *model)
                {
                    append_alias_match(&mut by_model_id, matched, availability_ctx, alias_name);
                } else {
                    append_pinned_alias_match(
                        &mut by_model_id,
                        model,
                        provider.as_deref(),
                        alias.description.as_deref(),
                        availability_ctx,
                        alias_name,
                    );
                }

                let provider_for_discovery = provider
                    .as_deref()
                    .or_else(|| models::infer_provider_from_model_id(model));
                for matched in models::auto_resolve_all(
                    provider_for_discovery,
                    match_patterns,
                    exclude_patterns,
                    cache,
                ) {
                    append_alias_match(&mut by_model_id, matched, availability_ctx, alias_name);
                }
            }
        }
    }

    let mut out: Vec<ListModelEntry> = by_model_id.into_values().collect();
    sort_list_model_entries(&mut out);
    out
}

fn collect_catalog_model_entries(
    cache: &models::ModelsCache,
    availability_ctx: AvailabilityContext<'_>,
) -> Vec<ListModelEntry> {
    let mut out: Vec<ListModelEntry> = cache
        .models
        .iter()
        .map(|model| model_entry_for_cached(model, availability_ctx))
        .collect();
    sort_list_model_entries(&mut out);
    out
}

fn collect_all_model_entries_static(
    merged: &IndexMap<String, ModelAlias>,
    cache: &models::ModelsCache,
) -> Vec<ListModelEntry> {
    let mut by_model_id: IndexMap<String, ListModelEntry> = IndexMap::new();

    for (alias_name, alias) in merged {
        match &alias.spec {
            ModelSpec::AutoResolve {
                provider,
                match_patterns,
                exclude_patterns,
            } => {
                for matched in models::auto_resolve_all(
                    provider.as_deref(),
                    match_patterns,
                    exclude_patterns,
                    cache,
                ) {
                    let entry = by_model_id
                        .entry(matched.id.clone())
                        .or_insert_with(|| model_entry_for_cached_static(matched));
                    append_alias_name(entry, alias_name);
                }
            }
            ModelSpec::Pinned {
                model, provider, ..
            } => {
                let entry = by_model_id.entry(model.clone()).or_insert_with(|| {
                    cache
                        .models
                        .iter()
                        .find(|cache_model| cache_model.id == *model)
                        .map(model_entry_for_cached_static)
                        .unwrap_or_else(|| {
                            model_entry_for_pinned_static(
                                model,
                                provider.as_deref(),
                                alias.description.as_deref(),
                            )
                        })
                });
                append_alias_name(entry, alias_name);
            }
            ModelSpec::PinnedWithMatch {
                model,
                provider,
                match_patterns,
                exclude_patterns,
            } => {
                let entry = by_model_id.entry(model.clone()).or_insert_with(|| {
                    cache
                        .models
                        .iter()
                        .find(|cache_model| cache_model.id == *model)
                        .map(model_entry_for_cached_static)
                        .unwrap_or_else(|| {
                            model_entry_for_pinned_static(
                                model,
                                provider.as_deref(),
                                alias.description.as_deref(),
                            )
                        })
                });
                append_alias_name(entry, alias_name);

                let provider_for_discovery = provider
                    .as_deref()
                    .or_else(|| models::infer_provider_from_model_id(model));
                for matched in models::auto_resolve_all(
                    provider_for_discovery,
                    match_patterns,
                    exclude_patterns,
                    cache,
                ) {
                    let entry = by_model_id
                        .entry(matched.id.clone())
                        .or_insert_with(|| model_entry_for_cached_static(matched));
                    append_alias_name(entry, alias_name);
                }
            }
        }
    }

    let mut out: Vec<ListModelEntry> = by_model_id.into_values().collect();
    sort_list_model_entries(&mut out);
    out
}

fn collect_catalog_model_entries_static(cache: &models::ModelsCache) -> Vec<ListModelEntry> {
    let mut out: Vec<ListModelEntry> = cache
        .models
        .iter()
        .map(model_entry_for_cached_static)
        .collect();
    sort_list_model_entries(&mut out);
    out
}

fn append_alias_match(
    by_model_id: &mut IndexMap<String, ListModelEntry>,
    model: &models::CachedModel,
    availability_ctx: AvailabilityContext<'_>,
    alias_name: &str,
) {
    let entry = by_model_id
        .entry(model.id.clone())
        .or_insert_with(|| model_entry_for_cached(model, availability_ctx));

    append_alias_name(entry, alias_name);
}

fn append_pinned_alias_match(
    by_model_id: &mut IndexMap<String, ListModelEntry>,
    model_id: &str,
    provider: Option<&str>,
    description: Option<&str>,
    availability_ctx: AvailabilityContext<'_>,
    alias_name: &str,
) {
    let entry = by_model_id.entry(model_id.to_string()).or_insert_with(|| {
        model_entry_for_pinned(model_id, provider, description, availability_ctx)
    });

    append_alias_name(entry, alias_name);
}

fn append_alias_name(entry: &mut ListModelEntry, alias_name: &str) {
    if !entry
        .matched_aliases
        .iter()
        .any(|existing| existing == alias_name)
    {
        entry.matched_aliases.push(alias_name.to_string());
    }
}

fn model_entry_for_cached(
    model: &models::CachedModel,
    availability_ctx: AvailabilityContext<'_>,
) -> ListModelEntry {
    model_entry_for_cached_with_auth(
        model,
        availability_ctx,
        models::harness::native_harness_authenticated,
    )
}

fn model_entry_for_cached_with_auth<F>(
    model: &models::CachedModel,
    availability_ctx: AvailabilityContext<'_>,
    auth_check: F,
) -> ListModelEntry
where
    F: Fn(&str) -> bool,
{
    let (harness, harness_source) =
        resolve_harness_with_routing_auth(&model.provider, &model.id, availability_ctx, auth_check);

    ListModelEntry {
        id: model.id.clone(),
        provider: model.provider.clone(),
        release_date: model.release_date.clone(),
        harness,
        harness_source,
        harness_candidates: models::harness::harness_candidates_for_provider(&model.provider),
        description: model.description.clone(),
        cost_input: model.cost_input,
        cost_output: model.cost_output,
        cost_cache_read: model.cost_cache_read,
        cost_cache_write: model.cost_cache_write,
        cost_reasoning: model.cost_reasoning,
        matched_aliases: Vec::new(),
        availability: Some(models::availability::classify_model(
            &model.id,
            &model.provider,
            availability_ctx.installed,
            availability_ctx.opencode_probe_result,
            availability_ctx.pi_probe_result,
            availability_ctx.cursor_probe_result,
            availability_ctx.is_offline,
        )),
    }
}

fn model_entry_for_pinned(
    model_id: &str,
    provider: Option<&str>,
    description: Option<&str>,
    availability_ctx: AvailabilityContext<'_>,
) -> ListModelEntry {
    let provider = provider
        .map(str::to_string)
        .or_else(|| models::infer_provider_from_model_id(model_id).map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string());
    let (harness, harness_source) =
        resolve_harness_with_routing(&provider, model_id, availability_ctx);

    ListModelEntry {
        id: model_id.to_string(),
        provider: provider.clone(),
        release_date: None,
        harness,
        harness_source,
        harness_candidates: models::harness::harness_candidates_for_provider(&provider),
        description: description.map(str::to_string),
        cost_input: None,
        cost_output: None,
        cost_cache_read: None,
        cost_cache_write: None,
        cost_reasoning: None,
        matched_aliases: Vec::new(),
        availability: Some(models::availability::classify_model(
            model_id,
            &provider,
            availability_ctx.installed,
            availability_ctx.opencode_probe_result,
            availability_ctx.pi_probe_result,
            availability_ctx.cursor_probe_result,
            availability_ctx.is_offline,
        )),
    }
}

fn model_entry_for_cached_static(model: &models::CachedModel) -> ListModelEntry {
    ListModelEntry {
        id: model.id.clone(),
        provider: model.provider.clone(),
        release_date: model.release_date.clone(),
        harness: None,
        harness_source: HarnessSource::Unavailable,
        harness_candidates: Vec::new(),
        description: model.description.clone(),
        cost_input: model.cost_input,
        cost_output: model.cost_output,
        cost_cache_read: model.cost_cache_read,
        cost_cache_write: model.cost_cache_write,
        cost_reasoning: model.cost_reasoning,
        matched_aliases: Vec::new(),
        availability: None,
    }
}

fn model_entry_for_pinned_static(
    model_id: &str,
    provider: Option<&str>,
    description: Option<&str>,
) -> ListModelEntry {
    let provider = provider
        .map(str::to_string)
        .or_else(|| models::infer_provider_from_model_id(model_id).map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string());
    ListModelEntry {
        id: model_id.to_string(),
        provider,
        release_date: None,
        harness: None,
        harness_source: HarnessSource::Unavailable,
        harness_candidates: Vec::new(),
        description: description.map(str::to_string),
        cost_input: None,
        cost_output: None,
        cost_cache_read: None,
        cost_cache_write: None,
        cost_reasoning: None,
        matched_aliases: Vec::new(),
        availability: None,
    }
}

fn sort_list_model_entries(entries: &mut [ListModelEntry]) {
    entries.sort_by(|a, b| {
        a.provider
            .to_ascii_lowercase()
            .cmp(&b.provider.to_ascii_lowercase())
            .then_with(|| {
                b.release_date
                    .as_deref()
                    .unwrap_or("")
                    .cmp(a.release_date.as_deref().unwrap_or(""))
            })
            .then_with(|| a.id.cmp(&b.id))
    });
}

fn routing_settings_evidence<'a>(
    input: &'a RouteTraceInput<'a>,
) -> crate::routing::RoutingSettingsEvidence<'a> {
    crate::routing::RoutingSettingsEvidence::new(
        input.model_id,
        Some(input.provider_for_order),
        input.provider_constraint,
        input.installed,
        input.opencode_probe_result,
        input.pi_probe_result,
        input.cursor_probe_result,
        input.catalog_model_slugs,
        input.routing_settings,
    )
}

fn resolve_harness_with_routing(
    provider: &str,
    model_id: &str,
    availability_ctx: AvailabilityContext<'_>,
) -> (Option<String>, HarnessSource) {
    resolve_harness_with_routing_auth(
        provider,
        model_id,
        availability_ctx,
        models::harness::native_harness_authenticated,
    )
}

fn resolve_harness_with_routing_auth<F>(
    provider: &str,
    model_id: &str,
    availability_ctx: AvailabilityContext<'_>,
    auth_check: F,
) -> (Option<String>, HarnessSource)
where
    F: Fn(&str) -> bool,
{
    let route_input = RouteTraceInput {
        model_id,
        provider_for_order: provider,
        provider_constraint: None,
        installed: availability_ctx.installed,
        opencode_probe_result: availability_ctx.opencode_probe_result,
        pi_probe_result: availability_ctx.pi_probe_result,
        cursor_probe_result: availability_ctx.cursor_probe_result,
        catalog_model_slugs: availability_ctx.catalog_model_slugs,
        routing_settings: availability_ctx.routing_settings,
    };
    let routing_evidence = routing_settings_evidence(&route_input);
    let trace = crate::routing::evaluate_candidates_with_auth(
        &routing_evidence.routing_input(),
        auth_check,
    );

    match crate::routing::acceptance::accept_route(
        &trace,
        availability_ctx.installed,
        crate::routing::acceptance::MatchPolicy::InstalledOnly,
    ) {
        Ok(()) => (
            Some(trace.selected_harness().to_string()),
            HarnessSource::AutoDetected,
        ),
        Err(_) => (None, HarnessSource::Unavailable),
    }
}

fn provider_constraint_for_alias(alias: &ModelAlias) -> Option<String> {
    match &alias.spec {
        ModelSpec::Pinned { provider, .. } | ModelSpec::PinnedWithMatch { provider, .. } => {
            provider.clone()
        }
        ModelSpec::AutoResolve { provider, .. } => provider.clone(),
    }
    .map(|provider| provider.trim().to_ascii_lowercase())
}

fn route_trace_for_resolved_model(input: &RouteTraceInput<'_>) -> crate::routing::RoutingTrace {
    let routing_evidence = routing_settings_evidence(input);
    crate::routing::evaluate_candidates(&routing_evidence.routing_input())
}

fn route_trace_for_resolved_model_with_probes(
    input: &RouteTraceInput<'_>,
    probe_resolver: &mut dyn crate::routing::ProbeResolver,
) -> crate::routing::RoutingTrace {
    let routing_evidence = routing_settings_evidence(input);
    crate::routing::evaluate_candidates_with_auth_and_probes(
        &routing_evidence.routing_input(),
        probe_resolver,
        models::harness::native_harness_authenticated,
    )
}

fn route_trace_for_fixed_harness_with_probes(
    input: &RouteTraceInput<'_>,
    fixed_harness: &str,
    source: crate::routing::RouteSource,
    probe_resolver: &mut dyn crate::routing::ProbeResolver,
) -> crate::routing::RoutingTrace {
    let provider_for_order = crate::routing::provider_for_order_for_fixed_harness(
        Some(input.provider_for_order),
        fixed_harness,
    );
    let routing_evidence = routing_settings_evidence(input);
    let mut fixed_input = routing_evidence.routing_input();
    fixed_input.provider_for_order = provider_for_order;
    let assessment = crate::routing::evaluate_fixed_harness_with_auth_and_probes(
        &fixed_input,
        fixed_harness,
        probe_resolver,
        models::harness::native_harness_authenticated,
    );
    crate::routing::trace_for_fixed_harness(source, fixed_harness, assessment, Vec::new())
}

fn effective_visibility(
    project_config: Option<&crate::config::LoadedProjectConfig>,
    args: &ListArgs,
) -> crate::config::ModelVisibility {
    if args.include.is_some() || args.exclude.is_some() {
        return crate::config::ModelVisibility {
            include: args.include.clone(),
            exclude: args.exclude.clone(),
        };
    }

    project_config
        .map(|loaded| loaded.effective.settings.model_visibility.clone())
        .unwrap_or_default()
}

#[allow(clippy::too_many_arguments)]
fn apply_routing_settings_to_resolved_aliases(
    resolved: &mut IndexMap<String, models::ResolvedAlias>,
    aliases: &IndexMap<String, ModelAlias>,
    installed: &HashSet<String>,
    opencode_probe_result: Option<&OpenCodeProbeResult>,
    pi_probe_result: Option<&PiProbeResult>,
    cursor_probe_result: Option<&CursorProbeResult>,
    catalog_model_slugs: Option<&[String]>,
    routing_settings: &ResolvedRoutingSettings,
) {
    for alias in resolved.values_mut() {
        let has_explicit_harness = aliases
            .get(&alias.name)
            .is_some_and(|source_alias| source_alias.harness.is_some());
        if has_explicit_harness {
            continue;
        }
        apply_routing_settings_to_resolved_alias(
            alias,
            installed,
            opencode_probe_result,
            pi_probe_result,
            cursor_probe_result,
            catalog_model_slugs,
            routing_settings,
        );
    }
}

fn apply_routing_settings_to_resolved_alias(
    alias: &mut models::ResolvedAlias,
    installed: &HashSet<String>,
    opencode_probe_result: Option<&OpenCodeProbeResult>,
    pi_probe_result: Option<&PiProbeResult>,
    cursor_probe_result: Option<&CursorProbeResult>,
    catalog_model_slugs: Option<&[String]>,
    routing_settings: &ResolvedRoutingSettings,
) {
    let provider_for_order =
        models::infer_provider_from_model_id(&alias.model_id).unwrap_or(alias.provider.as_str());
    let route_input = RouteTraceInput {
        model_id: &alias.model_id,
        provider_for_order,
        provider_constraint: None,
        installed,
        opencode_probe_result,
        pi_probe_result,
        cursor_probe_result,
        catalog_model_slugs,
        routing_settings,
    };
    let trace = route_trace_for_resolved_model(&route_input);
    alias.harness = Some(trace.harness.clone());
    alias.harness_source = match crate::routing::acceptance::accept_route(
        &trace,
        installed,
        crate::routing::acceptance::MatchPolicy::InstalledOnly,
    ) {
        Ok(()) => HarnessSource::AutoDetected,
        Err(_) => HarnessSource::Unavailable,
    };
}

fn annotate_resolved_availability(
    resolved: &mut IndexMap<String, models::ResolvedAlias>,
    installed: &HashSet<String>,
    opencode_probe_result: Option<&OpenCodeProbeResult>,
    pi_probe_result: Option<&PiProbeResult>,
    cursor_probe_result: Option<&CursorProbeResult>,
    is_offline: bool,
) {
    for alias in resolved.values_mut() {
        alias.availability = Some(models::availability::classify_model(
            &alias.model_id,
            &alias.provider,
            installed,
            opencode_probe_result,
            pi_probe_result,
            cursor_probe_result,
            is_offline,
        ));
    }
}

fn prune_unavailable(resolved: &mut IndexMap<String, models::ResolvedAlias>) {
    resolved.retain(|_, alias| {
        alias
            .availability
            .as_ref()
            .map(|availability| availability.status != AvailabilityStatus::Unavailable)
            .unwrap_or(true)
    });
}

fn filter_model_entries_by_visibility(
    entries: Vec<ListModelEntry>,
    visibility: &crate::config::ModelVisibility,
) -> Vec<ListModelEntry> {
    if visibility.include.is_none() && visibility.exclude.is_none() {
        return entries;
    }

    entries
        .into_iter()
        .filter(|entry| {
            let paths = entry
                .availability
                .as_ref()
                .map(|availability| availability.runnable_paths.as_slice())
                .unwrap_or(&[]);
            let included = visibility.include.as_ref().is_none_or(|includes| {
                includes.iter().any(|pattern| {
                    models::matches_visibility_pattern(pattern, &entry.id, &entry.provider, paths)
                })
            });
            let excluded = visibility.exclude.as_ref().is_some_and(|excludes| {
                excludes.iter().any(|pattern| {
                    models::matches_visibility_pattern(pattern, &entry.id, &entry.provider, paths)
                })
            });
            included && !excluded
        })
        .collect()
}

fn add_availability_json_fields(
    obj: &mut serde_json::Value,
    availability: Option<&ModelAvailability>,
) {
    if let Some(availability) = availability {
        obj["availability"] = serde_json::json!(availability.status);
        obj["availability_source"] = serde_json::json!(availability.source);
        obj["runnable_paths"] = serde_json::json!(availability.runnable_paths);
    }
}

fn add_cost_json_fields(obj: &mut serde_json::Value, model: &models::CachedModel) {
    obj["cost_input"] = serde_json::json!(model.cost_input);
    obj["cost_output"] = serde_json::json!(model.cost_output);
    obj["cost_cache_read"] = serde_json::json!(model.cost_cache_read);
    obj["cost_cache_write"] = serde_json::json!(model.cost_cache_write);
    obj["cost_reasoning"] = serde_json::json!(model.cost_reasoning);
}

fn add_probe_results_json(
    out: &mut serde_json::Value,
    probe_result: Option<&OpenCodeProbeResult>,
    pi_probe_result: Option<&PiProbeResult>,
    cursor_probe_result: Option<&CursorProbeResult>,
) {
    if let Some(probe) = probe_result {
        out["probe_results"] = serde_json::json!({
            "opencode": {
                "success": probe.model_probe_success,
                "models_found": probe.model_slugs.len(),
            }
        });
    }
    if let Some(probe) = pi_probe_result {
        if out.get("probe_results").is_none() {
            out["probe_results"] = serde_json::json!({});
        }
        out["probe_results"]["pi"] = serde_json::json!({
            "compatible": probe.compatible,
            "version": probe.version,
            "missing_surface_tokens": probe.help_surface_tokens_missing,
        });
    }
    if let Some(probe) = cursor_probe_result {
        if out.get("probe_results").is_none() {
            out["probe_results"] = serde_json::json!({});
        }
        out["probe_results"]["cursor"] = serde_json::json!({
            "success": probe.model_probe_success,
            "models_found": probe.slugs.len(),
        });
    }
}

fn availability_status_label(availability: Option<&ModelAvailability>) -> &'static str {
    match availability.map(|value| value.status) {
        Some(AvailabilityStatus::Runnable) => "runnable",
        Some(AvailabilityStatus::Unavailable) => "unavailable",
        Some(AvailabilityStatus::Unknown) => "unknown",
        None => "unknown",
    }
}

fn annotate_one_availability(
    resolved: &mut models::ResolvedAlias,
    args: &ResolveAliasArgs,
    installed: &HashSet<String>,
    opencode_probe_result: Option<&OpenCodeProbeResult>,
    pi_probe_result: Option<&PiProbeResult>,
    cursor_probe_result: Option<&CursorProbeResult>,
) {
    let is_offline = models::is_mars_offline() || args.no_refresh_models;
    resolved.availability = Some(models::availability::classify_model(
        &resolved.model_id,
        &resolved.provider,
        installed,
        opencode_probe_result,
        pi_probe_result,
        cursor_probe_result,
        is_offline,
    ));
}

fn print_availability_text(availability: Option<&ModelAvailability>) {
    if let Some(availability) = availability {
        println!(
            "Availability: {} ({:?})",
            availability_status_label(Some(availability)),
            availability.source
        );
        for (idx, path) in availability.runnable_paths.iter().enumerate() {
            let label = if idx == 0 {
                "Runnable via:"
            } else {
                "             "
            };
            println!("{label} {} -> {}", path.harness, path.harness_model_id);
        }
    }
}

fn add_route_json_fields(out: &mut serde_json::Value, trace: &crate::routing::RoutingTrace) {
    let report = trace.to_report();
    out["route"] = serde_json::json!(report.compact_summary());
    out["route_trace"] = serde_json::json!(report);
}

fn print_route_text(trace: &crate::routing::RoutingTrace) {
    let report = trace.to_report();
    println!(
        "Route:    {} ({}, {}, {})",
        trace.selected_harness(),
        trace.source.label(),
        trace.selected_selection_kind().label(),
        trace.selected_match_evidence().label()
    );
    if !report.candidates_tried.is_empty() {
        println!("Tried:    {}", report.candidates_tried.join(", "));
    }
    for assessment in report.assessments {
        if let Some(skip_reason) = assessment.skip_reason {
            println!("Skip:     {} ({})", assessment.harness, skip_reason);
        }
    }
}

fn run_resolve(args: &ResolveAliasArgs, ctx: &MarsContext, json: bool) -> Result<i32, MarsError> {
    let project_config = load_project_config_layers_optional(&ctx.project_root)?;
    let merged = load_merged_aliases(&ctx.project_root, project_config.as_ref())?;
    let mars = mars_dir(ctx);
    let ttl = models_cache_ttl_hours(project_config.as_ref());
    let refresh =
        models::resolve_models_refresh_control(args.refresh_models, args.no_refresh_models)?;
    let mode = refresh.catalog_mode;
    let default_settings = crate::config::Settings::default();
    let settings = project_config
        .as_ref()
        .map(|loaded| &loaded.effective.settings)
        .unwrap_or(&default_settings);
    let routing_settings = ResolvedRoutingSettings::from_settings(settings);
    let routing_diagnostics = routing_settings.diagnostic_messages();
    if !json {
        emit_routing_settings_warnings(&routing_diagnostics);
    }

    // Cache is enrichment, not a gate. If unavailable, skip to passthrough.
    let mut cache_error = None;
    let cache_result = match ensure_fresh_or_json_error(&mars, ttl, mode, json)? {
        FreshOrJsonError::Fresh(cache, outcome) => Some((cache, outcome)),
        FreshOrJsonError::JsonError(error_message) => {
            cache_error = Some(error_message);
            None
        }
    };
    let mut capability_session = CapabilitySession::collect(&CapabilityCollectionOptions {
        offline: models::is_mars_offline(),
        probe_refresh: refresh.probe_refresh,
    });
    let installed = capability_session.installed_harnesses();

    // Step 1: exact alias lookup
    if let Some(alias) = merged.get(&args.name) {
        if cache_result.is_none() && matches!(alias.spec, ModelSpec::AutoResolve { .. }) {
            return run_auto_resolve_alias_cache_unavailable(
                AutoResolveAliasCacheUnavailableInput {
                    name: &args.name,
                    alias,
                    project_config: project_config.as_ref(),
                    cache_error: cache_error.as_deref(),
                    routing_diagnostics: &routing_diagnostics,
                    json,
                },
            );
        }

        let fallback_cache = models::ModelsCache {
            models: Vec::new(),
            fetched_at: None,
        };
        let fallback_outcome = models::RefreshOutcome::Offline;
        let fallback_catalog_slugs = models::catalog_model_slugs(&fallback_cache);
        let cache_catalog_slugs = cache_result
            .as_ref()
            .map(|(cache, _)| models::catalog_model_slugs(cache));
        let (cache, outcome) = cache_result
            .as_ref()
            .map(|(cache, outcome)| (cache, outcome))
            .unwrap_or((&fallback_cache, &fallback_outcome));
        let catalog_model_slugs = cache_catalog_slugs
            .as_deref()
            .unwrap_or(fallback_catalog_slugs.as_slice());

        let runtime = ResolveRuntime {
            cache,
            catalog_model_slugs,
            outcome,
            installed: &installed,
            routing_settings: &routing_settings,
            probe_refresh: refresh.probe_refresh,
        };
        return run_resolve_exact_alias(
            ResolveExactAliasInput {
                args,
                alias,
                merged: &merged,
                project_config: project_config.as_ref(),
                runtime,
                routing_diagnostics: &routing_diagnostics,
                json,
            },
            &mut capability_session,
        );
    }

    // Step 2: alias-prefix resolution
    if let Some((cache, outcome)) = &cache_result
        && let Some(mut resolved) = models::resolve_with_alias_prefix_with_probe(
            &args.name, &merged, cache, None, None, None,
        )
    {
        let catalog_slugs = models::catalog_model_slugs(cache);
        let route_input = RouteTraceInput {
            model_id: &resolved.model_id,
            provider_for_order: models::infer_provider_from_model_id(&resolved.model_id)
                .unwrap_or(resolved.provider.as_str()),
            provider_constraint: None,
            installed: &installed,
            opencode_probe_result: None,
            pi_probe_result: None,
            cursor_probe_result: None,
            catalog_model_slugs: Some(catalog_slugs.as_slice()),
            routing_settings: &routing_settings,
        };
        let route_trace = {
            let mut probe_resolver = SessionProbeResolver {
                session: &mut capability_session,
            };
            route_trace_for_resolved_model_with_probes(&route_input, &mut probe_resolver)
        };
        resolved.harness = Some(route_trace.selected_harness().to_string());
        resolved.harness_source = match crate::routing::acceptance::accept_route(
            &route_trace,
            &installed,
            crate::routing::acceptance::MatchPolicy::InstalledOnly,
        ) {
            Ok(()) => HarnessSource::AutoDetected,
            Err(_) => HarnessSource::Unavailable,
        };
        annotate_one_availability(
            &mut resolved,
            args,
            &installed,
            capability_session.loaded_opencode_probe_result(),
            capability_session.loaded_pi_probe_result(),
            capability_session.loaded_cursor_probe_result(),
        );
        let cache_outcome = capability_session
            .loaded_opencode_outcome()
            .cloned()
            .unwrap_or(CachedProbeOutcome::Unavailable);
        return run_output_resolved(OutputResolvedInput {
            name: &args.name,
            resolved: &resolved,
            source: "alias_prefix",
            route_trace: &route_trace,
            outcome,
            cache_outcome: &cache_outcome,
            probe_refresh: refresh.probe_refresh,
            routing_diagnostics: &routing_diagnostics,
            json,
        });
    }

    // Step 3: passthrough — no cache needed
    let outcome = cache_result
        .as_ref()
        .map(|(_, o)| o.clone())
        .unwrap_or(models::RefreshOutcome::Offline);
    let is_offline = models::is_mars_offline() || args.no_refresh_models;
    let passthrough_catalog_slugs = cache_result
        .as_ref()
        .map(|(cache, _)| models::catalog_model_slugs(cache));
    run_output_passthrough(OutputPassthroughInput {
        name: &args.name,
        outcome: &outcome,
        is_offline,
        installed: &installed,
        capability_session: &mut capability_session,
        catalog_model_slugs: passthrough_catalog_slugs.as_deref(),
        routing_settings: &routing_settings,
        cache_error: cache_error.as_deref(),
        routing_diagnostics: &routing_diagnostics,
        json,
    })
}

fn run_refresh_probe(args: &RefreshProbeArgs) -> Result<i32, MarsError> {
    match args.target.as_str() {
        "opencode" => opencode_cache::run_refresh_probe_command(),
        "pi" => pi_cache::run_refresh_probe_command(),
        "cursor" => cursor_cache::run_refresh_probe_command(),
        _ => Ok(1),
    }
}

fn run_alias(args: &AddAliasArgs, ctx: &MarsContext, json: bool) -> Result<i32, MarsError> {
    let normalized_harness =
        models::harness::normalize_harness_name(&args.harness).ok_or_else(|| {
            MarsError::Config(ConfigError::Invalid {
                message: format!(
                    "invalid harness '{}'; valid harnesses: {}",
                    args.harness,
                    models::harness::VALID_HARNESSES.join(", ")
                ),
            })
        })?;
    let mut config = crate::config::load(&ctx.project_root)?;
    config.models.insert(
        args.name.clone(),
        ModelAlias {
            harness: Some(normalized_harness.clone()),
            description: args.description.clone(),
            prompting: None,
            default_effort: None,
            autocompact: None,
            autocompact_pct: None,
            spec: ModelSpec::Pinned {
                model: args.model_id.clone(),
                provider: None,
            },
        },
    );
    crate::config::save(&ctx.project_root, &config)?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "status": "ok",
                "alias": args.name,
                "model": args.model_id,
                "harness": normalized_harness,
            }))
            .unwrap()
        );
    } else {
        println!(
            "Added alias `{}` → {} (harness: {})",
            args.name, args.model_id, normalized_harness
        );
    }

    Ok(0)
}

fn run_prompting(args: &PromptingArgs, ctx: &MarsContext, json: bool) -> Result<i32, MarsError> {
    let project_config = load_project_config_layers_optional(&ctx.project_root)?;
    let merged = load_merged_aliases(&ctx.project_root, project_config.as_ref())?;
    let refresh =
        models::resolve_models_refresh_control(args.refresh_models, args.no_refresh_models)?;

    let target = resolve_prompt_ref(
        &args.reference,
        ctx,
        &merged,
        project_config.as_ref(),
        refresh,
    )?;

    if json {
        let out = target.to_json(&args.reference);
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else if target.found {
        print_prompt_target(&target);
    } else {
        eprintln!(
            "Unknown agent or model ref `{}`. Run `mars agents` or `mars models list` to see available refs.",
            args.reference
        );
        eprintln!("Examples:");
        eprintln!("  mars models prompting @explorer");
        eprintln!("  mars models prompting gpt55");
        return Ok(1);
    }

    Ok(if target.found { 0 } else { 1 })
}

#[derive(Debug)]
struct PromptTarget {
    found: bool,
    ref_kind: Option<PromptRefKind>,
    agent_name: Option<String>,
    model_alias: Option<String>,
    model_name: Option<String>,
    prompting: Option<String>,
}

#[derive(Debug, Clone, Copy)]
enum PromptRefKind {
    Agent,
    Model,
}

impl PromptRefKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Model => "model",
        }
    }
}

impl PromptTarget {
    fn unknown() -> Self {
        Self {
            found: false,
            ref_kind: None,
            agent_name: None,
            model_alias: None,
            model_name: None,
            prompting: None,
        }
    }

    fn to_json(&self, input_ref: &str) -> serde_json::Value {
        serde_json::json!({
            "ref": input_ref,
            "ref_kind": self.ref_kind.map(PromptRefKind::as_str),
            "agent_name": self.agent_name,
            "model_alias": self.model_alias,
            "model_name": self.model_name,
            "found": self.found,
            "prompting": self.prompting,
        })
    }
}

fn resolve_prompt_ref(
    input_ref: &str,
    ctx: &MarsContext,
    aliases: &IndexMap<String, ModelAlias>,
    project_config: Option<&crate::config::LoadedProjectConfig>,
    refresh: models::ModelsRefreshControl,
) -> Result<PromptTarget, MarsError> {
    if let Some(agent) = resolve_prompt_agent(input_ref, ctx)? {
        return prompt_target_for_agent(agent, ctx, aliases, project_config, refresh);
    }

    if input_ref.starts_with('@') {
        return Ok(PromptTarget::unknown());
    }

    Ok(aliases
        .get(input_ref)
        .map(|alias| prompt_target_for_model(input_ref, alias, ctx, project_config, refresh))
        .unwrap_or_else(PromptTarget::unknown))
}

struct PromptAgent {
    name: String,
    file_stem: String,
    profile: AgentProfile,
}

#[derive(Debug, Clone, Copy)]
enum PromptAgentMatch {
    FileStem,
    ProfileName,
}

fn resolve_prompt_agent(
    input_ref: &str,
    ctx: &MarsContext,
) -> Result<Option<PromptAgent>, MarsError> {
    let lookup_name = agent_ref_lookup_name(input_ref);
    let mut agents = load_prompt_agents(ctx)?;

    for match_kind in [PromptAgentMatch::FileStem, PromptAgentMatch::ProfileName] {
        if let Some(index) = agents
            .iter()
            .position(|agent| prompt_agent_matches(agent, lookup_name, match_kind))
        {
            return Ok(Some(agents.remove(index)));
        }
    }

    Ok(None)
}

fn load_prompt_agents(ctx: &MarsContext) -> Result<Vec<PromptAgent>, MarsError> {
    let lock = crate::lock::load(&ctx.project_root)?;
    let mars_dir = ctx.project_root.join(".mars");
    let mut agents = Vec::new();

    for (dest_path, item) in lock.canonical_flat_items() {
        if item.kind != ItemKind::Agent {
            continue;
        }

        let disk_path = dest_path.resolve(&mars_dir);
        let content = match std::fs::read_to_string(&disk_path) {
            Ok(content) => content,
            Err(err) => {
                eprintln!("warning: skipping {}: {err}", disk_path.display());
                continue;
            }
        };

        let mut diags = Vec::new();
        let (profile, _fm) = match parse_agent_content(&content, &mut diags) {
            Ok(parsed) => parsed,
            Err(err) => {
                eprintln!("warning: skipping {}: {err}", disk_path.display());
                continue;
            }
        };
        if let Some(fatal) = diags.iter().find(|diag| diag.is_error()) {
            eprintln!(
                "warning: skipping {}: {}",
                disk_path.display(),
                fatal.message()
            );
            continue;
        }

        let stem = prompt_path_stem(&disk_path);
        let agent_name = profile.name.as_deref().unwrap_or(stem.as_str());
        agents.push(PromptAgent {
            name: agent_name.to_string(),
            file_stem: stem,
            profile,
        });
    }

    Ok(agents)
}

fn prompt_agent_matches(
    agent: &PromptAgent,
    lookup_name: &str,
    match_kind: PromptAgentMatch,
) -> bool {
    match match_kind {
        PromptAgentMatch::FileStem => agent.file_stem.eq_ignore_ascii_case(lookup_name),
        PromptAgentMatch::ProfileName => agent.name.eq_ignore_ascii_case(lookup_name),
    }
}

fn agent_ref_lookup_name(input_ref: &str) -> &str {
    input_ref.strip_prefix('@').unwrap_or(input_ref)
}

fn prompt_path_stem(path: &std::path::Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string()
}

fn prompt_target_for_agent(
    agent: PromptAgent,
    ctx: &MarsContext,
    aliases: &IndexMap<String, ModelAlias>,
    project_config: Option<&crate::config::LoadedProjectConfig>,
    refresh: models::ModelsRefreshControl,
) -> Result<PromptTarget, MarsError> {
    let effective_config = project_config
        .map(|loaded| loaded.effective.clone())
        .unwrap_or_default();
    let policy = resolve_policy(
        &effective_config,
        PolicyInput {
            project_root: &ctx.project_root,
            runtime_aliases: aliases,
            agent: Some(&agent.file_stem),
            profile: &agent.profile,
            model_override: None,
            harness_override: None,
            effort_override: None,
            approval_override: None,
            sandbox_override: None,
            models_refresh: refresh,
        },
    )?;

    Ok(prompt_target_for_routing(
        Some(agent.name),
        PromptRefKind::Agent,
        &policy.routing,
        aliases,
    ))
}

fn prompt_target_for_routing(
    agent_name: Option<String>,
    ref_kind: PromptRefKind,
    routing: &crate::build::bundle::Routing,
    aliases: &IndexMap<String, ModelAlias>,
) -> PromptTarget {
    let token = routing.model_token.trim();
    let model_alias = (!token.is_empty() && aliases.contains_key(token)).then(|| token.to_string());
    let prompting = model_alias
        .as_deref()
        .and_then(|alias| aliases.get(alias))
        .and_then(|alias| alias.prompting.clone());
    let model_name = runnable_model_name(routing);

    PromptTarget {
        found: true,
        ref_kind: Some(ref_kind),
        agent_name,
        model_alias,
        model_name,
        prompting,
    }
}

fn runnable_model_name(routing: &crate::build::bundle::Routing) -> Option<String> {
    let harness_model = routing.harness_model.trim();
    if !harness_model.is_empty() {
        return Some(harness_model.to_string());
    }

    let model = routing.model.trim();
    (!model.is_empty()).then(|| model.to_string())
}

fn prompt_target_for_model(
    alias_name: &str,
    alias: &ModelAlias,
    ctx: &MarsContext,
    project_config: Option<&crate::config::LoadedProjectConfig>,
    refresh: models::ModelsRefreshControl,
) -> PromptTarget {
    let cache = prompt_model_cache(ctx, project_config, refresh);
    PromptTarget {
        found: true,
        ref_kind: Some(PromptRefKind::Model),
        agent_name: None,
        model_alias: Some(alias_name.to_string()),
        model_name: Some(model_name_for_alias(alias_name, alias, &cache)),
        prompting: alias.prompting.clone(),
    }
}

fn prompt_model_cache(
    ctx: &MarsContext,
    project_config: Option<&crate::config::LoadedProjectConfig>,
    refresh: models::ModelsRefreshControl,
) -> models::ModelsCache {
    let mars_dir = ctx.project_root.join(".mars");
    let ttl = models_cache_ttl_hours(project_config);
    models::ensure_fresh(&mars_dir, ttl, refresh.catalog_mode)
        .map(|(cache, _)| cache)
        .or_else(|_| models::read_cache(&mars_dir))
        .unwrap_or(models::ModelsCache {
            models: Vec::new(),
            fetched_at: None,
        })
}

fn model_name_for_alias(
    alias_name: &str,
    alias: &ModelAlias,
    cache: &models::ModelsCache,
) -> String {
    models::resolve_model_id_for_alias(alias, cache)
        .unwrap_or_else(|| alias.pinned_model_id().unwrap_or(alias_name).to_string())
}

fn print_prompt_target(target: &PromptTarget) {
    if let Some(text) = target.prompting.as_deref() {
        println!("{text}");
        return;
    }

    match target.ref_kind {
        Some(PromptRefKind::Agent) => {
            let agent_name = target.agent_name.as_deref().unwrap_or("unknown");
            match target.model_alias.as_deref() {
                Some(model_alias) => {
                    println!(
                        "No prompting guidance defined for agent `{agent_name}` (model alias `{model_alias}`)."
                    );
                    print_prompting_field_hint(model_alias);
                }
                None => {
                    let model = target.model_name.as_deref().unwrap_or("no model");
                    println!(
                        "No prompting guidance defined for agent `{agent_name}` (model `{model}`)."
                    );
                    println!("Prompting guidance is read from a known model alias.");
                }
            }
        }
        Some(PromptRefKind::Model) => {
            let model_alias = target.model_alias.as_deref().unwrap_or("unknown");
            println!("No prompting guidance defined for model alias `{model_alias}`.");
            print_prompting_field_hint(model_alias);
        }
        None => {}
    }

    println!();
    println!("Examples:");
    println!("  mars models prompting @explorer");
    println!("  mars models prompting gpt55");
}

fn print_prompting_field_hint(model_alias: &str) {
    println!("Add a `prompting` field to the alias in mars.toml:");
    println!();
    println!("  [models.{model_alias}]");
    println!("  prompting = \"Prompting tips for this model.\"");
}

enum FreshOrJsonError {
    Fresh(models::ModelsCache, models::RefreshOutcome),
    JsonError(String),
}

fn ensure_fresh_or_json_error(
    mars: &std::path::Path,
    ttl: u32,
    mode: models::RefreshMode,
    json: bool,
) -> Result<FreshOrJsonError, MarsError> {
    match models::ensure_fresh(mars, ttl, mode) {
        Ok((cache, outcome)) => Ok(FreshOrJsonError::Fresh(cache, outcome)),
        Err(err @ MarsError::ModelCacheUnavailable { .. }) if json => {
            Ok(FreshOrJsonError::JsonError(format!("{err}")))
        }
        Err(err) => Err(err),
    }
}

struct ResolveExactAliasInput<'a> {
    args: &'a ResolveAliasArgs,
    alias: &'a ModelAlias,
    merged: &'a IndexMap<String, ModelAlias>,
    project_config: Option<&'a crate::config::LoadedProjectConfig>,
    runtime: ResolveRuntime<'a>,
    routing_diagnostics: &'a [String],
    json: bool,
}

fn run_resolve_exact_alias(
    input: ResolveExactAliasInput<'_>,
    capability_session: &mut CapabilitySession,
) -> Result<i32, MarsError> {
    let ResolveExactAliasInput {
        args,
        alias,
        merged,
        project_config,
        runtime,
        routing_diagnostics,
        json,
    } = input;
    let cache_warning = cache_warning(runtime.outcome);
    if let Some(warning) = cache_warning.as_deref()
        && !json
    {
        eprintln!("warning: {warning}");
    }

    let name = &args.name;
    let source = determine_source(name, project_config);
    let mut diag = DiagnosticCollector::new();
    let mut resolved_entry =
        models::resolve_one_with_probe(name, merged, runtime.cache, &mut diag, None, None, None);
    let mut route_trace = None;
    let mut fixed_harness_route_rejection = None;
    if let Some(r) = resolved_entry.as_mut() {
        let provider_constraint = provider_constraint_for_alias(alias);
        let route_input = RouteTraceInput {
            model_id: &r.model_id,
            provider_for_order: &r.provider,
            provider_constraint: provider_constraint.as_deref(),
            installed: runtime.installed,
            opencode_probe_result: None,
            pi_probe_result: None,
            cursor_probe_result: None,
            catalog_model_slugs: Some(runtime.catalog_model_slugs),
            routing_settings: runtime.routing_settings,
        };
        route_trace = Some(if let Some(fixed_harness) = alias.harness.as_deref() {
            let mut probe_resolver = SessionProbeResolver {
                session: capability_session,
            };
            let fixed_trace = route_trace_for_fixed_harness_with_probes(
                &route_input,
                fixed_harness,
                crate::routing::RouteSource::Alias,
                &mut probe_resolver,
            );
            let assessed = fixed_trace
                .assessments
                .iter()
                .find(|assessment| assessment.harness == fixed_harness)
                .or_else(|| fixed_trace.assessments.first());
            fixed_harness_route_rejection = match assessed {
                Some(assessment) => crate::routing::acceptance::accept_assessment(assessment).err(),
                None => Some(
                    crate::routing::acceptance::RejectionReason::AssessmentFailed {
                        harness: fixed_harness.to_string(),
                        skip_reason: Some("missing_assessment".to_string()),
                    },
                ),
            };
            fixed_trace
        } else {
            let mut probe_resolver = SessionProbeResolver {
                session: capability_session,
            };
            route_trace_for_resolved_model_with_probes(&route_input, &mut probe_resolver)
        });
        if let Some(trace) = route_trace.as_ref() {
            r.harness = Some(trace.selected_harness().to_string());
            r.harness_source = match crate::routing::acceptance::accept_route(
                trace,
                runtime.installed,
                crate::routing::acceptance::MatchPolicy::InstalledOnly,
            ) {
                Ok(()) => HarnessSource::AutoDetected,
                Err(_) => HarnessSource::Unavailable,
            };
            if alias.harness.is_some() {
                r.harness_source = HarnessSource::Explicit;
            }
        }
        annotate_one_availability(
            r,
            args,
            runtime.installed,
            capability_session.loaded_opencode_probe_result(),
            capability_session.loaded_pi_probe_result(),
            capability_session.loaded_cursor_probe_result(),
        );
    }
    let diagnostics = diag.drain();
    let probe_outcome = capability_session
        .loaded_opencode_outcome()
        .cloned()
        .unwrap_or(CachedProbeOutcome::Unavailable);

    if let Some(rejection_reason) = fixed_harness_route_rejection {
        let trace = route_trace
            .as_ref()
            .expect("fixed harness route trace exists");
        let Some(resolved) = resolved_entry.as_ref() else {
            return Ok(1);
        };
        return run_resolve_fixed_harness_failure(ResolveFixedHarnessFailureInput {
            name,
            source: source.as_str(),
            resolved,
            trace,
            cache_warning: cache_warning.as_deref(),
            diagnostics: &diagnostics,
            rejection_reason: &rejection_reason,
            routing_diagnostics,
            json,
        });
    }

    if json {
        if let Some(r) = resolved_entry.as_ref() {
            let mut out = serde_json::json!({
                "name": r.name,
                "source": source,
                "provider": r.provider,
                "harness": r.harness,
                "harness_source": r.harness_source,
                "harness_candidates": r.harness_candidates,
                "model_id": r.model_id,
                "resolved_model": r.model_id,
                "spec": format_spec(&alias.spec),
                "description": r.description,
            });
            out["probe_cache"] = serde_json::json!(probe_outcome.cache_status());
            if let Some(error) = unavailable_harness_error(r) {
                out["error"] = serde_json::json!(error);
            }
            if let Some(default_effort) = &r.default_effort {
                out["default_effort"] = serde_json::json!(default_effort);
            }
            if let Some(autocompact) = r.autocompact {
                out["autocompact"] = serde_json::json!(autocompact);
            }
            if let Some(autocompact_pct) = r.autocompact_pct {
                out["autocompact_pct"] = serde_json::json!(autocompact_pct);
            }
            add_availability_json_fields(&mut out, r.availability.as_ref());
            if let Some(warning) = cache_warning.as_deref() {
                out["cache_warning"] = serde_json::json!(warning);
            }
            if !diagnostics.is_empty() {
                out["diagnostics"] = serde_json::json!(diagnostics_to_json_entries(&diagnostics));
            }
            add_routing_diagnostics_json(&mut out, routing_diagnostics);
            if let Some(trace) = route_trace.as_ref() {
                add_route_json_fields(&mut out, trace);
            }
            println!("{}", serde_json::to_string_pretty(&out).unwrap());
        } else {
            let mut out = serde_json::json!({
                "error": format!("alias `{}` did not resolve to a model ID", name),
            });
            if let Some(warning) = cache_warning.as_deref() {
                out["cache_warning"] = serde_json::json!(warning);
            }
            if !diagnostics.is_empty() {
                out["diagnostics"] = serde_json::json!(diagnostics_to_json_entries(&diagnostics));
            }
            add_routing_diagnostics_json(&mut out, routing_diagnostics);
            println!("{}", serde_json::to_string_pretty(&out).unwrap());
            return Ok(1);
        }
    } else {
        if runtime.probe_refresh == ProbeRefreshMode::Background
            && matches!(probe_outcome, CachedProbeOutcome::Stale(_))
        {
            eprintln!("note: using cached opencode probe (stale, background refresh triggered)");
        }
        let Some(r) = resolved_entry.as_ref() else {
            eprintln!("error: alias `{}` did not resolve to a model ID", name);
            return Ok(1);
        };
        let harness = r.harness.as_deref().unwrap_or("—");
        println!("Alias:    {}", name);
        println!("Source:   {}", source);
        println!(
            "Harness:  {} ({})",
            harness,
            harness_source_label(&r.harness_source)
        );
        println!("Provider: {}", r.provider);
        match &alias.spec {
            ModelSpec::Pinned { model, provider: _ } => {
                println!("Mode:     pinned");
                println!("Model:    {}", model);
            }
            ModelSpec::PinnedWithMatch {
                model,
                provider: _,
                match_patterns,
                exclude_patterns,
            } => {
                println!("Mode:     pinned");
                println!("Model:    {}", model);
                println!("Match:    {}", match_patterns.join(", "));
                if !exclude_patterns.is_empty() {
                    println!("Exclude:  {}", exclude_patterns.join(", "));
                }
                println!("Resolved: {}", r.model_id);
            }
            ModelSpec::AutoResolve {
                provider: _,
                match_patterns,
                exclude_patterns,
            } => {
                println!("Mode:     auto-resolve");
                println!("Match:    {}", match_patterns.join(", "));
                if !exclude_patterns.is_empty() {
                    println!("Exclude:  {}", exclude_patterns.join(", "));
                }
                println!("Resolved: {}", r.model_id);
            }
        }
        if let Some(error) = unavailable_harness_error(r) {
            println!("Error:    {}", error);
        }
        print_availability_text(r.availability.as_ref());
        if let Some(desc) = &r.description {
            println!("Desc:     {}", desc);
        }
        if let Some(trace) = route_trace.as_ref() {
            print_route_text(trace);
        }
        emit_drained_text_diagnostics(&diagnostics);
    }

    Ok(0)
}

struct ResolveFixedHarnessFailureInput<'a> {
    name: &'a str,
    source: &'a str,
    resolved: &'a models::ResolvedAlias,
    trace: &'a crate::routing::RoutingTrace,
    cache_warning: Option<&'a str>,
    diagnostics: &'a [Diagnostic],
    rejection_reason: &'a crate::routing::acceptance::RejectionReason,
    routing_diagnostics: &'a [String],
    json: bool,
}

struct AutoResolveAliasCacheUnavailableInput<'a> {
    name: &'a str,
    alias: &'a ModelAlias,
    project_config: Option<&'a crate::config::LoadedProjectConfig>,
    cache_error: Option<&'a str>,
    routing_diagnostics: &'a [String],
    json: bool,
}

fn run_auto_resolve_alias_cache_unavailable(
    input: AutoResolveAliasCacheUnavailableInput<'_>,
) -> Result<i32, MarsError> {
    let AutoResolveAliasCacheUnavailableInput {
        name,
        alias,
        project_config,
        cache_error,
        routing_diagnostics,
        json,
    } = input;
    let source = determine_source(name, project_config);
    let detail = cache_error.unwrap_or("models cache unavailable");
    let error = format!(
        "alias `{name}` requires models cache for auto-resolve, but cache is unavailable ({detail})"
    );

    if json {
        let mut out = serde_json::json!({
            "name": name,
            "source": source,
            "spec": format_spec(&alias.spec),
            "error": error,
        });
        if let Some(cache_error) = cache_error {
            out["cache_error"] = serde_json::json!(cache_error);
        }
        add_routing_diagnostics_json(&mut out, routing_diagnostics);
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else {
        eprintln!("error: {error}");
    }

    Ok(1)
}

fn run_resolve_fixed_harness_failure(
    input: ResolveFixedHarnessFailureInput<'_>,
) -> Result<i32, MarsError> {
    let ResolveFixedHarnessFailureInput {
        name,
        source,
        resolved,
        trace,
        cache_warning,
        diagnostics,
        rejection_reason,
        routing_diagnostics,
        json,
    } = input;
    let error_message = fixed_alias_rejection_message(rejection_reason);

    if json {
        let mut out = serde_json::json!({
            "name": name,
            "source": source,
            "provider": resolved.provider,
            "harness": trace.selected_harness(),
            "model_id": resolved.model_id,
            "resolved_model": resolved.model_id,
            "error": error_message,
            "route_rejection": route_rejection_json(rejection_reason),
            "harnesses_tried": trace.candidates_tried,
        });
        add_route_json_fields(&mut out, trace);
        if let Some(warning) = cache_warning {
            out["cache_warning"] = serde_json::json!(warning);
        }
        if !diagnostics.is_empty() {
            out["diagnostics"] = serde_json::json!(diagnostics_to_json_entries(diagnostics));
        }
        add_routing_diagnostics_json(&mut out, routing_diagnostics);
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else {
        eprintln!("error: {error_message}");
        println!("Alias:    {name}");
        println!("Source:   {source}");
        println!("Provider: {}", resolved.provider);
        println!("Resolved: {}", resolved.model_id);
        print_route_text(trace);
        emit_drained_text_diagnostics(diagnostics);
    }

    Ok(1)
}

fn run_output_resolved(input: OutputResolvedInput<'_>) -> Result<i32, MarsError> {
    let OutputResolvedInput {
        name,
        resolved,
        source,
        route_trace,
        outcome,
        cache_outcome,
        probe_refresh,
        routing_diagnostics,
        json,
    } = input;
    let cache_warning = cache_warning(outcome);
    if let Some(warning) = cache_warning.as_deref()
        && !json
    {
        eprintln!("warning: {warning}");
    }

    if json {
        let mut out = serde_json::json!({
            "name": name,
            "source": source,
            "provider": resolved.provider,
            "harness": resolved.harness,
            "harness_source": resolved.harness_source,
            "harness_candidates": resolved.harness_candidates,
            "model_id": resolved.model_id,
            "resolved_model": resolved.model_id,
            "description": resolved.description,
        });
        if let Some(error) = unavailable_harness_error(resolved) {
            out["error"] = serde_json::json!(error);
        }
        if let Some(default_effort) = &resolved.default_effort {
            out["default_effort"] = serde_json::json!(default_effort);
        }
        if let Some(autocompact) = resolved.autocompact {
            out["autocompact"] = serde_json::json!(autocompact);
        }
        if let Some(autocompact_pct) = resolved.autocompact_pct {
            out["autocompact_pct"] = serde_json::json!(autocompact_pct);
        }
        out["probe_cache"] = serde_json::json!(cache_outcome.cache_status());
        add_availability_json_fields(&mut out, resolved.availability.as_ref());
        if let Some(warning) = cache_warning.as_deref() {
            out["cache_warning"] = serde_json::json!(warning);
        }
        add_routing_diagnostics_json(&mut out, routing_diagnostics);
        add_route_json_fields(&mut out, route_trace);
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else {
        if probe_refresh == ProbeRefreshMode::Background
            && matches!(cache_outcome, CachedProbeOutcome::Stale(_))
        {
            eprintln!("note: using cached opencode probe (stale, background refresh triggered)");
        }
        let harness = resolved.harness.as_deref().unwrap_or("—");
        println!("Alias:    {}", name);
        println!("Source:   {}", source);
        println!(
            "Harness:  {} ({})",
            harness,
            harness_source_label(&resolved.harness_source)
        );
        println!("Provider: {}", resolved.provider);
        println!("Resolved: {}", resolved.model_id);
        if let Some(error) = unavailable_harness_error(resolved) {
            println!("Error:    {}", error);
        }
        print_availability_text(resolved.availability.as_ref());
        if let Some(desc) = &resolved.description {
            println!("Desc:     {}", desc);
        }
        print_route_text(route_trace);
    }

    Ok(0)
}

fn run_output_passthrough(input: OutputPassthroughInput<'_>) -> Result<i32, MarsError> {
    let OutputPassthroughInput {
        name,
        outcome,
        is_offline,
        installed,
        capability_session,
        catalog_model_slugs,
        routing_settings,
        cache_error,
        routing_diagnostics,
        json,
    } = input;
    if name.trim().is_empty() {
        if json {
            let mut out = serde_json::json!({
                "error": "model name cannot be empty",
            });
            if let Some(cache_error) = cache_error {
                out["cache_error"] = serde_json::json!(cache_error);
            }
            add_routing_diagnostics_json(&mut out, routing_diagnostics);
            println!("{}", serde_json::to_string_pretty(&out).unwrap());
        } else {
            eprintln!("error: model name cannot be empty");
        }
        return Ok(1);
    }

    let cache_warning = cache_warning(outcome);
    if let Some(warning) = cache_warning.as_deref()
        && !json
    {
        eprintln!("warning: {warning}");
    }

    let (passthrough_model_id, provider_constraint) =
        models::split_provider_constrained_model_token(name);
    let guessed_provider =
        models::infer_provider_from_model_id(&passthrough_model_id).map(str::to_string);
    let provider_for_order = provider_constraint.as_deref().unwrap_or("unknown");
    let provider_for_classification = guessed_provider
        .as_deref()
        .or(provider_constraint.as_deref())
        .unwrap_or("unknown");
    let routing_evidence = crate::routing::RoutingSettingsEvidence::new(
        &passthrough_model_id,
        Some(provider_for_order),
        provider_constraint.as_deref(),
        installed,
        None,
        None,
        None,
        catalog_model_slugs,
        routing_settings,
    );
    let trace = {
        let mut probe_resolver = SessionProbeResolver {
            session: capability_session,
        };
        crate::routing::evaluate_candidates_with_auth_and_probes(
            &routing_evidence.routing_input(),
            &mut probe_resolver,
            models::harness::native_harness_authenticated,
        )
    };
    if let Err(rejection_reason) = crate::routing::acceptance::accept_route(
        &trace,
        installed,
        crate::routing::acceptance::MatchPolicy::RequireSlugEvidence,
    ) {
        let message = passthrough_rejection_message(name, &rejection_reason);
        if json {
            let mut out = serde_json::json!({
                "error": message,
                "source": "passthrough",
                "model_id": passthrough_model_id,
                "resolved_model": passthrough_model_id,
                "provider_constraint": provider_constraint,
                "harnesses_tried": trace.candidates_tried,
                "route_rejection": route_rejection_json(&rejection_reason),
            });
            add_route_json_fields(&mut out, &trace);
            if !trace.selected_diagnostics().is_empty() {
                out["diagnostics"] = serde_json::json!(trace.selected_diagnostics());
            }
            if let Some(warning) = cache_warning.as_deref() {
                out["cache_warning"] = serde_json::json!(warning);
            }
            if let Some(cache_error) = cache_error {
                out["cache_error"] = serde_json::json!(cache_error);
            }
            add_routing_diagnostics_json(&mut out, routing_diagnostics);
            println!("{}", serde_json::to_string_pretty(&out).unwrap());
        } else {
            eprintln!("error: {message}");
            print_route_text(&trace);
        }
        return Ok(1);
    }

    let harness = installed
        .contains(trace.selected_harness())
        .then_some(trace.selected_harness().to_string());
    let harness_source = "pattern_guess";
    let harness_candidates = models::harness::harness_candidates_for_provider(provider_for_order);
    let availability = models::availability::classify_model(
        &passthrough_model_id,
        provider_for_classification,
        installed,
        capability_session.loaded_opencode_probe_result(),
        capability_session.loaded_pi_probe_result(),
        capability_session.loaded_cursor_probe_result(),
        is_offline,
    );

    let warning = passthrough_catalog_warning(name, &trace);

    if json {
        let mut out = serde_json::json!({
            "name": name,
            "source": "passthrough",
            "model_id": passthrough_model_id,
            "resolved_model": passthrough_model_id,
            "provider": guessed_provider,
            "harness": harness,
            "harness_source": harness_source,
            "harness_candidates": harness_candidates,
            "description": serde_json::Value::Null,
        });
        if let Some(warning) = warning.as_deref() {
            out["warning"] = serde_json::json!(warning);
        }
        add_availability_json_fields(&mut out, Some(&availability));
        add_route_json_fields(&mut out, &trace);
        if let Some(warning) = cache_warning.as_deref() {
            out["cache_warning"] = serde_json::json!(warning);
        }
        if let Some(cache_error) = cache_error {
            out["cache_error"] = serde_json::json!(cache_error);
        }
        add_routing_diagnostics_json(&mut out, routing_diagnostics);
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else {
        if let Some(warning) = warning.as_deref() {
            eprintln!("warning: {}", warning);
        }
        let h = harness.as_deref().unwrap_or("—");
        println!("Model:      {}", name);
        println!("Source:     passthrough");
        println!("Harness:    {} ({})", h, harness_source);
        if let Some(provider) = guessed_provider {
            println!("Provider:   {}", provider);
        }
        if !harness_candidates.is_empty() {
            println!("Candidates: {}", harness_candidates.join(", "));
        }
        print_route_text(&trace);
    }

    Ok(0)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn load_project_config_layers_optional(
    project_root: &std::path::Path,
) -> Result<Option<crate::config::LoadedProjectConfig>, MarsError> {
    match crate::config::load_project_config_layers(project_root) {
        Ok(loaded) => Ok(Some(loaded)),
        Err(MarsError::Config(crate::error::ConfigError::NotFound { .. })) => Ok(None),
        Err(err) => Err(err),
    }
}

fn models_cache_ttl_hours(project_config: Option<&crate::config::LoadedProjectConfig>) -> u32 {
    project_config
        .map(|loaded| loaded.effective.settings.models_cache_ttl_hours)
        .unwrap_or_else(|| crate::config::Settings::default().models_cache_ttl_hours)
}

/// Load model aliases by combining lock-persisted dependency aliases with effective
/// project/local consumer aliases.
fn load_merged_aliases(
    project_root: &std::path::Path,
    project_config: Option<&crate::config::LoadedProjectConfig>,
) -> Result<indexmap::IndexMap<String, ModelAlias>, MarsError> {
    let lock = crate::lock::load_for_runtime_aliases(project_root)?;
    Ok(models::merged_runtime_aliases(
        &lock.dependency_model_aliases,
        project_config.map(|loaded| &loaded.effective.models),
    ))
}

/// Determine which layer provides an alias (consumer or dependency).
fn determine_source(
    name: &str,
    project_config: Option<&crate::config::LoadedProjectConfig>,
) -> String {
    let Some(project_config) = project_config else {
        return "unknown".to_string();
    };

    if project_config.local.models.contains_key(name) {
        return "consumer local (mars.local.toml)".to_string();
    }

    if project_config.config.models.contains_key(name) {
        return "consumer (mars.toml)".to_string();
    }

    "dependency".to_string()
}

fn format_spec(spec: &ModelSpec) -> serde_json::Value {
    match spec {
        ModelSpec::Pinned { model, provider } => {
            let mut out = serde_json::json!({ "mode": "pinned", "model": model });
            if let Some(provider) = provider {
                out["provider"] = serde_json::json!(provider);
            }
            out
        }
        ModelSpec::PinnedWithMatch {
            model,
            provider,
            match_patterns,
            exclude_patterns,
        } => {
            let mut out = serde_json::json!({
                "mode": "pinned",
                "model": model,
                "match": match_patterns,
                "exclude": exclude_patterns,
            });
            if let Some(provider) = provider {
                out["provider"] = serde_json::json!(provider);
            }
            out
        }
        ModelSpec::AutoResolve {
            provider,
            match_patterns,
            exclude_patterns,
        } => {
            let mut obj = serde_json::json!({
                "mode": "auto-resolve",
                "match": match_patterns,
                "exclude": exclude_patterns,
            });
            if let Some(provider) = provider {
                obj["provider"] = serde_json::json!(provider);
            }
            obj
        }
    }
}

fn mode_for_alias(spec: Option<&ModelSpec>) -> &'static str {
    match spec {
        Some(ModelSpec::Pinned { .. }) | Some(ModelSpec::PinnedWithMatch { .. }) => "pinned",
        Some(ModelSpec::AutoResolve { .. }) => "auto-resolve",
        None => "unknown",
    }
}

fn harness_source_label(source: &HarnessSource) -> &'static str {
    match source {
        HarnessSource::Explicit => "explicit",
        HarnessSource::AutoDetected => "auto-detected",
        HarnessSource::Unavailable => "unavailable",
    }
}

fn unavailable_harness_error(resolved: &models::ResolvedAlias) -> Option<String> {
    if resolved.harness_source != HarnessSource::Unavailable {
        return None;
    }
    if let Some(h) = &resolved.harness {
        Some(format!("Harness '{}' is not installed", h))
    } else {
        Some(format!(
            "No installed harness for provider '{}'. Install one of: {}",
            resolved.provider,
            resolved.harness_candidates.join(", ")
        ))
    }
}

fn fixed_alias_rejection_message(
    rejection: &crate::routing::acceptance::RejectionReason,
) -> String {
    match rejection {
        crate::routing::acceptance::RejectionReason::HarnessNotInstalled { harness } => format!(
            "alias harness `{harness}` is not installed and cannot run resolved model under model-first routing"
        ),
        crate::routing::acceptance::RejectionReason::NoSlugEvidence { harness } => format!(
            "alias harness `{harness}` did not provide required model slug evidence under model-first routing"
        ),
        crate::routing::acceptance::RejectionReason::AssessmentFailed {
            harness,
            skip_reason,
        } => format!(
            "alias harness `{harness}` cannot run resolved model under model-first routing ({})",
            skip_reason.as_deref().unwrap_or("unavailable")
        ),
    }
}

fn passthrough_rejection_message(
    model_name: &str,
    rejection: &crate::routing::acceptance::RejectionReason,
) -> String {
    match rejection {
        crate::routing::acceptance::RejectionReason::HarnessNotInstalled { harness } => format!(
            "model '{model_name}' selected harness '{harness}', but that harness is not installed"
        ),
        crate::routing::acceptance::RejectionReason::NoSlugEvidence { .. } => format!(
            "model '{model_name}' did not match any harness-reported model slug under model-first routing"
        ),
        crate::routing::acceptance::RejectionReason::AssessmentFailed {
            harness,
            skip_reason,
        } => format!(
            "model '{model_name}' failed model-first routing assessment on harness '{harness}' ({})",
            skip_reason.as_deref().unwrap_or("unavailable")
        ),
    }
}

fn passthrough_catalog_warning(name: &str, trace: &crate::routing::RoutingTrace) -> Option<String> {
    match trace.selected_match_evidence() {
        crate::routing::MatchEvidence::Passthrough => Some(format!(
            "model '{}' not found in catalog, passing through to harness",
            name
        )),
        crate::routing::MatchEvidence::Confirmed | crate::routing::MatchEvidence::Constrained => {
            None
        }
        crate::routing::MatchEvidence::None => None,
    }
}

fn route_rejection_json(
    rejection: &crate::routing::acceptance::RejectionReason,
) -> serde_json::Value {
    match rejection {
        crate::routing::acceptance::RejectionReason::HarnessNotInstalled { harness } => {
            serde_json::json!({
                "reason": "harness_not_installed",
                "harness": harness,
            })
        }
        crate::routing::acceptance::RejectionReason::NoSlugEvidence { harness } => {
            serde_json::json!({
                "reason": "no_slug_evidence",
                "harness": harness,
            })
        }
        crate::routing::acceptance::RejectionReason::AssessmentFailed {
            harness,
            skip_reason,
        } => {
            serde_json::json!({
                "reason": "assessment_failed",
                "harness": harness,
                "skip_reason": skip_reason,
            })
        }
    }
}

fn stale_warning(reason: &str) -> String {
    format!("models cache refresh failed: {reason}; using stale cache")
}

fn cache_warning(outcome: &models::RefreshOutcome) -> Option<String> {
    match outcome {
        models::RefreshOutcome::StaleFallback { reason } => Some(stale_warning(reason)),
        _ => None,
    }
}

fn emit_routing_settings_warnings(routing_diagnostics: &[String]) {
    for message in routing_diagnostics {
        eprintln!("warning: {message}");
    }
}

fn add_routing_diagnostics_json(out: &mut serde_json::Value, routing_diagnostics: &[String]) {
    if !routing_diagnostics.is_empty() {
        out["routing_diagnostics"] = serde_json::json!(routing_diagnostics);
    }
}

fn diagnostics_to_json_entries(diagnostics: &[Diagnostic]) -> Vec<serde_json::Value> {
    diagnostics
        .iter()
        .map(|diagnostic| {
            serde_json::json!({
                "level": diagnostic_level_label(diagnostic.level),
                "code": diagnostic.code,
                "message": diagnostic.message,
                "context": diagnostic.context,
            })
        })
        .collect()
}

fn drain_diagnostics_json(diag: &mut DiagnosticCollector) -> Option<serde_json::Value> {
    let diagnostics = diag.drain();
    if diagnostics.is_empty() {
        None
    } else {
        Some(serde_json::json!(diagnostics_to_json_entries(&diagnostics)))
    }
}

fn emit_drained_text_diagnostics(diagnostics: &[Diagnostic]) {
    for diagnostic in diagnostics {
        let label = diagnostic_level_label(diagnostic.level);
        eprintln!("{label}: {}", diagnostic.message);
    }
}

fn emit_text_diagnostics(diag: &mut DiagnosticCollector) {
    let diagnostics = diag.drain();
    emit_drained_text_diagnostics(&diagnostics);
}

fn diagnostic_level_label(level: DiagnosticLevel) -> &'static str {
    match level {
        DiagnosticLevel::Error => "error",
        DiagnosticLevel::Warning => "warning",
        DiagnosticLevel::Info => "info",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use indexmap::IndexMap;
    use tempfile::TempDir;

    fn write_mars_toml(temp: &TempDir, contents: &str) {
        std::fs::write(temp.path().join("mars.toml"), contents).unwrap();
    }

    fn normalized_exit_code(result: Result<i32, MarsError>) -> i32 {
        match result {
            Ok(code) => code,
            Err(err) => err.exit_code(),
        }
    }

    #[test]
    fn list_args_parses_no_refresh_models() {
        let args = ListArgs::try_parse_from(["mars", "--no-refresh-models"]).unwrap();
        assert!(args.no_refresh_models);
    }

    #[test]
    fn list_args_parses_refresh_models() {
        let args = ListArgs::try_parse_from(["mars", "--refresh-models"]).unwrap();
        assert!(args.refresh_models);
    }

    #[test]
    fn list_refresh_and_no_refresh_conflict() {
        assert!(
            ListArgs::try_parse_from(["mars", "--refresh-models", "--no-refresh-models"]).is_err()
        );
    }

    #[test]
    fn list_args_parses_catalog() {
        let args = ListArgs::try_parse_from(["mars", "--catalog"]).unwrap();
        assert!(args.catalog);
    }

    #[test]
    fn list_all_and_catalog_conflict() {
        let parsed = ModelsArgs::try_parse_from(["mars", "list", "--all", "--catalog"]);
        assert!(parsed.is_err());
    }

    #[test]
    fn list_all_and_include_can_combine() {
        let parsed = ModelsArgs::try_parse_from(["mars", "list", "--all", "--include", "opus"]);
        assert!(parsed.is_ok());
    }

    #[test]
    fn list_catalog_and_include_can_combine() {
        let parsed = ModelsArgs::try_parse_from(["mars", "list", "--catalog", "--include", "opus"]);
        assert!(parsed.is_ok());
    }

    #[test]
    fn resolve_alias_args_parses_no_refresh_models() {
        let args =
            ResolveAliasArgs::try_parse_from(["mars", "opus", "--no-refresh-models"]).unwrap();
        assert!(args.no_refresh_models);
    }

    #[test]
    fn list_no_refresh_without_cache_is_non_zero() {
        let temp = TempDir::new().unwrap();
        write_mars_toml(&temp, "[settings]\n");
        let ctx = MarsContext::new(temp.path().to_path_buf()).unwrap();
        let args = ModelsArgs::try_parse_from(["mars", "list", "--no-refresh-models"]).unwrap();

        let exit = normalized_exit_code(run(&args, &ctx, false));
        assert_ne!(exit, 0);
    }

    #[test]
    fn resolve_no_refresh_without_cache_is_non_zero() {
        let temp = TempDir::new().unwrap();
        write_mars_toml(
            &temp,
            r#"[settings]

[models.opus]
harness = "claude"
model = "claude-opus-4-6"
"#,
        );
        let ctx = MarsContext::new(temp.path().to_path_buf()).unwrap();
        let args =
            ModelsArgs::try_parse_from(["mars", "resolve", "opus", "--no-refresh-models"]).unwrap();

        let exit = normalized_exit_code(run(&args, &ctx, false));
        assert_ne!(exit, 0);
    }

    #[test]
    fn alias_updates_existing_model_entry() {
        let temp = TempDir::new().unwrap();
        write_mars_toml(
            &temp,
            r#"[settings]

[models.fast]
harness = "claude"
model = "claude-3-5-sonnet"
description = "Old alias"
"#,
        );
        let ctx = MarsContext::new(temp.path().to_path_buf()).unwrap();

        let args = AddAliasArgs {
            name: "fast".to_string(),
            model_id: "gpt-5.3-codex".to_string(),
            harness: "codex".to_string(),
            description: Some("Updated alias".to_string()),
        };

        let exit = run_alias(&args, &ctx, false).unwrap();
        assert_eq!(exit, 0);

        let config = crate::config::load(temp.path()).unwrap();
        assert_eq!(config.models.len(), 1);

        let alias = config.models.get("fast").unwrap();
        assert_eq!(alias.harness.as_deref(), Some("codex"));
        assert_eq!(alias.description.as_deref(), Some("Updated alias"));
        match &alias.spec {
            ModelSpec::Pinned { model, provider } => {
                assert_eq!(model, "gpt-5.3-codex");
                assert_eq!(provider, &None);
            }
            _ => panic!("expected pinned alias"),
        }
    }

    #[test]
    fn alias_rejects_invalid_harness_at_write_boundary() {
        let temp = TempDir::new().unwrap();
        write_mars_toml(&temp, "[settings]\n");
        let ctx = MarsContext::new(temp.path().to_path_buf()).unwrap();

        let args = AddAliasArgs {
            name: "fast".to_string(),
            model_id: "gpt-5.3-codex".to_string(),
            harness: "gemini".to_string(),
            description: None,
        };

        let err = run_alias(&args, &ctx, false).unwrap_err().to_string();
        assert!(err.contains("invalid harness 'gemini'"));
        assert!(err.contains("valid harnesses: claude, codex, pi, cursor, opencode"));
    }

    #[test]
    fn alias_normalizes_mixed_case_harness_before_write() {
        let temp = TempDir::new().unwrap();
        write_mars_toml(&temp, "[settings]\n");
        let ctx = MarsContext::new(temp.path().to_path_buf()).unwrap();

        let args = AddAliasArgs {
            name: "fast".to_string(),
            model_id: "gpt-5.3-codex".to_string(),
            harness: "OpenCode".to_string(),
            description: None,
        };

        let exit = run_alias(&args, &ctx, false).unwrap();
        assert_eq!(exit, 0);

        let config = crate::config::load(temp.path()).unwrap();
        let alias = config.models.get("fast").unwrap();
        assert_eq!(alias.harness.as_deref(), Some("opencode"));
    }

    fn auto_alias(
        provider: &str,
        match_patterns: &[&str],
        exclude_patterns: &[&str],
    ) -> ModelAlias {
        ModelAlias {
            harness: None,
            description: None,
            prompting: None,
            default_effort: None,
            autocompact: None,
            autocompact_pct: None,
            spec: ModelSpec::AutoResolve {
                provider: Some(provider.to_string()),
                match_patterns: match_patterns.iter().map(|v| (*v).to_string()).collect(),
                exclude_patterns: exclude_patterns.iter().map(|v| (*v).to_string()).collect(),
            },
        }
    }

    fn pinned_with_match_alias(
        model: &str,
        provider: &str,
        match_patterns: &[&str],
        exclude_patterns: &[&str],
    ) -> ModelAlias {
        ModelAlias {
            harness: None,
            description: None,
            prompting: None,
            default_effort: None,
            autocompact: None,
            autocompact_pct: None,
            spec: ModelSpec::PinnedWithMatch {
                model: model.to_string(),
                provider: Some(provider.to_string()),
                match_patterns: match_patterns.iter().map(|v| (*v).to_string()).collect(),
                exclude_patterns: exclude_patterns.iter().map(|v| (*v).to_string()).collect(),
            },
        }
    }

    fn pinned_alias(model: &str) -> ModelAlias {
        ModelAlias {
            harness: None,
            description: None,
            prompting: None,
            default_effort: None,
            autocompact: None,
            autocompact_pct: None,
            spec: ModelSpec::Pinned {
                model: model.to_string(),
                provider: None,
            },
        }
    }

    fn pinned_alias_with_provider(model: &str, provider: &str) -> ModelAlias {
        ModelAlias {
            harness: None,
            description: None,
            prompting: None,
            default_effort: None,
            autocompact: None,
            autocompact_pct: None,
            spec: ModelSpec::Pinned {
                model: model.to_string(),
                provider: Some(provider.to_string()),
            },
        }
    }

    fn cached_model(id: &str, provider: &str, release_date: Option<&str>) -> models::CachedModel {
        models::CachedModel {
            id: id.to_string(),
            provider: provider.to_string(),
            release_date: release_date.map(|value| value.to_string()),
            description: Some(format!("desc-{id}")),
            context_window: None,
            max_output: None,
            cost_input: None,
            cost_output: None,
            cost_cache_read: None,
            cost_cache_write: None,
            cost_reasoning: None,
        }
    }

    fn cache(models: Vec<models::CachedModel>) -> models::ModelsCache {
        models::ModelsCache {
            models,
            fetched_at: Some("123".to_string()),
        }
    }

    fn installed(names: &[&str]) -> HashSet<String> {
        names.iter().map(|name| (*name).to_string()).collect()
    }

    fn default_routing_settings() -> ResolvedRoutingSettings {
        crate::config::routing_settings::resolve(&crate::config::Settings::default())
    }

    #[allow(clippy::too_many_arguments)]
    fn collect_all_model_entries(
        merged: &IndexMap<String, ModelAlias>,
        cache: &models::ModelsCache,
        installed: &HashSet<String>,
        opencode_probe_result: Option<&OpenCodeProbeResult>,
        pi_probe_result: Option<&PiProbeResult>,
        cursor_probe_result: Option<&CursorProbeResult>,
        is_offline: bool,
        routing_settings: &ResolvedRoutingSettings,
    ) -> Vec<ListModelEntry> {
        let catalog_slugs = models::catalog_model_slugs(cache);
        super::collect_all_model_entries(
            merged,
            cache,
            AvailabilityContext {
                installed,
                opencode_probe_result,
                pi_probe_result,
                cursor_probe_result,
                catalog_model_slugs: Some(catalog_slugs.as_slice()),
                is_offline,
                routing_settings,
            },
        )
    }

    fn collect_catalog_model_entries(
        cache: &models::ModelsCache,
        installed: &HashSet<String>,
        opencode_probe_result: Option<&OpenCodeProbeResult>,
        pi_probe_result: Option<&PiProbeResult>,
        cursor_probe_result: Option<&CursorProbeResult>,
        is_offline: bool,
        routing_settings: &ResolvedRoutingSettings,
    ) -> Vec<ListModelEntry> {
        collect_catalog_model_entries_with_auth(
            cache,
            installed,
            opencode_probe_result,
            pi_probe_result,
            cursor_probe_result,
            is_offline,
            routing_settings,
            models::harness::native_harness_authenticated,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn collect_catalog_model_entries_with_auth<F>(
        cache: &models::ModelsCache,
        installed: &HashSet<String>,
        opencode_probe_result: Option<&OpenCodeProbeResult>,
        pi_probe_result: Option<&PiProbeResult>,
        cursor_probe_result: Option<&CursorProbeResult>,
        is_offline: bool,
        routing_settings: &ResolvedRoutingSettings,
        auth_check: F,
    ) -> Vec<ListModelEntry>
    where
        F: Fn(&str) -> bool + Copy,
    {
        let catalog_slugs = models::catalog_model_slugs(cache);
        let availability_ctx = AvailabilityContext {
            installed,
            opencode_probe_result,
            pi_probe_result,
            cursor_probe_result,
            catalog_model_slugs: Some(catalog_slugs.as_slice()),
            is_offline,
            routing_settings,
        };
        let mut out: Vec<ListModelEntry> = cache
            .models
            .iter()
            .map(|model| {
                super::model_entry_for_cached_with_auth(model, availability_ctx, auth_check)
            })
            .collect();
        super::sort_list_model_entries(&mut out);
        out
    }

    #[test]
    fn list_all_shows_multiple_per_alias() {
        let mut merged = IndexMap::new();
        merged.insert(
            "opus".to_string(),
            auto_alias("Anthropic", &["claude-opus-*"], &[]),
        );

        let models_cache = cache(vec![
            cached_model("claude-opus-4-6", "Anthropic", Some("2026-02-05")),
            cached_model("claude-opus-4-7", "Anthropic", Some("2026-04-01")),
        ]);

        let installed = installed(&[]);
        let rows = collect_all_model_entries(
            &merged,
            &models_cache,
            &installed,
            None,
            None,
            None,
            false,
            &default_routing_settings(),
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "claude-opus-4-7");
        assert_eq!(rows[1].id, "claude-opus-4-6");
    }

    #[test]
    fn list_all_includes_matched_aliases_with_dedup() {
        let mut merged = IndexMap::new();
        merged.insert(
            "opus".to_string(),
            auto_alias("Anthropic", &["claude-opus-*"], &[]),
        );
        merged.insert(
            "legacy".to_string(),
            auto_alias("Anthropic", &["*4-6"], &[]),
        );

        let models_cache = cache(vec![cached_model(
            "claude-opus-4-6",
            "Anthropic",
            Some("2026-02-05"),
        )]);

        let installed = installed(&[]);
        let rows = collect_all_model_entries(
            &merged,
            &models_cache,
            &installed,
            None,
            None,
            None,
            false,
            &default_routing_settings(),
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "claude-opus-4-6");
        assert_eq!(rows[0].matched_aliases, vec!["opus", "legacy"]);
    }

    #[test]
    fn list_all_includes_pinned_cache_entries() {
        let mut merged = IndexMap::new();
        merged.insert("fixed".to_string(), pinned_alias("gpt-5.3-codex"));

        let models_cache = cache(vec![cached_model(
            "gpt-5.3-codex",
            "OpenAI",
            Some("2026-01-01"),
        )]);
        let installed = installed(&[]);
        let rows = collect_all_model_entries(
            &merged,
            &models_cache,
            &installed,
            None,
            None,
            None,
            false,
            &default_routing_settings(),
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "gpt-5.3-codex");
        assert_eq!(rows[0].matched_aliases, vec!["fixed"]);
    }

    #[test]
    fn list_all_includes_pinned_cache_miss_entries() {
        let mut merged = IndexMap::new();
        merged.insert("fixed".to_string(), pinned_alias("gpt-5.3-codex"));

        let models_cache = cache(Vec::new());
        let installed = installed(&[]);
        let rows = collect_all_model_entries(
            &merged,
            &models_cache,
            &installed,
            None,
            None,
            None,
            false,
            &default_routing_settings(),
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "gpt-5.3-codex");
        assert!(rows[0].provider.eq_ignore_ascii_case("openai"));
        assert_eq!(rows[0].release_date, None);
        assert_eq!(rows[0].matched_aliases, vec!["fixed"]);
    }

    #[test]
    fn list_all_uses_declared_provider_for_pinned_cache_miss_entries() {
        let mut merged = IndexMap::new();
        merged.insert(
            "custom".to_string(),
            pinned_alias_with_provider("custom-model-id", "Anthropic"),
        );

        let models_cache = cache(Vec::new());
        let installed = installed(&[]);
        let rows = collect_all_model_entries(
            &merged,
            &models_cache,
            &installed,
            None,
            None,
            None,
            false,
            &default_routing_settings(),
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "custom-model-id");
        assert_eq!(rows[0].provider, "Anthropic");
        assert_eq!(rows[0].release_date, None);
        assert_eq!(rows[0].matched_aliases, vec!["custom"]);
    }

    #[test]
    fn list_all_includes_unavailable_harness_entries_with_fallback_candidates() {
        let mut merged = IndexMap::new();
        merged.insert("x".to_string(), auto_alias("Unknown", &["x-*"], &[]));
        let models_cache = cache(vec![cached_model("x-1", "Unknown", Some("2026-01-01"))]);

        let installed = installed(&[]);
        let rows = collect_all_model_entries(
            &merged,
            &models_cache,
            &installed,
            None,
            None,
            None,
            false,
            &default_routing_settings(),
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].harness, None);
        assert_eq!(rows[0].harness_source, HarnessSource::Unavailable);
        assert_eq!(
            rows[0].harness_candidates,
            vec!["claude", "codex", "pi", "cursor", "opencode"]
        );
    }

    #[test]
    fn list_catalog_shows_all_cache_sorted() {
        let models_cache = cache(vec![
            cached_model("gpt-5", "OpenAI", Some("2025-06-01")),
            cached_model("claude-opus-4-6", "Anthropic", Some("2026-02-05")),
            cached_model("claude-sonnet-4-5", "Anthropic", Some("2025-08-01")),
        ]);

        let installed = installed(&[]);
        let rows = collect_catalog_model_entries(
            &models_cache,
            &installed,
            None,
            None,
            None,
            false,
            &default_routing_settings(),
        );
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].id, "claude-opus-4-6");
        assert_eq!(rows[1].id, "claude-sonnet-4-5");
        assert_eq!(rows[2].id, "gpt-5");
    }

    #[test]
    fn list_catalog_uses_catalog_slugs_for_native_harness_matching() {
        let models_cache = cache(vec![cached_model(
            "claude-opus-4-6",
            "Anthropic",
            Some("2026-02-05"),
        )]);

        let installed = installed(&["claude"]);
        let rows = collect_catalog_model_entries_with_auth(
            &models_cache,
            &installed,
            None,
            None,
            None,
            false,
            &default_routing_settings(),
            |_| true,
        );

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].harness.as_deref(), Some("claude"));
        assert_eq!(rows[0].harness_source, HarnessSource::AutoDetected);
    }

    #[test]
    fn list_all_includes_pinned_with_match_discovery_candidates() {
        let mut merged = IndexMap::new();
        merged.insert(
            "opus".to_string(),
            pinned_with_match_alias("claude-opus-4-6", "Anthropic", &["claude-opus-*"], &[]),
        );
        let models_cache = cache(vec![
            cached_model("claude-opus-4-7", "Anthropic", Some("2026-04-16")),
            cached_model("claude-opus-4-6", "Anthropic", Some("2026-02-05")),
        ]);

        let installed = installed(&[]);
        let rows = collect_all_model_entries(
            &merged,
            &models_cache,
            &installed,
            None,
            None,
            None,
            false,
            &default_routing_settings(),
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "claude-opus-4-7");
        assert_eq!(rows[1].id, "claude-opus-4-6");
        assert_eq!(rows[0].matched_aliases, vec!["opus"]);
        assert_eq!(rows[1].matched_aliases, vec!["opus"]);
    }

    #[test]
    fn resolve_pinned_with_match_uses_model_field() {
        let mut merged = IndexMap::new();
        merged.insert(
            "opus".to_string(),
            pinned_with_match_alias("claude-opus-4-6", "Anthropic", &["claude-opus-*"], &[]),
        );
        let models_cache = cache(vec![
            cached_model("claude-opus-4-7", "Anthropic", Some("2026-04-16")),
            cached_model("claude-opus-4-6", "Anthropic", Some("2026-02-05")),
        ]);
        let mut diag = DiagnosticCollector::new();
        let resolved = models::resolve_one("opus", &merged, &models_cache, &mut diag).unwrap();
        assert_eq!(resolved.model_id, "claude-opus-4-6");
        assert!(diag.drain().is_empty());
    }

    fn passthrough_trace(
        match_evidence: crate::routing::MatchEvidence,
    ) -> crate::routing::RoutingTrace {
        crate::routing::RoutingTrace {
            source: crate::routing::RouteSource::Provider,
            selection_kind: crate::routing::SelectionKind::Auto,
            match_evidence,
            harness: "opencode".to_string(),
            harness_order_position: None,
            candidates_tried: vec!["opencode".to_string()],
            assessments: Vec::new(),
            diagnostics: Vec::new(),
            exhaustion_reason: None,
        }
    }

    #[test]
    fn passthrough_catalog_warning_omits_warning_for_confirmed_and_constrained_routes() {
        assert!(
            passthrough_catalog_warning(
                "openai/gpt-5.4-mini",
                &passthrough_trace(crate::routing::MatchEvidence::Confirmed)
            )
            .is_none()
        );
        assert!(
            passthrough_catalog_warning(
                "openai/gpt-5.4-mini",
                &passthrough_trace(crate::routing::MatchEvidence::Constrained)
            )
            .is_none()
        );
    }

    #[test]
    fn passthrough_catalog_warning_keeps_warning_for_passthrough_routes() {
        let warning = passthrough_catalog_warning(
            "unknown-model",
            &passthrough_trace(crate::routing::MatchEvidence::Passthrough),
        )
        .expect("passthrough warning expected");
        assert!(warning.contains("not found in catalog"));
    }
}
