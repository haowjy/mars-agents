use indexmap::IndexMap;

use crate::compiler::agents::HarnessKind;
use crate::compiler::agents::lower::NativeModel;
use crate::diagnostic::DiagnosticCollector;
use crate::harness::registry::HarnessId;
use crate::models::{ModelAlias, ModelSpec};

/// Resolve a package-authored model token without catalogs, caches, or host probes.
pub(super) fn resolve_model(
    agent_name: &str,
    token: Option<&str>,
    harness: HarnessKind,
    native_aliases: &[&str],
    aliases: &IndexMap<String, ModelAlias>,
    diag: &mut DiagnosticCollector,
) -> NativeModel {
    let descriptor = crate::compiler::harness_descriptor::descriptor(harness);
    let harness_name = display_name(harness);
    let Some(token) = token else {
        return NativeModel::Clear;
    };
    let Some(alias) = aliases.get(token) else {
        diag.info(
            "plugin-model-default",
            format!(
                "agent `{agent_name}`: model `{token}` is not a package alias for {harness_name}; using the harness default"
            ),
        );
        return NativeModel::Clear;
    };

    let target = alias_target(alias);
    if let Some(model) = alias.pinned_model_id()
        && target == Some(descriptor.id)
    {
        return NativeModel::Set(model.to_string());
    }
    if native_aliases.contains(&token) && target == Some(descriptor.id) {
        return NativeModel::Set(token.to_string());
    }

    diag.info(
        "plugin-model-default",
        format!(
            "agent `{agent_name}`: model `{token}` has no deterministic {harness_name} id; using the harness default"
        ),
    );
    NativeModel::Clear
}

pub(super) const fn display_name(harness: HarnessKind) -> &'static str {
    match harness {
        HarnessKind::Claude => "Claude",
        HarnessKind::Codex => "Codex",
        HarnessKind::OpenCode => "OpenCode",
        HarnessKind::Cursor => "Cursor",
        HarnessKind::Pi => "Pi",
    }
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
