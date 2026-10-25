use indexmap::IndexMap;

use crate::compiler::agents::lower::NativeModel;
use crate::diagnostic::DiagnosticCollector;
use crate::harness::registry::HarnessId;
use crate::models::{ModelAlias, ModelSpec};

const CLAUDE_NATIVE_ALIASES: &[&str] = &["opus", "sonnet", "haiku"];

/// Resolve a package-authored model token without catalogs, caches, or host probes.
pub(super) fn resolve_claude_model(
    agent_name: &str,
    token: Option<&str>,
    aliases: &IndexMap<String, ModelAlias>,
    diag: &mut DiagnosticCollector,
) -> NativeModel {
    let Some(token) = token else {
        return NativeModel::Clear;
    };
    let Some(alias) = aliases.get(token) else {
        diag.info(
            "plugin-model-default",
            format!(
                "agent `{agent_name}`: model `{token}` is not a package alias for Claude; using the harness default"
            ),
        );
        return NativeModel::Clear;
    };

    let target = alias_target(alias);
    if let Some(model) = alias.pinned_model_id()
        && target == Some(HarnessId::Claude)
    {
        return NativeModel::Set(model.to_string());
    }
    if CLAUDE_NATIVE_ALIASES.contains(&token) && target == Some(HarnessId::Claude) {
        return NativeModel::Set(token.to_string());
    }

    diag.info(
        "plugin-model-default",
        format!(
            "agent `{agent_name}`: model `{token}` has no deterministic Claude id; using the harness default"
        ),
    );
    NativeModel::Clear
}

fn alias_target(alias: &ModelAlias) -> Option<HarnessId> {
    if let Some(harness) = alias.harness.as_deref() {
        return crate::harness::registry::parse(harness);
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
