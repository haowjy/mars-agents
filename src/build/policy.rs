use std::collections::BTreeMap;
use std::path::Path;

use indexmap::IndexMap;

use crate::build::bundle::{ExecutionPolicy, Routing};
use crate::compiler::agents::{
    AgentProfile, ApprovalMode, EffortLevel, HarnessKind, OverrideFields, SandboxMode,
};
use crate::error::{ConfigError, MarsError};
use crate::models::availability::{RunnableConfidence, RunnablePathSource, resolve_runnable_path};
use crate::models::probes::opencode_cache;
use crate::models::{self, ModelAlias, ModelsCache};

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
    pub routing: Routing,
    pub execution_policy: ExecutionPolicy,
    pub provenance: BTreeMap<String, String>,
    pub warnings: Vec<String>,
}

pub fn resolve_policy(input: PolicyInput<'_>) -> Result<ResolvedPolicy, MarsError> {
    let mut warnings = Vec::new();
    let mut provenance = BTreeMap::new();

    let model_config = load_model_resolution_config(input.project_root)?;
    let aliases = model_config.aliases;
    let cache = load_models_cache(input.project_root)?;

    let (model_token, model_source) = match input.model_override {
        Some(model) => (model.to_string(), "cli".to_string()),
        None => match input.profile.model.as_deref() {
            Some(model) => (model.to_string(), "profile".to_string()),
            None => {
                return Err(MarsError::Config(ConfigError::Invalid {
                    message: "launch-bundle requires a model (set `model:` in the agent profile or pass `--model`)"
                        .to_string(),
                }));
            }
        },
    };

    let alias = aliases.get(&model_token);
    let mut alias_resolution_failed = false;
    let model = if let Some(alias) = alias {
        match models::resolve_model_id_for_alias(alias, &cache) {
            Some(model_id) => model_id,
            None => {
                alias_resolution_failed = true;
                warnings.push(format!(
                    "model alias `{model_token}` did not resolve from cached catalog; using token as model id"
                ));
                model_token.clone()
            }
        }
    } else {
        model_token.clone()
    };
    provenance.insert("model_source".to_string(), model_source);

    let provider = alias
        .and_then(|entry| models::resolve_provider_for_alias(entry, &cache))
        .or_else(|| models::infer_provider_from_model_id(&model).map(str::to_string));

    let profile_harness = input.profile.harness.as_ref().map(harness_kind_to_str);
    let alias_harness = alias.and_then(|entry| entry.harness.as_deref());
    let provider_harness = provider
        .as_deref()
        .and_then(models::harness::preferred_harness_for_provider);
    let config_default_harness = match model_config.default_harness.as_deref() {
        Some(value) => match normalize_harness_name(value) {
            Some(valid) => Some(valid.to_string()),
            None => {
                warnings.push(format!(
                    "settings.default_harness `{value}` is invalid; expected one of: claude, codex, opencode, cursor, pi"
                ));
                None
            }
        },
        None => None,
    };

    let model_from_cli = input.model_override.is_some();
    let (harness, harness_source) = if let Some(harness) = input.harness_override {
        (harness.to_string(), "cli")
    } else if model_from_cli {
        if let Some(harness) = alias_harness {
            (harness.to_string(), "alias")
        } else if let Some(harness) = provider_harness {
            (harness, "provider")
        } else if let Some(harness) = config_default_harness {
            (harness, "config")
        } else {
            warnings.push(
                "harness not set by CLI/profile/alias/provider/config; defaulting to `claude`"
                    .to_string(),
            );
            ("claude".to_string(), "default")
        }
    } else if let Some(harness) = profile_harness {
        (harness.to_string(), "profile")
    } else if let Some(harness) = alias_harness {
        (harness.to_string(), "alias")
    } else if let Some(harness) = provider_harness {
        (harness, "provider")
    } else if let Some(harness) = config_default_harness {
        (harness, "config")
    } else {
        warnings.push(
            "harness not set by CLI/profile/alias/provider/config; defaulting to `claude`"
                .to_string(),
        );
        ("claude".to_string(), "default")
    };
    provenance.insert("harness_source".to_string(), harness_source.to_string());
    if harness == "cursor" {
        warnings.push(
            "Cursor is an experimental launch-bundle target. The contract may change without notice.".to_string(),
        );
        provenance.insert("harness_stability".to_string(), "experimental".to_string());
    }
    let resolved_harness = HarnessKind::from_str(&harness).ok_or_else(|| {
        MarsError::Config(ConfigError::Invalid {
            message: format!(
                "resolved harness `{harness}` is invalid; expected one of: claude, codex, opencode, cursor, pi"
            ),
        })
    })?;
    let matched_harness_override = input.profile.harness_overrides.get(&resolved_harness);
    let native_config = matched_harness_override
        .and_then(|fields| fields.native_config.clone())
        .filter(|map| !map.is_empty());
    if native_config.is_some() {
        provenance.insert(
            "native_config_source".to_string(),
            "profile-harness-override".to_string(),
        );
    }

    let (effort, effort_source) = resolve_effort(&input, alias, matched_harness_override);
    provenance.insert("effort_source".to_string(), effort_source);

    let (approval, approval_source) = resolve_approval(&input, matched_harness_override);
    provenance.insert("approval_source".to_string(), approval_source);

    let (sandbox, sandbox_source) = resolve_sandbox(&input, matched_harness_override);
    provenance.insert("sandbox_source".to_string(), sandbox_source);

    let (autocompact, autocompact_source) =
        resolve_autocompact(&input, alias, matched_harness_override);
    provenance.insert("autocompact_source".to_string(), autocompact_source);

    let (autocompact_pct, autocompact_pct_source) =
        resolve_autocompact_pct(&input, alias, matched_harness_override);
    provenance.insert("autocompact_pct_source".to_string(), autocompact_pct_source);

    let provider_for_runnable = if alias_resolution_failed {
        ""
    } else {
        provider.as_deref().unwrap_or("")
    };
    let cached_probe = if harness.eq_ignore_ascii_case("opencode") {
        opencode_cache::read_cached_probe_result()
    } else {
        None
    };
    let runnable = resolve_runnable_path(
        &model,
        provider_for_runnable,
        &harness,
        cached_probe.as_ref(),
    );

    if matches!(
        runnable.source,
        RunnablePathSource::Synthesized | RunnablePathSource::Passthrough
    ) {
        warnings.push(format!(
            "model '{}' does not have a confirmed runnable path for harness '{}'; using {} path '{}'",
            model,
            harness,
            runnable.source.label(),
            runnable.harness_model_id
        ));
    }
    if runnable.confidence == RunnableConfidence::Unknown {
        warnings.push(format!(
            "harness-model for '{}' targeting '{}' is unconfirmed ({})",
            model,
            harness,
            runnable.source.label()
        ));
    }
    if alias.is_none()
        && model_token == model
        && !model_exists_in_cache(&cache, &model)
        && matches!(runnable.source, RunnablePathSource::Passthrough)
    {
        warnings.push(format!(
            "model '{}' not found in models cache; passing through as harness model ID",
            model_token
        ));
    }

    Ok(ResolvedPolicy {
        routing: Routing {
            model,
            model_token,
            harness,
            harness_model: runnable.harness_model_id,
            harness_model_source: runnable.source.label().to_string(),
            harness_model_confidence: runnable.confidence.label().to_string(),
        },
        execution_policy: ExecutionPolicy {
            effort,
            approval,
            sandbox,
            autocompact,
            autocompact_pct,
            timeout: None,
            native_config,
        },
        provenance,
        warnings,
    })
}

