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
    pub(super) provider_for_order: Option<String>,
    pub(super) provider_constraint: Option<String>,
    pub(super) warnings: Vec<String>,
}

/// Resolve model selection precedence for launch-bundle.
///
/// Resolution order is `cli > overlay > profile > project > unset` where project maps to
/// `settings.default_model` from `mars.toml`.
pub(super) fn resolve_model<'a>(
    input: &PolicyInput<'_>,
    config_default_model: Option<&str>,
    overlay: Option<&AgentOverlay>,
    aliases: &'a IndexMap<String, ModelAlias>,
    cache: &ModelsCache,
) -> Result<ResolvedModel<'a>, MarsError> {
    if input.literal_model {
        let token = input.model_override.ok_or_else(|| {
            MarsError::Config(ConfigError::Invalid {
                message: "literal model selection requires a model override".into(),
            })
        })?;
        return Ok(resolve_literal_model(token.to_string(), PolicySource::Cli));
    }
    let (model_token, model_source) = match input.model_override {
        Some(model) => (model.to_string(), PolicySource::Cli),
        None => match overlay.and_then(|entry| entry.model.as_deref()) {
            Some(model) => (model.to_string(), PolicySource::Overlay),
            None => match input.profile.model.as_deref() {
                Some(model) => (model.to_string(), PolicySource::Profile),
                None => match config_default_model {
                    Some(model) => (model.to_string(), PolicySource::Project),
                    None => (String::new(), PolicySource::Unset),
                },
            },
        },
    };

    resolve_model_token(model_token, model_source, aliases, cache)
}

pub(super) fn resolve_model_token<'a>(
    model_token: String,
    model_source: PolicySource,
    aliases: &'a IndexMap<String, ModelAlias>,
    cache: &ModelsCache,
) -> Result<ResolvedModel<'a>, MarsError> {
    let mut warnings = Vec::new();
    let alias = aliases.get(&model_token);
    let (raw_model_token, token_provider_constraint) =
        models::split_provider_constrained_model_token(&model_token);
    let model = if let Some(alias) = alias {
        match models::resolve_model_id_for_alias(alias, cache) {
            Some(model_id) => model_id,
            None => {
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
        .and_then(models::provider_constraint_for_alias)
        .or(token_provider_constraint.clone());
    let provider_for_order = if let Some(entry) = alias {
        models::resolve_provider_for_alias(entry, cache)
            .or_else(|| models::infer_provider_from_model_id(&model).map(str::to_string))
    } else {
        token_provider_constraint
            .or_else(|| models::infer_provider_from_model_id(&model).map(str::to_string))
    };

    Ok(ResolvedModel {
        model_token,
        model_source,
        model,
        alias,
        provider_for_order,
        provider_constraint,
        warnings,
    })
}

pub(super) fn resolve_literal_model(
    model_token: String,
    model_source: PolicySource,
) -> ResolvedModel<'static> {
    let (model, token_provider_constraint) =
        models::split_provider_constrained_model_token(&model_token);
    let provider_constraint = token_provider_constraint.clone();
    let provider_for_order = token_provider_constraint
        .or_else(|| models::infer_provider_from_model_id(&model).map(str::to_string));

    ResolvedModel {
        model_token,
        model_source,
        model,
        alias: None,
        provider_for_order,
        provider_constraint,
        warnings: Vec::new(),
    }
}

pub(super) fn load_models_cache(project_root: &Path) -> Result<ModelsCache, MarsError> {
    let mars_dir = project_root.join(".mars");
    models::read_cache(&mars_dir)
}
