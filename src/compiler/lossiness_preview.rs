//! Preview lossiness diagnostics for source-tree agents/skills without running sync.
//!
//! `mars check` and `mars init` call this after validation/scaffolding to surface
//! field-loss warnings using the same staging, variant projection, and native
//! emission policy as the sync pipeline.

use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use tempfile::TempDir;

use crate::compiler::agent_copy;
use crate::compiler::agents::lower::lower_for_harness_with_model;
use crate::compiler::agents::{HarnessKind, parse_agent_profile};
use crate::compiler::agent_surface_policy;
use crate::compiler::native_agents::{NativeModelRoutingRuntime, qualifying_agent_emissions};
use crate::compiler::variants;
use crate::config::routing_settings::ResolvedRoutingSettings;
use crate::config::{SkillOverlay, Settings};
use crate::diagnostic::{Diagnostic, DiagnosticCollector};
use crate::dialect::Dialect;
use crate::error::{ConfigError, MarsError};
use crate::frontmatter;
use crate::lock::ItemKind;
use crate::local_source::LOCAL_SOURCE_DIR;
use crate::models::ModelsCache;
use crate::target::TargetRegistry;
use crate::types::RenameMap;

/// Harnesses that would receive native agent artifacts during sync for these settings.
pub fn configured_emit_harnesses(settings: &Settings) -> Vec<HarnessKind> {
    settings
        .managed_targets()
        .iter()
        .filter_map(|t| HarnessKind::from_target_dir(t))
        .collect()
}

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

/// Lower discovered source agents/skills against configured target harnesses and
/// return lossiness diagnostics.
pub fn collect_source_lossiness_diagnostics(
    base: &Path,
    settings: &Settings,
) -> Result<Vec<Diagnostic>, MarsError> {
    let native_skill_keys = native_skill_harness_keys(settings);
    let configured_harnesses = configured_emit_harnesses(settings);
    if native_skill_keys.is_empty() && configured_harnesses.is_empty() {
        return Ok(Vec::new());
    }

    let (source_root, dialect) = preview_staging_root_and_dialect(base)?;
    let skill_overrides = load_skill_overrides(base)?;
    let staging = TempDir::new()?;
    crate::staging::stage_canonical_source(
        &source_root,
        staging.path(),
        dialect,
        &skill_overrides,
        &RenameMap::new(),
        None,
    )?;

    let mut diag = DiagnosticCollector::new();
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
    let model_aliases = load_model_aliases(base)?;
    let models_cache = ModelsCache {
        models: Vec::new(),
        fetched_at: None,
    };
    let routing_settings = ResolvedRoutingSettings::from_settings(settings);
    let mut model_router =
        (!matches!(policy, crate::compiler::AgentSurfacePolicy::SuppressAll)).then(|| {
            NativeModelRoutingRuntime::collect(&model_aliases, &models_cache, routing_settings)
        });

    let discovered = crate::discover::discover_resolved_source(staging.path(), None)?;
    for item in discovered {
        match item.id.kind {
            ItemKind::Agent => {
                let path = skill_or_agent_path(staging.path(), &item);
                collect_staged_agent_lossiness(
                    &path,
                    item.id.name.as_str(),
                    &policy,
                    &configured_harnesses,
                    fanout_agents,
                    model_router.as_mut(),
                    &mut diag,
                );
            }
            ItemKind::Skill => {
                let skill_dir = skill_or_agent_path(staging.path(), &item);
                let skill_name = item.id.name.as_str();
                variants::validate_skill_variants(&skill_dir, skill_name, &mut diag);
                for harness_key in &native_skill_keys {
                    variants::emit_staged_skill_lossiness_for_harness(
                        &skill_dir,
                        harness_key.as_str(),
                        skill_name,
                        &mut diag,
                    )?;
                }
            }
            ItemKind::Hook | ItemKind::McpServer | ItemKind::BootstrapDoc => {}
        }
    }

    Ok(diag.drain())
}

fn preview_staging_root_and_dialect(base: &Path) -> Result<(PathBuf, Dialect), MarsError> {
    let config = match crate::config::load(base) {
        Ok(config) => Some(config),
        Err(MarsError::Config(ConfigError::NotFound { .. })) => None,
        Err(err) => return Err(err),
    };
    let is_consumer = is_consumer_project(base, config.as_ref());
    if is_consumer {
        let local_root = base.join(LOCAL_SOURCE_DIR);
        if local_root.is_dir() {
            return Ok((
                local_root.clone(),
                Dialect::resolve_local(None, &local_root),
            ));
        }
        return Ok((base.to_path_buf(), Dialect::resolve_local(None, base)));
    }
    Ok((base.to_path_buf(), Dialect::resolve(None, base)))
}

fn is_consumer_project(base: &Path, config: Option<&crate::config::Config>) -> bool {
    let Some(config) = config else {
        return false;
    };
    if config.package.is_some() {
        return false;
    }
    base.join(LOCAL_SOURCE_DIR).is_dir() || base.join(".mars").is_dir()
}

fn load_skill_overrides(base: &Path) -> Result<IndexMap<String, SkillOverlay>, MarsError> {
    match crate::config::load(base) {
        Ok(config) => {
            let effective =
                crate::config::merge(config, crate::config::LocalConfig::default())?;
            Ok(effective.skills)
        }
        Err(MarsError::Config(ConfigError::NotFound { .. })) => Ok(IndexMap::new()),
        Err(err) => Err(err),
    }
}

fn load_model_aliases(base: &Path) -> Result<IndexMap<String, crate::models::ModelAlias>, MarsError> {
    match crate::config::load_effective_project_config(base) {
        Ok(effective) => Ok(effective.models),
        Err(MarsError::Config(ConfigError::NotFound { .. })) => Ok(IndexMap::new()),
        Err(err) => Err(err),
    }
}

