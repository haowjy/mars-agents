//! Preview lossiness diagnostics for source-tree agents/skills without running sync.
//!
//! `mars check` and `mars init` call this after validation/scaffolding to surface
//! field-loss warnings using the same staging, variant projection, and native
//! emission policy as the sync pipeline.
//!
//! Config lens: **publish / consumer view** — `mars.toml` only, with
//! `mars.local.toml` ignored. This matches `mars check` dependency resolution
//! (`resolve::PublishLens`), which strips local dev overrides
//! so validation reflects what downstream consumers see.

use std::path::Path;

use crate::compiler::agent_copy;
use crate::compiler::agent_surface_policy;
use crate::compiler::agents::lower::lower_for_harness_with_model;
use crate::compiler::harness_descriptor::configured_emit_harnesses;
use crate::compiler::lossiness;
use crate::compiler::native_agents::{
    AgentRef, NativeEmission, NativeModelRoutingRuntime, qualifying_agent_emissions,
};
use crate::compiler::variants;
use crate::config::Settings;
use crate::config::routing_settings::ResolvedRoutingSettings;
use crate::diagnostic::{Diagnostic, DiagnosticCollector, LossinessMode};
use crate::error::MarsError;
use crate::models::ModelsCache;
use crate::resolve::PublishLens;
use crate::target::TargetRegistry;
use mars_schema::HarnessId;

/// Native harness variant keys for skill projection on configured managed targets.
fn native_skill_harness_keys(settings: &Settings) -> Vec<String> {
    let registry = TargetRegistry::new();
    let mut keys = Vec::new();
    for target in settings.managed_targets() {
        if let Some(key) = registry
            .get(&target)
            .and_then(|adapter| adapter.skill_variant_key())
        {
            keys.push(key.to_string());
        }
    }
    keys
}

/// Lower an already prepared package without rediscovering or restaging it.
pub fn collect_prepared_source_lossiness_diagnostics(
    lens: &PublishLens,
    prepared: &mars_schema::prepare::Prepared,
    authored_root: &Path,
    mode: LossinessMode,
    rerun_verbose: &'static str,
) -> Result<Vec<Diagnostic>, MarsError> {
    let settings = &lens.effective.settings;
    let native_skill_keys = native_skill_harness_keys(settings);
    let configured_harnesses = configured_emit_harnesses(settings);
    if native_skill_keys.is_empty() && configured_harnesses.is_empty() {
        return Ok(Vec::new());
    }

    let mut diag = DiagnosticCollector::with_lossiness_mode(mode).rerun_verbose_with(rerun_verbose);
    let mut codex_emitted = false;
    let agent_copy_spec = agent_copy::build_agent_copy_spec(
        settings.meridian_agent_copy(),
        &settings.managed_targets(),
        &mut diag,
    );
    let policy = agent_surface_policy(
        settings.agent_emission.as_ref(),
        agent_copy_spec.as_ref(),
        false,
    );
    let fanout_agents = settings.meridian_fanout_agents();
    let models_cache = ModelsCache {
        models: Vec::new(),
        fetched_at: None,
    };
    let routing_settings = ResolvedRoutingSettings::from_settings(settings);
    let empty_model_aliases = indexmap::IndexMap::new();
    let model_aliases = lens
        .manifest
        .as_ref()
        .map(|manifest| &manifest.models)
        .unwrap_or(&empty_model_aliases);
    let mut model_router = (!matches!(policy, crate::compiler::AgentSurfacePolicy::SuppressAll))
        .then(|| {
            NativeModelRoutingRuntime::collect(model_aliases, &models_cache, routing_settings)
                .with_settings(settings.clone())
        });

    for definition in &prepared.definitions {
        let content = String::from_utf8_lossy(&definition.normalized_bytes);
        match definition.kind {
            mars_schema::item::DefinitionKind::Agent => {
                codex_emitted |= collect_agent_content_lossiness(
                    &content,
                    &definition.slug,
                    &policy,
                    &configured_harnesses,
                    fanout_agents,
                    model_router.as_mut(),
                    &mut diag,
                )
            }
            mars_schema::item::DefinitionKind::Skill => {
                let source_path = authored_root.join(&definition.source_path);
                let skill_dir = source_path.parent().unwrap_or(authored_root);
                variants::validate_skill_variants(skill_dir, &definition.slug, &mut diag);
                for harness_key in &native_skill_keys {
                    variants::emit_prepared_skill_lossiness_for_harness(
                        skill_dir,
                        &content,
                        harness_key,
                        &definition.slug,
                        &mut diag,
                    )?;
                }
            }
        }
    }

    // As in sync: the project's network settings reach no native Codex file.
    if codex_emitted {
        lossiness::record_project_network_loss(&settings.network_keys(), "Codex", &mut diag);
    }
    Ok(diag.drain())
}

fn collect_agent_content_lossiness(
    content: &str,
    agent_name: &str,
    policy: &crate::compiler::AgentSurfacePolicy,
    configured_harnesses: &[HarnessId],
    fanout_agents: &[String],
    model_router: Option<&mut NativeModelRoutingRuntime<'_>>,
    diag: &mut DiagnosticCollector,
) -> bool {
    let Some(model_router) = model_router else {
        return false;
    };
    let Ok(mars_schema::read::Valid {
        fields: profile,
        body,
        ..
    }) = mars_schema::agent::read(content).into_valid()
    else {
        return false;
    };
    let mut codex_emitted = false;
    for NativeEmission {
        harness,
        model,
        profile,
    } in qualifying_agent_emissions(
        AgentRef {
            profile: &profile,
            overlay: None,
        },
        agent_name,
        policy,
        fanout_agents,
        None,
        configured_harnesses,
        model_router,
    ) {
        codex_emitted |= harness == HarnessId::Codex;
        let lowered = lower_for_harness_with_model(&harness, &profile, &body, &model);
        diag.record_lossy_fields("agent", agent_name, &lowered.lossy_fields);
    }
    codex_emitted
}
