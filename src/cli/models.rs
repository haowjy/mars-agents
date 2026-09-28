//! CLI handlers for `mars models` subcommands.
#![allow(clippy::print_literal)]

use crate::routing::report::{RouteDecisionReport, SelectionOutcome};
use clap::{Parser, Subcommand};
use indexmap::IndexMap;
use std::collections::HashSet;

use crate::config::routing_settings::ResolvedRoutingSettings;
use crate::diagnostic::{Diagnostic, DiagnosticCollector, DiagnosticLevel};
use crate::error::{ConfigError, MarsError};
use crate::harness::host::{
    CapabilityCollectionOptions, CapabilitySession, ListingEvidence, NativeAuthCache,
};
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

use super::models_common::{
    catalog_providers, load_merged_aliases, load_project_config_layers_optional,
    models_cache_ttl_hours,
};
pub use super::models_prompting::PromptingArgs;

/// Manage aliases and the last-known-good models.dev catalog (24h refresh-after by default).
#[derive(Debug, Parser)]
pub struct ModelsArgs {
    #[command(subcommand)]
    pub command: ModelsCommand,
}

#[derive(Debug, Subcommand)]
pub enum ModelsCommand {
    /// Force a synchronous models.dev fetch and update the local cache.
    Refresh,
    /// List curated harness models.
    List(ListArgs),
    /// List model aliases without probing harnesses.
    Aliases(CatalogViewArgs),
    /// List raw models.dev catalog entries.
    Catalog(CatalogViewArgs),
    /// Show resolution chain for a specific alias.
    Resolve(ResolveAliasArgs),
    /// Show prompting guidance for an agent or model alias.
    Prompting(PromptingArgs),
    /// Quick-add a pinned alias to mars.toml [models].
    Alias(AddAliasArgs),
    #[command(name = "__refresh-probe", hide = true)]
    RefreshProbe(RefreshProbeArgs),
    /// Internal detached models.dev refresh worker.
    #[command(name = "__refresh-catalog", hide = true)]
    RefreshCatalog(RefreshCatalogArgs),
}

#[derive(Debug, Parser)]
pub struct ListArgs {
    /// Include hidden curated rows.
    #[arg(long)]
    pub all: bool,
    /// Assess runtime eligibility for each displayed harness model.
    #[arg(long)]
    pub live: bool,
    /// Narrow to one registered harness.
    #[arg(long)]
    pub harness: Option<String>,
    /// Narrow by glob against the launch model ID.
    #[arg(long)]
    pub r#match: Option<String>,
    /// Refresh models.dev catalog and harness probes synchronously before running (blocks until complete).
    #[arg(long, conflicts_with = "no_refresh_models")]
    pub refresh_models: bool,
    /// Use disk-only catalog/probe caches; do not start background refresh.
    #[arg(long, conflicts_with = "refresh_models")]
    pub no_refresh_models: bool,
}

#[derive(Debug, Parser)]
pub struct CatalogViewArgs {
    /// Force a models.dev catalog refresh (does not probe harnesses).
    #[arg(long, conflicts_with = "no_refresh_models")]
    pub refresh_models: bool,
    /// Use the catalog cache without starting refresh work.
    #[arg(long, conflicts_with = "refresh_models")]
    pub no_refresh_models: bool,
}

#[derive(Debug, Parser)]
pub struct ResolveAliasArgs {
    /// Alias name to resolve.
    pub name: String,
    /// Refresh models.dev catalog and harness probes synchronously before running (blocks until complete).
    #[arg(long, conflicts_with = "no_refresh_models")]
    refresh_models: bool,
    /// Use disk-only catalog/probe caches; do not start background refresh.
    #[arg(long, conflicts_with = "refresh_models")]
    no_refresh_models: bool,
}

#[derive(Debug, Parser)]
pub struct RefreshProbeArgs {
    #[arg(long)]
    target: String,
}