fn skill_or_agent_path(staged_root: &Path, item: &crate::discover::DiscoveredItem) -> PathBuf {
    if item.source_path == Path::new(".") {
        staged_root.to_path_buf()
    } else {
        staged_root.join(&item.source_path)
    }
}

fn collect_staged_agent_lossiness(
    path: &Path,
    agent_name: &str,
    policy: &crate::compiler::AgentSurfacePolicy,
    configured_harnesses: &[HarnessKind],
    fanout_agents: &[String],
    model_router: Option<&mut NativeModelRoutingRuntime<'_>>,
    diag: &mut DiagnosticCollector,
) {
    let Some(model_router) = model_router else {
        return;
    };
    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };
    let Ok(fm) = frontmatter::parse(&content) else {
        return;
    };
    let mut agent_diags = Vec::new();
    let profile = parse_agent_profile(&fm, &mut agent_diags);
    if agent_diags.iter().any(|d| d.is_error()) {
        return;
    }
    let body = fm.body();
    for (harness, model) in qualifying_agent_emissions(
        &profile,
        agent_name,
        policy,
        fanout_agents,
        None,
        configured_harnesses,
        model_router,
    ) {
        let lowered = lower_for_harness_with_model(&harness, &profile, &fm, body, &model);
        crate::compiler::lossiness::emit_agent_lossiness_warnings(
            agent_name,
            &lowered.lossy_fields,
            diag,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Settings;
    use crate::diagnostic::DiagnosticCategory;
    use tempfile::TempDir;

    #[test]
    fn configured_emit_harnesses_uses_managed_targets() {
        let mut settings = Settings::default();
        settings.targets = Some(vec![".cursor".into(), ".agents".into()]);
        let harnesses = configured_emit_harnesses(&settings);
        assert_eq!(harnesses, vec![HarnessKind::Cursor]);
    }

    #[test]
    fn collect_source_lossiness_empty_without_targets() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("agents")).unwrap();
        std::fs::write(
            dir.path().join("agents/coder.md"),
            "---\nname: coder\ndescription: test\nharness-overrides:\n  cursor:\n    native-config:\n      x: true\n---\n# Coder",
        )
        .unwrap();

        let diags = collect_source_lossiness_diagnostics(dir.path(), &Settings::default()).unwrap();
        assert!(diags.is_empty());
    }

    #[test]
    fn collect_source_lossiness_reports_agent_field_loss() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("mars.toml"),
            "[settings]\ntargets = [\".cursor\"]\nagent_emission = \"always\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("agents")).unwrap();
        std::fs::write(
            dir.path().join("agents/worker.md"),
            "---\nname: worker\ndescription: test\nharness-overrides:\n  cursor:\n    native-config:\n      cursor.only: true\n---\n# Worker",
        )
        .unwrap();

        let settings = crate::config::load(dir.path()).unwrap().settings;
        let diags = collect_source_lossiness_diagnostics(dir.path(), &settings).unwrap();
        assert!(
            diags.iter().any(|d| {
                d.category == Some(DiagnosticCategory::Lossiness)
                    && d.message.contains("native-config")
            }),
            "expected lossiness diagnostic: {diags:?}"
        );
    }

    #[test]
    fn collect_source_lossiness_suppresses_agents_under_never_emission() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("mars.toml"),
            "[settings]\ntargets = [\".cursor\"]\nagent_emission = \"never\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("agents")).unwrap();
        std::fs::write(
            dir.path().join("agents/worker.md"),
            "---\nname: worker\ndescription: test\nharness-overrides:\n  cursor:\n    native-config:\n      cursor.only: true\n---\n# Worker",
        )
        .unwrap();

        let settings = crate::config::load(dir.path()).unwrap().settings;
        let diags = collect_source_lossiness_diagnostics(dir.path(), &settings).unwrap();
        assert!(
            diags.iter().all(|d| d.category != Some(DiagnosticCategory::Lossiness)),
            "expected no agent lossiness under SuppressAll: {diags:?}"
        );
    }

    #[test]
    fn collect_source_lossiness_stages_foreign_claude_skill() {
        let dir = TempDir::new().unwrap();
        let skill = dir.path().join(".claude/skills/demo");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: demo\ndescription: d\ndisable-model-invocation: true\n---\n# Body\n",
        )
        .unwrap();

        let mut settings = Settings::default();
        settings.targets = Some(vec![".codex".into()]);
        settings.agent_emission = Some(crate::config::AgentEmission::Always);
        let diags = collect_source_lossiness_diagnostics(dir.path(), &settings).unwrap();
        assert!(
            diags.iter().any(|d| {
                d.category == Some(DiagnosticCategory::Lossiness)
                    && d.message.contains("model-invocable")
                    && d.message.contains(".codex")
            }),
            "expected codex lossiness after claude lift: {diags:?}"
        );
    }

    #[test]
    fn collect_source_lossiness_only_warns_for_native_skill_targets() {
        let dir = TempDir::new().unwrap();
        let skill = dir.path().join("skills/demo");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: demo\ndescription: d\nwhen_to_use: planning\n---\n# Body\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("mars.toml"),
            "[settings]\ntargets = [\".agents\"]\nagent_emission = \"always\"\n",
        )
        .unwrap();

        let settings = crate::config::load(dir.path()).unwrap().settings;
        let diags = collect_source_lossiness_diagnostics(dir.path(), &settings).unwrap();
        assert!(
            diags.iter().all(|d| d.category != Some(DiagnosticCategory::Lossiness)),
            ".agents does not project native skills — expected no skill lossiness: {diags:?}"
        );
    }
}
