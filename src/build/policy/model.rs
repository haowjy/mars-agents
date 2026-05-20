use std::path::Path;

use indexmap::IndexMap;

use crate::build::policy::PolicyInput;
use crate::error::{ConfigError, MarsError};
use crate::models::{self, ModelAlias, ModelsCache};

pub(super) struct ResolvedModel<'a> {
    pub(super) model_token: String,
    pub(super) model_source: String,
    pub(super) model: String,
    pub(super) alias: Option<&'a ModelAlias>,
    pub(super) alias_resolution_failed: bool,
    pub(super) provider: Option<String>,
    pub(super) warnings: Vec<String>,
}

/// Resolve model selection precedence for launch-bundle.
///
/// Resolution order is `cli > profile > project > error` where project maps to
/// `settings.default_model` from `mars.toml`.
pub(super) fn resolve_model<'a>(
    input: &PolicyInput<'_>,
    aliases: &'a IndexMap<String, ModelAlias>,
    cache: &ModelsCache,
) -> Result<ResolvedModel<'a>, MarsError> {
    let mut warnings = Vec::new();

    let (model_token, model_source) = match input.model_override {
        Some(model) => (model.to_string(), "cli".to_string()),
        None => match input.profile.model.as_deref() {
            Some(model) => (model.to_string(), "profile".to_string()),
            None => match input.config_default_model {
                Some(model) => (model.to_string(), "project".to_string()),
                None => {
                    return Err(MarsError::Config(ConfigError::Invalid {
                        message: "launch-bundle requires a model (set `model:` in the agent profile, set `settings.default_model` in mars.toml, or pass `--model`)"
                            .to_string(),
                    }));
                }
            },
        },
    };

    let alias = aliases.get(&model_token);
    let mut alias_resolution_failed = false;
    let model = if let Some(alias) = alias {
        match models::resolve_model_id_for_alias(alias, cache) {
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

    let provider = alias
        .and_then(|entry| models::resolve_provider_for_alias(entry, cache))
        .or_else(|| models::infer_provider_from_model_id(&model).map(str::to_string));

    Ok(ResolvedModel {
        model_token,
        model_source,
        model,
        alias,
        alias_resolution_failed,
        provider,
        warnings,
    })
}

pub(super) fn load_models_cache(project_root: &Path) -> Result<ModelsCache, MarsError> {
    let mars_dir = project_root.join(".mars");
    models::read_cache(&mars_dir)
}