#[derive(Debug, Parser)]
pub struct RefreshCatalogArgs {
    #[arg(long)]
    mars_dir: std::path::PathBuf,
    #[arg(long)]
    refresh_after_hours: u32,
    #[arg(long)]
    providers_json: String,
    #[arg(long)]
    expected_generation: u64,
    #[arg(long)]
    claim_token: String,
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
        ModelsCommand::List(args) => super::models_inventory::run_list(args, ctx, json),
        ModelsCommand::Aliases(args) => super::models_inventory::run_aliases(args, ctx, json),
        ModelsCommand::Catalog(args) => super::models_inventory::run_catalog(args, ctx, json),
        ModelsCommand::Resolve(a) => run_resolve(a, ctx, json),
        ModelsCommand::Prompting(a) => super::models_prompting::run(a, ctx, json),
        ModelsCommand::Alias(a) => run_alias(a, ctx, json),
        ModelsCommand::RefreshProbe(a) => run_refresh_probe(a),
        ModelsCommand::RefreshCatalog(a) => {
            if a.mars_dir != ctx.project_root.join(".mars") {
                return Err(MarsError::Config(crate::error::ConfigError::Invalid {
                    message: "internal catalog worker path does not match project root".to_string(),
                }));
            }
            let providers: Vec<String> =
                serde_json::from_str(&a.providers_json).map_err(|error| {
                    MarsError::Config(crate::error::ConfigError::Invalid {
                        message: format!("invalid internal catalog worker providers: {error}"),
                    })
                })?;
            models::run_background_refresh(
                &a.mars_dir,
                a.refresh_after_hours,
                &providers,
                a.expected_generation,
                &a.claim_token,
            )?;
            Ok(0)
        }
    }
}

fn mars_dir(ctx: &MarsContext) -> std::path::PathBuf {
    ctx.project_root.join(".mars")
}

fn run_refresh(ctx: &MarsContext, json: bool) -> Result<i32, MarsError> {
    let mars = mars_dir(ctx);
    let project_config = load_project_config_layers_optional(&ctx.project_root)?;
    let ttl = models_cache_ttl_hours(project_config.as_ref());
    let providers = catalog_providers(project_config.as_ref());
    eprint!("Fetching models catalog... ");

    let (cache, outcome) = models::ensure_fresh_with_catalog_providers(
        &mars,
        ttl,
        models::RefreshMode::Force,
        &providers,
    )?;
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

#[derive(Clone, Copy)]
struct AvailabilityContext<'a> {
    installed: &'a HashSet<String>,
    routing_settings: &'a ResolvedRoutingSettings,
}

struct ResolveRuntime<'a> {
    auth: &'a NativeAuthCache,
    cache: &'a models::ModelsCache,
    catalog_model_slugs: &'a [String],
    outcome: &'a models::RefreshOutcome,
    installed: &'a HashSet<String>,
    routing_settings: &'a ResolvedRoutingSettings,
    probe_refresh: ProbeRefreshMode,
}

struct RouteTraceInput<'a> {
    preferred_harness: Option<&'a str>,
    auth: &'a NativeAuthCache,
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

pub(super) struct SessionProbeResolver<'a> {
    pub(super) session: &'a mut CapabilitySession,
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

    fn listing_evidence(
        &mut self,
        harness: crate::harness::registry::HarnessId,
    ) -> ListingEvidence {
        self.session.listing_evidence(harness)
    }
}

struct OutputResolvedInput<'a> {
    name: &'a str,
    resolved: &'a models::ResolvedAlias,
    source: &'a str,
    route_trace: &'a crate::routing::RoutingTrace,
    routing_settings: &'a ResolvedRoutingSettings,
    outcome: &'a models::RefreshOutcome,
    cache_outcome: &'a CachedProbeOutcome,
    probe_refresh: ProbeRefreshMode,
    routing_diagnostics: &'a [String],
    json: bool,
}

struct OutputPassthroughInput<'a> {
    auth: &'a NativeAuthCache,
    name: &'a str,
    outcome: &'a models::RefreshOutcome,
    installed: &'a HashSet<String>,
    capability_session: &'a mut CapabilitySession,
    catalog_model_slugs: Option<&'a [String]>,
    routing_settings: &'a ResolvedRoutingSettings,
    cache_error: Option<&'a str>,
    routing_diagnostics: &'a [String],
    json: bool,
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

fn route_trace_for_resolved_model_with_probes(
    input: &RouteTraceInput<'_>,
    probe_resolver: &mut dyn crate::routing::ProbeResolver,
) -> crate::routing::RoutingTrace {
    let routing_evidence = routing_settings_evidence(input);
    let mut routing_input = routing_evidence.routing_input();
    routing_input.preferred_harness = input
        .preferred_harness
        .map(|harness| (harness, crate::routing::RouteSource::Alias));
    crate::routing::evaluate_candidates(&routing_input, probe_resolver, |harness| {
        input.auth.state(harness)
    })
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

fn availability_status_label(availability: Option<&ModelAvailability>) -> &'static str {
    match availability.map(|value| value.status) {
        Some(AvailabilityStatus::Runnable) => "runnable",
        Some(AvailabilityStatus::Unavailable) => "unavailable",
        Some(AvailabilityStatus::Unknown) => "unknown",
        None => "unknown",
    }
}