fn model_exists_in_cache(cache: &ModelsCache, model_id: &str) -> bool {
    cache
        .models
        .iter()
        .any(|model| model.id.eq_ignore_ascii_case(model_id))
}

fn load_models_cache(project_root: &Path) -> Result<ModelsCache, MarsError> {
    let mars_dir = project_root.join(".mars");
    models::read_cache(&mars_dir)
}

struct ModelResolutionConfig {
    aliases: IndexMap<String, ModelAlias>,
    default_harness: Option<String>,
}

fn load_model_resolution_config(project_root: &Path) -> Result<ModelResolutionConfig, MarsError> {
    let mut merged = models::builtin_aliases();
    let mut default_harness = None;

    let merged_path = project_root.join(".mars").join("models-merged.json");
    if let Ok(content) = std::fs::read_to_string(&merged_path)
        && let Ok(cached) = serde_json::from_str::<IndexMap<String, ModelAlias>>(&content)
    {
        for (name, alias) in cached {
            merged.insert(name, alias);
        }
    }

    match crate::config::load(project_root) {
        Ok(config) => {
            default_harness = config.settings.default_harness.clone();
            for (name, alias) in &config.models {
                merged.insert(name.clone(), alias.clone());
            }
        }
        Err(MarsError::Config(ConfigError::NotFound { .. })) => {}
        Err(err) => return Err(err),
    }

    Ok(ModelResolutionConfig {
        aliases: merged,
        default_harness,
    })
}

fn normalize_harness_name(value: &str) -> Option<&'static str> {
    match value.trim() {
        "claude" => Some("claude"),
        "codex" => Some("codex"),
        "opencode" => Some("opencode"),
        "cursor" => Some("cursor"),
        "pi" => Some("pi"),
        _ => None,
    }
}

