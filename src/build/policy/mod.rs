use std::collections::BTreeMap;
use std::path::Path;

use crate::build::bundle::ExecutionPolicy;
use crate::compiler::agents::AgentProfile;
use crate::error::MarsError;
use crate::harness::host::{CapabilityCollectionOptions, collect_capability_snapshot};

mod config;
mod execution;
mod harness;
mod model;
mod runnable;

pub struct PolicyInput<'a> {
    pub project_root: &'a Path,
    pub profile: &'a AgentProfile,
    pub model_override: Option<&'a str>,
    pub harness_override: Option<&'a str>,
    pub effort_override: Option<&'a str>,
    pub approval_override: Option<&'a str>,
    pub sandbox_override: Option<&'a str>,
}

pub struct ResolvedPolicy {
    pub routing: crate::build::bundle::Routing,
    pub execution_policy: ExecutionPolicy,
    pub provenance: BTreeMap<String, String>,
    pub warnings: Vec<String>,
}

pub fn resolve_policy(input: PolicyInput<'_>) -> Result<ResolvedPolicy, MarsError> {
    let mut warnings = Vec::new();
    let mut provenance = BTreeMap::new();

    let resolution_config = config::load_policy_resolution_config(input.project_root)?;
    let cache = model::load_models_cache(input.project_root)?;
    let resolved_model = model::resolve_model(&input, &resolution_config.aliases, &cache)?;

    warnings.extend(resolved_model.warnings);
    provenance.insert(
        "model_source".to_string(),
        resolved_model.model_source.clone(),
    );

    let capability_snapshot = collect_capability_snapshot(&CapabilityCollectionOptions {
        offline: crate::models::is_mars_offline(),
        allow_probe_refresh: true,
    });
    let installed_harnesses = capability_snapshot.installed_harnesses();
    let opencode_probe_result = capability_snapshot.opencode.result();
    let pi_probe_result = capability_snapshot.pi.result();

    let harness_resolution = harness::resolve_harness(
        &input,
        resolved_model.alias,
        harness::HarnessEvidence {
            model_id: &resolved_model.model,
            provider: resolved_model.provider.as_deref(),
            config_default_harness: resolution_config.default_harness.as_deref(),
            harness_order: resolution_config.harness_order.as_deref(),
            installed_harnesses: &installed_harnesses,
            linked_harnesses: (!resolution_config.linked_harnesses.is_empty())
                .then_some(resolution_config.linked_harnesses.as_slice()),
            opencode_probe_result,
            pi_probe_result,
        },
    )?;

    warnings.extend(harness_resolution.warnings);
    provenance.insert(
        "harness_source".to_string(),
        harness_resolution.source.to_string(),
    );
    provenance.insert(
        "route_confidence".to_string(),
        harness_resolution.route_confidence.label().to_string(),
    );
    provenance.insert(
        "candidates_tried".to_string(),
        harness_resolution.candidates_tried.join(","),
    );
    if harness_resolution.source == "config-order"
        && let Some(position) = harness_resolution.harness_order_position
    {
        provenance.insert("harness_order_position".to_string(), position.to_string());
    }
    if harness_resolution.is_experimental {
        warnings.push(
            "Cursor is an experimental launch-bundle target. The contract may change without notice.".to_string(),
        );
        provenance.insert("harness_stability".to_string(), "experimental".to_string());
    }

    let matched_harness_override = input
        .profile
        .harness_overrides
        .get(&harness_resolution.resolved_harness);
    let execution_resolution =
        execution::resolve_execution_policy(&input, resolved_model.alias, matched_harness_override);

    provenance.insert(
        "effort_source".to_string(),
        execution_resolution.effort_source,
    );
    provenance.insert(
        "approval_source".to_string(),
        execution_resolution.approval_source,
    );
    provenance.insert(
        "sandbox_source".to_string(),
        execution_resolution.sandbox_source,
    );
    provenance.insert(
        "autocompact_source".to_string(),
        execution_resolution.autocompact_source,
    );
    provenance.insert(
        "autocompact_pct_source".to_string(),
        execution_resolution.autocompact_pct_source,
    );
    if execution_resolution.native_config.is_some() {
        provenance.insert(
            "native_config_source".to_string(),
            "profile-harness-override".to_string(),
        );
    }

    let routing_resolution = runnable::resolve_routing(runnable::RoutingInput {
        model: resolved_model.model,
        model_token: resolved_model.model_token,
        harness: harness_resolution.harness,
        harness_source: harness_resolution.source,
        route_confidence: harness_resolution.route_confidence.label().to_string(),
        provider: resolved_model.provider.as_deref(),
        opencode_probe_result,
        alias_resolution_failed: resolved_model.alias_resolution_failed,
        alias_exists: resolved_model.alias.is_some(),
        cache: &cache,
    });

    warnings.extend(routing_resolution.warnings);

    Ok(ResolvedPolicy {
        routing: routing_resolution.routing,
        execution_policy: ExecutionPolicy {
            effort: execution_resolution.effort,
            approval: execution_resolution.approval,
            sandbox: execution_resolution.sandbox,
            autocompact: execution_resolution.autocompact,
            autocompact_pct: execution_resolution.autocompact_pct,
            timeout: None,
            native_config: execution_resolution.native_config,
            codex_rules: None,
        },
        provenance,
        warnings,
    })
}
