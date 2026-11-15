use indexmap::IndexMap;

use crate::build::policy::fallback_model_policy_entries;
use crate::compiler::agents::lower::NativeModel;
use crate::models::{ModelAlias, ModelSpec};
use mars_schema::HarnessId;
use mars_schema::model_policy::ModelPolicyRule;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ModelFallbackReason {
    NotPackageAlias,
    NoDeterministicId,
}

pub(super) struct ResolvedModel {
    pub native: NativeModel,
    pub fallback: Option<ModelFallbackReason>,
    /// The token `native` names: the agent's own `model:`, or the first `model-policies`
    /// backup that runs on the harness. `None` when no model is set.
    pub token: Option<String>,
}

/// The agent's `model:` on this harness; when that doesn't run here, the first
/// `model-policies` rule, in written order, whose model does. A `no-fallback` rule, a
/// glob and a repeated entry are skipped as launch fallback skips them. `when` is
/// guidance for the caller, not a condition. With no rule that fits, the result is the
/// harness default and the reason the agent's own model didn't resolve.
pub(super) fn resolve_model(
    token: Option<&str>,
    policies: &[ModelPolicyRule],
    harness: HarnessId,
    native_aliases: &[&str],
    aliases: &IndexMap<String, ModelAlias>,
) -> ResolvedModel {
    let own = resolve_token(token, harness, native_aliases, aliases);
    if token.is_none() || own.token.is_some() {
        return own;
    }
    fallback_model_policy_entries(policies)
        .map(|(_, backup)| backup)
        .find_map(|backup| {
            let resolved = resolve_token(Some(backup), harness, native_aliases, aliases);
            resolved.token.is_some().then_some(resolved)
        })
        .unwrap_or(own)
}

fn resolve_token(
    token: Option<&str>,
    harness: HarnessId,
    native_aliases: &[&str],
    aliases: &IndexMap<String, ModelAlias>,
) -> ResolvedModel {
    let descriptor = crate::compiler::harness_descriptor::descriptor(harness);
    let Some(token) = token else {
        return ResolvedModel {
            native: NativeModel::Clear,
            fallback: None,
            token: None,
        };
    };
    let Some(alias) = aliases.get(token) else {
        return ResolvedModel {
            native: NativeModel::Clear,
            fallback: Some(ModelFallbackReason::NotPackageAlias),
            token: None,
        };
    };

    let target = alias_target(alias);
    if let Some(model) = alias.pinned_model_id()
        && target == Some(descriptor.id)
        // A file-less OpenCode agent names its model as provider and id, so a bare id cannot be set.
        && (harness != HarnessId::OpenCode || model.contains('/'))
    {
        return ResolvedModel {
            native: NativeModel::Set(model.to_string()),
            fallback: None,
            token: Some(token.to_string()),
        };
    }
    if native_aliases.contains(&token) && target == Some(descriptor.id) {
        return ResolvedModel {
            native: NativeModel::Set(token.to_string()),
            fallback: None,
            token: Some(token.to_string()),
        };
    }

    ResolvedModel {
        native: NativeModel::Clear,
        fallback: Some(ModelFallbackReason::NoDeterministicId),
        token: None,
    }
}

fn alias_target(alias: &ModelAlias) -> Option<HarnessId> {
    if let Some(harness) = alias.harness.as_deref() {
        return mars_schema::harness::parse(harness);
    }

    let provider = match &alias.spec {
        ModelSpec::Pinned { model, provider }
        | ModelSpec::PinnedWithMatch {
            model, provider, ..
        } => provider
            .as_deref()
            .or_else(|| crate::models::infer_provider_from_model_id(model)),
        ModelSpec::AutoResolve { provider, .. } => provider.as_deref(),
    }?;
    crate::harness::registry::native_harness_for_provider(provider)
}
