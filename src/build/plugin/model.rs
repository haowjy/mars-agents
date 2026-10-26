use indexmap::IndexMap;

use crate::compiler::agents::lower::NativeModel;
use crate::models::{ModelAlias, ModelSpec};
use mars_schema::HarnessId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ModelFallbackReason {
    NotPackageAlias,
    NoDeterministicId,
}

pub(super) struct ResolvedModel {
    pub native: NativeModel,
    pub fallback: Option<ModelFallbackReason>,
}

/// Resolve a package-authored model token without catalogs, caches, or host probes.
pub(super) fn resolve_model(
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
        };
    };
    let Some(alias) = aliases.get(token) else {
        return ResolvedModel {
            native: NativeModel::Clear,
            fallback: Some(ModelFallbackReason::NotPackageAlias),
        };
    };

    let target = alias_target(alias);
    if let Some(model) = alias.pinned_model_id()
        && target == Some(descriptor.id)
    {
        return ResolvedModel {
            native: NativeModel::Set(model.to_string()),
            fallback: None,
        };
    }
    if native_aliases.contains(&token) && target == Some(descriptor.id) {
        return ResolvedModel {
            native: NativeModel::Set(token.to_string()),
            fallback: None,
        };
    }

    ResolvedModel {
        native: NativeModel::Clear,
        fallback: Some(ModelFallbackReason::NoDeterministicId),
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
