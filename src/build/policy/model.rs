use std::path::Path;

use indexmap::IndexMap;

use crate::build::policy::{PolicyInput, PolicySource};
use crate::config::AgentOverlay;
use crate::error::{ConfigError, MarsError};
use crate::models::{self, ModelAlias, ModelsCache};

pub(super) struct ResolvedModel<'a> {
    pub(super) model_token: String,
    pub(super) model_source: PolicySource,
    pub(super) model: String,
    pub(super) alias: Option<&'a ModelAlias>,
    pub(super) alias_resolution_failed: bool,
    pub(super) provider_for_order: Option<String>,
    pub(super) provider_constraint: Option<String>,
    pub(super) warnings: Vec<String>,
}

/// Resolve model selection precedence for launch-bundle.
///
/// Resolution order is `cli > profile > project > error` where project maps to
/// `settings.default_model` from `mars.toml`.
pub(super) fn resolve_model<'a>(
    input: &PolicyInput<'_>,
    overlay: Option<&AgentOverlay>,
    aliases: &'a IndexMap<String, ModelAlias>,
    cache: &ModelsCache,
) -> Result<ResolvedModel<'a>, MarsError> {
    let mut warnings = Vec::new();

    let (model_token, model_source) = match input.model_override {
        Some(model) => (model.to_string(), PolicySource::Cli),
        None => match overlay.and_then(|entry| entry.model.as_deref()) {
            Some(model) => (model.to_string(), PolicySource::Overlay),
            None => match input.profile.model.as_deref() {
                Some(model) => (model.to_string(), PolicySource::Profile),
                None => match input.config_default_model {
                    Some(model) => (model.to_string(), PolicySource::Project),
                    None => {
                        return Err(MarsError::Config(ConfigError::Invalid {
                            message: "launch-bundle requires a model (set `model:` in the agent profile, set `settings.default_model` in mars.toml, or pass `--model`)"
                                .to_string(),
                        }));
                    }
                },
            },
        },
    };

    let alias = aliases.get(&model_token);
    let (raw_model_token, token_provider_constraint) =
        models::split_provider_constrained_model_token(&model_token);
    let mut alias_resolution_failed = false;
    let model = if let Some(alias) = alias {
        match models::resolve_model_id_for_alias(alias, cache) {
            Some(model_id) => model_id,
            None => {
                alias_resolution_failed = true;
                warnings.push(format!(
                    "model alias `{model_token}` did not resolve from cached catalog; using token as model id"
                ));
                raw_model_token.clone()
            }
        }
    } else {
        raw_model_token.clone()
    };

    let provider_constraint = alias
        .and_then(provider_constraint_for_alias)
        .or(token_provider_constraint.clone());
    let provider_for_order = if let Some(entry) = alias {
        models::resolve_provider_for_alias(entry, cache)
            .or_else(|| models::infer_provider_from_model_id(&model).map(str::to_string))
    } else {
        token_provider_constraint
    };

    Ok(ResolvedModel {
        model_token,
        model_source,
        model,
        alias,
        alias_resolution_failed,
        provider_for_order,
        provider_constraint,
        warnings,
    })
}

fn provider_constraint_for_alias(alias: &ModelAlias) -> Option<String> {
    match &alias.spec {
        models::ModelSpec::Pinned { provider, .. }
        | models::ModelSpec::PinnedWithMatch { provider, .. } => provider.clone(),
        models::ModelSpec::AutoResolve { provider, .. } => Some(provider.clone()),
    }
    .map(|provider| provider.trim().to_ascii_lowercase())
}

pub(super) fn load_models_cache(project_root: &Path) -> Result<ModelsCache, MarsError> {
    let mars_dir = project_root.join(".mars");
    models::read_cache(&mars_dir)
}