fn resolve_effort(
    input: &PolicyInput<'_>,
    alias: Option<&ModelAlias>,
    matched_harness_override: Option<&OverrideFields>,
) -> (Option<String>, String) {
    if let Some(effort) = input.effort_override {
        return (Some(effort.to_string()), "cli".to_string());
    }
    if let Some(effort) = matched_harness_override.and_then(|entry| entry.effort.as_ref()) {
        return (
            Some(effort_level_to_str(effort).to_string()),
            "profile-harness-override".to_string(),
        );
    }
    if let Some(effort) = input.profile.effort.as_ref() {
        return (
            Some(effort_level_to_str(effort).to_string()),
            "profile".to_string(),
        );
    }
    if let Some(effort) = alias.and_then(|entry| entry.default_effort.clone()) {
        return (Some(effort), "alias".to_string());
    }
    (None, "unset".to_string())
}

fn resolve_approval(
    input: &PolicyInput<'_>,
    matched_harness_override: Option<&OverrideFields>,
) -> (Option<String>, String) {
    if let Some(approval) = input.approval_override {
        return (Some(approval.to_string()), "cli".to_string());
    }
    if let Some(approval) = matched_harness_override.and_then(|entry| entry.approval.as_ref()) {
        return (
            Some(approval_mode_to_str(approval).to_string()),
            "profile-harness-override".to_string(),
        );
    }
    if let Some(approval) = input.profile.approval.as_ref() {
        return (
            Some(approval_mode_to_str(approval).to_string()),
            "profile".to_string(),
        );
    }
    (None, "unset".to_string())
}

fn resolve_sandbox(
    input: &PolicyInput<'_>,
    matched_harness_override: Option<&OverrideFields>,
) -> (Option<String>, String) {
    if let Some(sandbox) = input.sandbox_override {
        return (Some(sandbox.to_string()), "cli".to_string());
    }
    if let Some(sandbox) = matched_harness_override.and_then(|entry| entry.sandbox.as_ref()) {
        return (
            Some(sandbox_mode_to_str(sandbox).to_string()),
            "profile-harness-override".to_string(),
        );
    }
    if let Some(sandbox) = input.profile.sandbox.as_ref() {
        return (
            Some(sandbox_mode_to_str(sandbox).to_string()),
            "profile".to_string(),
        );
    }
    (None, "unset".to_string())
}

fn resolve_autocompact(
    input: &PolicyInput<'_>,
    alias: Option<&ModelAlias>,
    matched_harness_override: Option<&OverrideFields>,
) -> (Option<u32>, String) {
    if let Some(autocompact) = matched_harness_override.and_then(|entry| entry.autocompact) {
        return (Some(autocompact), "profile-harness-override".to_string());
    }
    if let Some(autocompact) = input.profile.autocompact {
        return (Some(autocompact), "profile".to_string());
    }
    if let Some(autocompact) = alias.and_then(|entry| entry.autocompact) {
        return (Some(autocompact), "alias".to_string());
    }
    (None, "unset".to_string())
}

fn resolve_autocompact_pct(
    input: &PolicyInput<'_>,
    alias: Option<&ModelAlias>,
    matched_harness_override: Option<&OverrideFields>,
) -> (Option<u8>, String) {
    if let Some(autocompact_pct) = matched_harness_override.and_then(|entry| entry.autocompact_pct)
    {
        return (
            Some(autocompact_pct),
            "profile-harness-override".to_string(),
        );
    }
    if let Some(autocompact_pct) = input.profile.autocompact_pct {
        return (Some(autocompact_pct), "profile".to_string());
    }
    if let Some(autocompact_pct) = alias.and_then(|entry| entry.autocompact_pct) {
        return (Some(autocompact_pct), "alias".to_string());
    }
    (None, "unset".to_string())
}

fn harness_kind_to_str(harness: &HarnessKind) -> &'static str {
    match harness {
        HarnessKind::Claude => "claude",
        HarnessKind::Codex => "codex",
        HarnessKind::OpenCode => "opencode",
        HarnessKind::Cursor => "cursor",
        HarnessKind::Pi => "pi",
    }
}

fn effort_level_to_str(effort: &EffortLevel) -> &'static str {
    match effort {
        EffortLevel::Low => "low",
        EffortLevel::Medium => "medium",
        EffortLevel::High => "high",
        EffortLevel::XHigh => "xhigh",
    }
}

fn approval_mode_to_str(mode: &ApprovalMode) -> &'static str {
    match mode {
        ApprovalMode::Default => "default",
        ApprovalMode::Auto => "auto",
        ApprovalMode::Confirm => "confirm",
        ApprovalMode::Yolo => "yolo",
    }
}

fn sandbox_mode_to_str(mode: &SandboxMode) -> &'static str {
    match mode {
        SandboxMode::Default => "default",
        SandboxMode::ReadOnly => "read-only",
        SandboxMode::WorkspaceWrite => "workspace-write",
        SandboxMode::DangerFullAccess => "danger-full-access",
    }
}
