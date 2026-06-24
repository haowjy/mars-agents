//! Preview lossiness diagnostics for source-tree agents/skills without running sync.
//!
//! `mars check` and `mars init` call this after validation/scaffolding to surface
//! field-loss warnings using the same staging, variant projection, and native
//! emission policy as the sync pipeline.
//!
//! Config lens: **publish / consumer view** — `mars.toml` only, with
//! `mars.local.toml` ignored. This matches `mars check` dependency resolution
//! (`resolve_available_skills` in `cli/check.rs`), which strips local dev overrides
//! so validation reflects what downstream consumers see.

use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use tempfile::TempDir;

use crate::compiler::agent_copy;
use crate::compiler::agent_surface_policy;
use crate::compiler::agents::lower::lower_for_harness_with_model;
use crate::compiler::agents::{HarnessKind, parse_agent_profile};
use crate::compiler::harness_descriptor::configured_emit_harnesses;
use crate::compiler::native_agents::{NativeModelRoutingRuntime, qualifying_agent_emissions};
use crate::compiler::variants;
use crate::config::routing_settings::ResolvedRoutingSettings;
use crate::config::{Config, LocalConfig, Settings, SkillOverlay};
use crate::diagnostic::{Diagnostic, DiagnosticCollector, LossinessMode};
use crate::dialect::Dialect;
use crate::error::{ConfigError, MarsError};
use crate::frontmatter;
use crate::local_source::LOCAL_SOURCE_DIR;
use crate::lock::ItemKind;
use crate::models::ModelsCache;
use crate::skill_source_name::flat_root_skill_source_name;
use crate::target::TargetRegistry;
use crate::types::RenameMap;

/// Publish-view project config for lossiness preview — one load, one lens.
///
/// Ignores `mars.local.toml` so preview matches consumer-facing `mars check`
/// validation (see module docs).
#[derive(Debug, Clone)]
struct PublishPreviewConfig {
    config: Option<Config>,
    settings: Settings,
    skills: IndexMap<String, SkillOverlay>,
    models: IndexMap<String, crate::models::ModelAlias>,
}

impl Default for PublishPreviewConfig {
    fn default() -> Self {
        Self {
            config: None,
            settings: Settings::default(),
            skills: IndexMap::new(),
            models: IndexMap::new(),
        }
    }
}

