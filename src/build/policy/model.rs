use std::path::Path;

use indexmap::IndexMap;

use crate::build::policy::{PolicyInput, PolicySource};
use crate::config::AgentOverlay;
use crate::error::MarsError;
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
/// Resolution order is `cli > overlay > profile > project > unset` where project maps to
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
                    None => (String::new(), PolicySource::Unset),
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
            .or_else(|| models::infer_provider_from_model_id(&model).map(str::to_string))
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

#[cfg(test)]
mod tests {
    use super::*;

    use crate::compiler::agents::{AgentProfile, HarnessOverrides};

    fn empty_profile() -> AgentProfile {
        AgentProfile {
            name: None,
            description: None,
            harness: None,
            model: None,
            mode: None,
            model_invocable: true,
            approval: None,
            sandbox: None,
            effort: None,
            autocompact: None,
            autocompact_pct: None,
            skills: Vec::new(),
            tools: Vec::new(),
            tools_denied: Vec::new(),
            disallowed_tools: Vec::new(),
            mcp_tools: Vec::new(),
            harness_overrides: HarnessOverrides::default(),
            model_policies: Vec::new(),
            fanout: Vec::new(),
        }
    }

    #[test]
    fn resolve_model_infers_provider_for_bare_model_id() {
        let profile = empty_profile();
        let input = PolicyInput {
            project_root: Path::new("."),
            agent: None,
            profile: &profile,
            model_override: Some("claude-opus-4-6"),
            config_default_model: None,
            harness_override: None,
            effort_override: None,
            approval_override: None,
            sandbox_override: None,
        };
        let aliases = IndexMap::new();
        let cache = ModelsCache {
            models: Vec::new(),
            fetched_at: None,
        };

        let resolved =
            resolve_model(&input, None, &aliases, &cache).expect("bare model id should resolve");

        assert_eq!(resolved.model, "claude-opus-4-6");
        assert_eq!(resolved.provider_for_order.as_deref(), Some("anthropic"));
    }

    #[test]
    fn resolve_model_returns_unset_when_no_model_source_exists() {
        let profile = empty_profile();
        let input = PolicyInput {
            project_root: Path::new("."),
            agent: None,
            profile: &profile,
            model_override: None,
            config_default_model: None,
            harness_override: None,
            effort_override: None,
            approval_override: None,
            sandbox_override: None,
        };
        let aliases = IndexMap::new();
        let cache = ModelsCache {
            models: Vec::new(),
            fetched_at: None,
        };

        let resolved =
            resolve_model(&input, None, &aliases, &cache).expect("missing model is allowed");

        assert_eq!(resolved.model_token, "");
        assert_eq!(resolved.model_source, PolicySource::Unset);
        assert_eq!(resolved.model, "");
        assert!(resolved.alias.is_none());
        assert!(!resolved.alias_resolution_failed);
        assert_eq!(resolved.provider_for_order, None);
        assert_eq!(resolved.provider_constraint, None);
        assert!(resolved.warnings.is_empty());
    }
}