fn apply_route_to_resolved_alias(
    resolved: &mut models::ResolvedAlias,
    trace: &crate::routing::RoutingTrace,
    context: AvailabilityContext<'_>,
) {
    resolved.harness = (!trace.harness.is_empty()).then(|| trace.harness.clone());
    resolved.harness_candidates =
        models::harness::harness_candidates_for_provider(&resolved.provider)
            .into_iter()
            .filter(|harness| context.routing_settings.harness_scope.permits(harness))
            .collect();
    resolved.harness_source = match crate::routing::acceptance::accept_route(
        trace,
        context.installed,
        crate::routing::acceptance::MatchPolicy::AllowPassthrough,
    ) {
        Ok(()) if trace.source == crate::routing::RouteSource::Alias => HarnessSource::Explicit,
        Ok(()) => HarnessSource::AutoDetected,
        Err(_) => HarnessSource::Unavailable,
    };
    resolved.availability = Some(models::availability::from_routing_trace(
        &resolved.model_id,
        &resolved.provider,
        trace,
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

fn model_report(
    name: &str,
    model: &str,
    source: &str,
    settings: &ResolvedRoutingSettings,
    trace: &crate::routing::RoutingTrace,
) -> RouteDecisionReport {
    let mut report =
        RouteDecisionReport::new(&settings.harness_scope, &settings.target_source, &[]);
    report.push(name, model, source, trace);
    report.select(0);
    report
}

fn add_route_json_fields(out: &mut serde_json::Value, report: &RouteDecisionReport) {
    out["route"] = serde_json::json!(
        report
            .selected_attempt()
            .map(|attempt| attempt.compact_summary())
    );
    out["route_trace"] = serde_json::json!(report);
    if report.selected.is_none() {
        let message = out
            .get("error")
            .and_then(|error| error.as_str())
            .unwrap_or("no permitted route for requested model")
            .to_string();
        out["error"] =
            serde_json::json!({"code": "model_candidates_exhausted", "message": message});
    }
}

fn print_route_text(report: &RouteDecisionReport) {
    if let Some(attempt) = report.selected_attempt() {
        println!(
            "Route:    {} ({}, {}, {})",
            attempt.harness, attempt.source, attempt.selection_kind, attempt.match_evidence
        );
    }
    println!("{report}");
}

fn run_resolve(args: &ResolveAliasArgs, ctx: &MarsContext, json: bool) -> Result<i32, MarsError> {
    let native_auth = NativeAuthCache::default();
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
    let mut routing_settings = ResolvedRoutingSettings::from_settings(settings);
    routing_settings.target_source = project_config
        .as_ref()
        .map(|config| config.effective.target_source.clone())
        .unwrap_or_default();
    let routing_diagnostics = routing_settings.diagnostic_messages();
    if !json {
        emit_routing_settings_warnings(&routing_diagnostics);
    }

    // Cache is enrichment, not a gate. If unavailable, skip to passthrough.
    let mut cache_error = None;
    let providers = catalog_providers(project_config.as_ref());
    let cache_result = match ensure_fresh_or_json_error(&mars, ttl, mode, json, &providers)? {
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
            auth: &native_auth,
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
        && let Some(mut resolved) =
            models::resolve_with_alias_prefix_static(&args.name, &merged, cache)
    {
        let base_alias = models::alias_prefix_base(&args.name, &merged);
        let provider_constraint = base_alias.and_then(models::provider_constraint_for_alias);
        let catalog_slugs = models::catalog_model_slugs(cache);
        let route_input = RouteTraceInput {
            preferred_harness: base_alias.and_then(|alias| alias.harness.as_deref()),
            auth: &native_auth,
            model_id: &resolved.model_id,
            provider_for_order: models::infer_provider_from_model_id(&resolved.model_id)
                .unwrap_or(resolved.provider.as_str()),
            provider_constraint: provider_constraint.as_deref(),
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
        apply_route_to_resolved_alias(
            &mut resolved,
            &route_trace,
            AvailabilityContext {
                installed: &installed,
                routing_settings: &routing_settings,
            },
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
            routing_settings: &routing_settings,
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
    let passthrough_catalog_slugs = cache_result
        .as_ref()
        .map(|(cache, _)| models::catalog_model_slugs(cache));
    run_output_passthrough(OutputPassthroughInput {
        auth: &native_auth,
        name: &args.name,
        outcome: &outcome,
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

enum FreshOrJsonError {
    Fresh(models::ModelsCache, models::RefreshOutcome),
    JsonError(String),
}

fn ensure_fresh_or_json_error(
    mars: &std::path::Path,
    ttl: u32,
    mode: models::RefreshMode,
    json: bool,
    providers: &[String],
) -> Result<FreshOrJsonError, MarsError> {
    match models::ensure_fresh_with_catalog_providers(mars, ttl, mode, providers) {
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
    let mut resolved_entry = models::resolve_one_static(name, merged, runtime.cache);
    let mut route_trace = None;
    if let Some(r) = resolved_entry.as_mut() {
        let provider_constraint = models::provider_constraint_for_alias(alias);
        let route_input = RouteTraceInput {
            preferred_harness: alias.harness.as_deref(),
            auth: runtime.auth,
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
        let mut probe_resolver = SessionProbeResolver {
            session: capability_session,
        };
        route_trace = Some(route_trace_for_resolved_model_with_probes(
            &route_input,
            &mut probe_resolver,
        ));
        if let Some(trace) = route_trace.as_ref() {
            apply_route_to_resolved_alias(
                r,
                trace,
                AvailabilityContext {
                    installed: runtime.installed,
                    routing_settings: runtime.routing_settings,
                },
            );
        }
    }
    let report = route_trace
        .as_ref()
        .zip(resolved_entry.as_ref())
        .map(|(trace, resolved)| {
            model_report(
                name,
                &resolved.model_id,
                &source,
                runtime.routing_settings,
                trace,
            )
        });
    let diagnostics = diag.drain();
    let probe_outcome = capability_session
        .loaded_opencode_outcome()
        .cloned()
        .unwrap_or(CachedProbeOutcome::Unavailable);

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
            if let Some(report) = report.as_ref() {
                add_route_json_fields(&mut out, report);
            }
            println!("{}", serde_json::to_string_pretty(&out).unwrap());
        } else {
            let mut out = serde_json::json!({
                "error": {"code": "model_unresolved", "message": format!("alias `{}` did not resolve to a model ID", name)},
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
            && matches!(
                probe_outcome,
                CachedProbeOutcome::Stale(_) | CachedProbeOutcome::StaleFailed(_)
            )
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
        if let Some(report) = report.as_ref() {
            print_route_text(report);
        }
        emit_drained_text_diagnostics(&diagnostics);
    }

    Ok(i32::from(
        route_trace
            .as_ref()
            .is_none_or(|trace| trace.selected_harness().is_empty()),
    ))
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
            "error": {"code": "model_unresolved", "message": error},
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

fn run_output_resolved(input: OutputResolvedInput<'_>) -> Result<i32, MarsError> {
    let OutputResolvedInput {
        name,
        resolved,
        source,
        route_trace,
        routing_settings,
        outcome,
        cache_outcome,
        probe_refresh,
        routing_diagnostics,
        json,
    } = input;
    let report = model_report(
        name,
        &resolved.model_id,
        source,
        routing_settings,
        route_trace,
    );
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
        add_route_json_fields(&mut out, &report);
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else {
        if probe_refresh == ProbeRefreshMode::Background
            && matches!(
                cache_outcome,
                CachedProbeOutcome::Stale(_) | CachedProbeOutcome::StaleFailed(_)
            )
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
        print_route_text(&report);
    }

    Ok(i32::from(route_trace.selected_harness().is_empty()))
}

fn run_output_passthrough(input: OutputPassthroughInput<'_>) -> Result<i32, MarsError> {
    let OutputPassthroughInput {
        auth,
        name,
        outcome,
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
                "error": {"code": "invalid_request", "message": "model name cannot be empty"},
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
        crate::routing::evaluate_candidates(
            &routing_evidence.routing_input(),
            &mut probe_resolver,
            |harness| auth.state(harness),
        )
    };
    let mut report = model_report(
        name,
        &passthrough_model_id,
        "passthrough",
        routing_settings,
        &trace,
    );
    let availability = models::availability::from_routing_trace(
        &passthrough_model_id,
        provider_for_classification,
        &trace,
    );
    if let Err(rejection_reason) = crate::routing::acceptance::accept_route(
        &trace,
        installed,
        crate::routing::acceptance::MatchPolicy::RequireSlugEvidence,
    ) {
        report.selected = None;
        report.outcome = SelectionOutcome::Exhausted;
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
            add_route_json_fields(&mut out, &report);
            add_availability_json_fields(&mut out, Some(&availability));
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
            print_route_text(&report);
            print_availability_text(Some(&availability));
        }
        return Ok(1);
    }

    let harness = installed
        .contains(trace.selected_harness())
        .then_some(trace.selected_harness().to_string());
    let harness_source = "pattern_guess";
    let harness_candidates = models::harness::harness_candidates_for_provider(provider_for_order);

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
        add_route_json_fields(&mut out, &report);
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
        print_route_text(&report);
    }

    Ok(0)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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
    Some(format!(
        "No permitted, runnable harness for model '{}'",
        resolved.model_id
    ))
}

fn passthrough_rejection_message(
    model_name: &str,
    rejection: &crate::routing::acceptance::RejectionReason,
) -> String {
    match rejection {
        crate::routing::acceptance::RejectionReason::HarnessNotInstalled { harness }
            if harness.is_empty() =>
        {
            format!("No permitted, runnable harness for model '{model_name}'")
        }
        crate::routing::acceptance::RejectionReason::HarnessNotInstalled { harness } => format!(
            "model '{model_name}' selected harness '{harness}', but that harness is not installed"
        ),
        crate::routing::acceptance::RejectionReason::NoSlugEvidence { .. } => format!(
            "model '{model_name}' could not be confirmed by an available harness model listing"
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
        crate::routing::acceptance::RejectionReason::HarnessNotInstalled { harness }
            if harness.is_empty() =>
        {
            serde_json::json!({"reason": "no_runnable_route", "harness": null})
        }
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

fn cache_warning(outcome: &models::RefreshOutcome) -> Option<String> {
    models::refresh_warning(outcome)
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

fn emit_drained_text_diagnostics(diagnostics: &[Diagnostic]) {
    for diagnostic in diagnostics {
        let label = diagnostic_level_label(diagnostic.level);
        eprintln!("{label}: {}", diagnostic.message);
    }
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
    use tempfile::TempDir;

    fn write_mars_toml(temp: &TempDir, contents: &str) {
        std::fs::write(temp.path().join("mars.toml"), contents).unwrap();
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
    fn command_matrix_keeps_aliases_catalog_and_new_list_flags_distinct() {
        for command in [
            "list",
            "aliases",
            "catalog",
            "resolve",
            "prompting",
            "alias",
            "refresh",
        ] {
            assert!(
                ModelsArgs::try_parse_from(["mars", command, "--help"]).is_err(),
                "{command} should have its own help surface"
            );
        }
        assert!(
            ModelsArgs::try_parse_from([
                "mars",
                "list",
                "--all",
                "--live",
                "--harness",
                "codex",
                "--match",
                "gpt-*"
            ])
            .is_ok()
        );
        for command in ["aliases", "catalog"] {
            assert!(ModelsArgs::try_parse_from(["mars", command, "--no-refresh-models"]).is_ok());
            assert!(ModelsArgs::try_parse_from(["mars", command, "--live"]).is_err());
        }
        for flag in [
            "--include",
            "--exclude",
            "--providers",
            "--no-visibility",
            "--catalog",
            "--unavailable",
        ] {
            assert!(
                ModelsArgs::try_parse_from(["mars", "list", flag]).is_err(),
                "{flag}"
            );
        }
    }

    #[test]
    fn resolve_alias_args_parses_no_refresh_models() {
        let args =
            ResolveAliasArgs::try_parse_from(["mars", "opus", "--no-refresh-models"]).unwrap();
        assert!(args.no_refresh_models);
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

    fn passthrough_trace(
        match_evidence: crate::routing::MatchEvidence,
    ) -> crate::routing::RoutingTrace {
        crate::routing::RoutingTrace {
            source: crate::routing::RouteSource::Provider,
            selection_kind: crate::routing::SelectionKind::Auto,
            selected_by_preference: false,
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