/// Load `mars.toml` once and derive all preview inputs from the publish lens.
fn load_publish_preview_config(base: &Path) -> Result<PublishPreviewConfig, MarsError> {
    let config = match crate::config::load(base) {
        Ok(config) => Some(config),
        Err(MarsError::Config(ConfigError::NotFound { .. })) => None,
        Err(err) => return Err(err),
    };
    let Some(config) = config else {
        return Ok(PublishPreviewConfig::default());
    };
    let effective = crate::config::merge(config.clone(), LocalConfig::default())?;
    Ok(PublishPreviewConfig {
        settings: effective.settings,
        skills: effective.skills,
        models: crate::config::layering::overlay_models_replace_by_key(
            &config.models,
            &LocalConfig::default(),
        ),
        config: Some(config),
    })
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
    mode: LossinessMode,
) -> Result<Vec<Diagnostic>, MarsError> {
    let preview = load_publish_preview_config(base)?;
    let settings = &preview.settings;
    let native_skill_keys = native_skill_harness_keys(settings);
    let configured_harnesses = configured_emit_harnesses(settings);
    if native_skill_keys.is_empty() && configured_harnesses.is_empty() {
        return Ok(Vec::new());
    }

    let (source_root, dialect, fallback_skill_name) =
        preview_staging_root_and_dialect(base, preview.config.as_ref())?;
    let staging = TempDir::new()?;
    let mut diag = DiagnosticCollector::with_lossiness_mode(mode);
    crate::staging::stage_canonical_source(
        &source_root,
        staging.path(),
        dialect,
        &preview.skills,
        &RenameMap::new(),
        fallback_skill_name.as_deref(),
        &mut diag,
    )?;

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
    let mut model_router = (!matches!(policy, crate::compiler::AgentSurfacePolicy::SuppressAll))
        .then(|| {
            NativeModelRoutingRuntime::collect(&preview.models, &models_cache, routing_settings)
        });

    let discovered =
        crate::discover::discover_resolved_source(staging.path(), fallback_skill_name.as_deref())?;
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

fn preview_staging_root_and_dialect(
    base: &Path,
    config: Option<&Config>,
) -> Result<(PathBuf, Dialect, Option<String>), MarsError> {
    let fallback_skill_name = preview_flat_skill_fallback_name(base, config);
    let is_consumer = is_consumer_project(base, config);
    if is_consumer {
        let local_root = base.join(LOCAL_SOURCE_DIR);
        if local_root.is_dir() {
            return Ok((
                local_root.clone(),
                Dialect::resolve_local(None, &local_root),
                fallback_skill_name,
            ));
        }
        return Ok((
            base.to_path_buf(),
            Dialect::resolve_local(None, base),
            fallback_skill_name,
        ));
    }
    Ok((
        base.to_path_buf(),
        Dialect::resolve(None, base),
        fallback_skill_name,
    ))
}

/// Stable flat-root skill name for preview staging/discovery.
///
/// Matches dependency staging: package name when declared, otherwise the source
/// directory basename — never the ephemeral temp staging directory name.
fn preview_flat_skill_fallback_name(base: &Path, config: Option<&Config>) -> Option<String> {
    if !base.join("SKILL.md").is_file() {
        return None;
    }
    Some(flat_root_skill_source_name(
        base,
        config
            .and_then(|c| c.package.as_ref())
            .map(|p| p.name.as_str()),
    ))
}

/// True when the project has local consumer authoring content in `.mars-src/`.
///
/// `.mars/` is a sync cache and must not flip preview into local-consumer mode;
/// sync discovers `_self` items only from `.mars-src/` (`local_source.rs`).
fn is_consumer_project(base: &Path, config: Option<&Config>) -> bool {
    let Some(config) = config else {
        return false;
    };
    if config.package.is_some() {
        return false;
    }
    base.join(LOCAL_SOURCE_DIR).is_dir()
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
    use crate::diagnostic::DiagnosticCategory;
    use tempfile::TempDir;

    #[test]
    fn configured_emit_harnesses_uses_managed_targets() {
        let settings = Settings {
            targets: Some(vec![".cursor".into(), ".agents".into()]),
            ..Default::default()
        };
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

        let diags =
            collect_source_lossiness_diagnostics(dir.path(), LossinessMode::Surface).unwrap();
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

        let diags =
            collect_source_lossiness_diagnostics(dir.path(), LossinessMode::Surface).unwrap();
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

        let diags =
            collect_source_lossiness_diagnostics(dir.path(), LossinessMode::Surface).unwrap();
        assert!(
            diags
                .iter()
                .all(|d| d.category != Some(DiagnosticCategory::Lossiness)),
            "expected no agent lossiness under SuppressAll: {diags:?}"
        );
    }

    #[test]
    fn collect_source_lossiness_stages_foreign_claude_skill() {
        let dir = TempDir::new().unwrap();
        let skill = dir.path().join("skills/demo");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: demo\ndescription: d\nmodel-invocable: false\n---\n# Body\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("mars.toml"),
            "[settings]\ntargets = [\".codex\"]\nagent_emission = \"always\"\n",
        )
        .unwrap();

        let diags =
            collect_source_lossiness_diagnostics(dir.path(), LossinessMode::Surface).unwrap();
        assert!(
            diags.iter().any(|d| {
                d.category == Some(DiagnosticCategory::Lossiness)
                    && d.message.contains("model-invocable")
                    && d.message.contains(".codex")
            }),
            "expected codex lossiness for staged skill: {diags:?}"
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

        let diags =
            collect_source_lossiness_diagnostics(dir.path(), LossinessMode::Surface).unwrap();
        assert!(
            diags
                .iter()
                .all(|d| d.category != Some(DiagnosticCategory::Lossiness)),
            ".agents does not project native skills — expected no skill lossiness: {diags:?}"
        );
    }

    #[test]
    fn collect_source_lossiness_stable_when_mars_cache_exists() {
        let dir = TempDir::new().unwrap();
        let skill = dir.path().join("skills/demo");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: demo\ndescription: d\ndisable-model-invocation: true\n---\n# Body\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("mars.toml"),
            "[settings]\ntargets = [\".codex\"]\nagent_emission = \"always\"\n",
        )
        .unwrap();

        let before =
            collect_source_lossiness_diagnostics(dir.path(), LossinessMode::Surface).unwrap();

        std::fs::create_dir_all(dir.path().join(".mars")).unwrap();

        let after =
            collect_source_lossiness_diagnostics(dir.path(), LossinessMode::Surface).unwrap();
        let before_msgs: Vec<_> = before.iter().map(|d| d.message.clone()).collect();
        let after_msgs: Vec<_> = after.iter().map(|d| d.message.clone()).collect();
        assert_eq!(
            before_msgs, after_msgs,
            "preview must not treat .mars cache as consumer authoring: before={before:?} after={after:?}"
        );
    }

    #[test]
    fn collect_source_lossiness_flat_skill_uses_source_name_not_temp_dir() {
        let root = TempDir::new().unwrap();
        let flat_pkg = root.path().join("flat-skill");
        std::fs::create_dir_all(&flat_pkg).unwrap();
        std::fs::write(
            flat_pkg.join("SKILL.md"),
            "---\nname: flat-skill\ndescription: flat layout\nmodel-invocable: false\n---\n# Flat\n",
        )
        .unwrap();
        std::fs::write(
            flat_pkg.join("mars.toml"),
            "[settings]\ntargets = [\".codex\"]\nagent_emission = \"always\"\n",
        )
        .unwrap();

        let diags =
            collect_source_lossiness_diagnostics(&flat_pkg, LossinessMode::Surface).unwrap();
        assert!(
            diags.iter().any(|d| {
                d.category == Some(DiagnosticCategory::Lossiness)
                    && d.message.contains("flat-skill")
                    && d.message.contains("model-invocable")
            }),
            "expected flat-skill lossiness diagnostic, not temp dir name: {diags:?}"
        );
        assert!(
            !diags.iter().any(|d| d.message.contains(".tmp")),
            "must not use temp staging dir basename in diagnostics: {diags:?}"
        );
    }

    #[test]
    fn collect_source_lossiness_hidden_mode_suppresses_warnings() {
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

        let diags =
            collect_source_lossiness_diagnostics(dir.path(), LossinessMode::Hidden).unwrap();
        assert!(
            diags
                .iter()
                .all(|d| d.category != Some(DiagnosticCategory::Lossiness)),
            "Hidden mode must not emit lossiness diagnostics: {diags:?}"
        );
    }

    #[test]
    fn publish_preview_ignores_mars_local_toml() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("mars.toml"),
            "[settings]\ntargets = [\".agents\"]\nagent_emission = \"always\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("mars.local.toml"),
            "[settings]\ntargets = [\".cursor\"]\nagent_emission = \"always\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("agents")).unwrap();
        std::fs::write(
            dir.path().join("agents/worker.md"),
            "---\nname: worker\ndescription: test\nharness-overrides:\n  cursor:\n    native-config:\n      cursor.only: true\n---\n# Worker",
        )
        .unwrap();

        let diags =
            collect_source_lossiness_diagnostics(dir.path(), LossinessMode::Surface).unwrap();
        assert!(
            diags
                .iter()
                .all(|d| d.category != Some(DiagnosticCategory::Lossiness)),
            "publish lens must ignore mars.local.toml target overrides: {diags:?}"
        );
    }
}
