use crate::build::policy::PolicySource::{
    Alias, Cli, Overlay, OverlayModelPolicy, Profile, ProfileHarnessOverride, ProfileModelPolicy,
    SettingsModelPolicy, Unset,
};
use crate::build::policy::{
    MatchedModelPolicy, PolicyInput, PolicySource, ResolvedField, matched_policy_string_override,
    matched_policy_u8_override, matched_policy_u32_override,
};
use crate::compiler::agents::{ApprovalMode, EffortLevel, OverrideFields, SandboxMode};
use crate::config::AgentOverlay;
use crate::models::ModelAlias;

pub(super) struct ExecutionResolution {
    pub(super) effort: ResolvedField<Option<String>>,
    pub(super) approval: ResolvedField<Option<String>>,
    pub(super) sandbox: ResolvedField<Option<String>>,
    pub(super) autocompact: ResolvedField<Option<u32>>,
    pub(super) autocompact_pct: ResolvedField<Option<u8>>,
    pub(super) native_config: Option<serde_json::Map<String, serde_json::Value>>,
}

pub(super) fn resolve_execution_policy(
    input: &PolicyInput<'_>,
    alias: Option<&ModelAlias>,
    overlay: Option<&AgentOverlay>,
    matched_policy: Option<&MatchedModelPolicy>,
    matched_harness_override: Option<&OverrideFields>,
) -> ExecutionResolution {
    let native_config = matched_harness_override
        .and_then(|fields| fields.native_config.clone())
        .filter(|map| !map.is_empty());

    let effort = resolve_effort(
        input,
        alias,
        overlay,
        matched_policy,
        matched_harness_override,
    );
    let approval = resolve_approval(input, overlay, matched_policy, matched_harness_override);
    let sandbox = resolve_sandbox(input, overlay, matched_policy, matched_harness_override);
    let autocompact = resolve_autocompact(
        input,
        alias,
        overlay,
        matched_policy,
        matched_harness_override,
    );
    let autocompact_pct = resolve_autocompact_pct(
        input,
        alias,
        overlay,
        matched_policy,
        matched_harness_override,
    );

    ExecutionResolution {
        effort,
        approval,
        sandbox,
        autocompact,
        autocompact_pct,
        native_config,
    }
}

fn resolved_field<T>(value: Option<T>, source: PolicySource) -> ResolvedField<Option<T>> {
    ResolvedField {
        value,
        source,
        matched_rule: None,
    }
}

fn field_layer<T>(source: PolicySource, value: Option<T>) -> Option<ResolvedField<Option<T>>> {
    value.map(|value| resolved_field(Some(value), source))
}

struct PolicyFieldLayers<T> {
    cli: Option<T>,
    harness_override: Option<T>,
    overlay: Option<T>,
    profile: Option<T>,
    alias: Option<T>,
}

fn resolve_policy_field<T>(
    key: &str,
    matched_policy: Option<&MatchedModelPolicy>,
    policy_override_by_source: impl Fn(
        Option<&MatchedModelPolicy>,
        &str,
        PolicySource,
    ) -> Option<ResolvedField<Option<T>>>,
    layers: PolicyFieldLayers<T>,
) -> ResolvedField<Option<T>> {
    [
        field_layer(Cli, layers.cli),
        field_layer(ProfileHarnessOverride, layers.harness_override),
        field_layer(Overlay, layers.overlay),
        policy_override_by_source(matched_policy, key, OverlayModelPolicy),
        field_layer(Profile, layers.profile),
        policy_override_by_source(matched_policy, key, ProfileModelPolicy),
        policy_override_by_source(matched_policy, key, SettingsModelPolicy),
        field_layer(Alias, layers.alias),
    ]
    .into_iter()
    .flatten()
    .next()
    .unwrap_or_else(|| resolved_field(None, Unset))
}

fn resolved_policy_field<T>(decision: ResolvedField<T>) -> ResolvedField<Option<T>> {
    ResolvedField {
        value: Some(decision.value),
        source: decision.source,
        matched_rule: decision.matched_rule,
    }
}

fn policy_string_override_by_source(
    matched_policy: Option<&MatchedModelPolicy>,
    key: &str,
    source: PolicySource,
) -> Option<ResolvedField<Option<String>>> {
    matched_policy_string_override(matched_policy, key)
        .filter(|decision| decision.source == source)
        .map(resolved_policy_field)
}

fn policy_u32_override_by_source(
    matched_policy: Option<&MatchedModelPolicy>,
    key: &str,
    source: PolicySource,
) -> Option<ResolvedField<Option<u32>>> {
    matched_policy_u32_override(matched_policy, key)
        .filter(|decision| decision.source == source)
        .map(resolved_policy_field)
}

fn policy_u8_override_by_source(
    matched_policy: Option<&MatchedModelPolicy>,
    key: &str,
    source: PolicySource,
) -> Option<ResolvedField<Option<u8>>> {
    matched_policy_u8_override(matched_policy, key)
        .filter(|decision| decision.source == source)
        .map(resolved_policy_field)
}

fn resolve_effort(
    input: &PolicyInput<'_>,
    alias: Option<&ModelAlias>,
    overlay: Option<&AgentOverlay>,
    matched_policy: Option<&MatchedModelPolicy>,
    matched_harness_override: Option<&OverrideFields>,
) -> ResolvedField<Option<String>> {
    resolve_policy_field(
        "effort",
        matched_policy,
        policy_string_override_by_source,
        PolicyFieldLayers {
            cli: input.effort_override.map(str::to_string),
            harness_override: matched_harness_override
                .and_then(|entry| entry.effort.as_ref())
                .map(effort_level_to_str)
                .map(str::to_string),
            overlay: overlay
                .and_then(|entry| entry.effort.as_deref())
                .map(str::to_string),
            profile: input
                .profile
                .effort
                .as_ref()
                .map(effort_level_to_str)
                .map(str::to_string),
            alias: alias.and_then(|entry| entry.default_effort.clone()),
        },
    )
}

fn resolve_approval(
    input: &PolicyInput<'_>,
    overlay: Option<&AgentOverlay>,
    matched_policy: Option<&MatchedModelPolicy>,
    matched_harness_override: Option<&OverrideFields>,
) -> ResolvedField<Option<String>> {
    resolve_policy_field(
        "approval",
        matched_policy,
        policy_string_override_by_source,
        PolicyFieldLayers {
            cli: input.approval_override.map(str::to_string),
            harness_override: matched_harness_override
                .and_then(|entry| entry.approval.as_ref())
                .map(approval_mode_to_str)
                .map(str::to_string),
            overlay: overlay
                .and_then(|entry| entry.approval.as_deref())
                .map(str::to_string),
            profile: input
                .profile
                .approval
                .as_ref()
                .map(approval_mode_to_str)
                .map(str::to_string),
            alias: None,
        },
    )
}

fn resolve_sandbox(
    input: &PolicyInput<'_>,
    overlay: Option<&AgentOverlay>,
    matched_policy: Option<&MatchedModelPolicy>,
    matched_harness_override: Option<&OverrideFields>,
) -> ResolvedField<Option<String>> {
    resolve_policy_field(
        "sandbox",
        matched_policy,
        policy_string_override_by_source,
        PolicyFieldLayers {
            cli: input.sandbox_override.map(str::to_string),
            harness_override: matched_harness_override
                .and_then(|entry| entry.sandbox.as_ref())
                .map(sandbox_mode_to_str)
                .map(str::to_string),
            overlay: overlay
                .and_then(|entry| entry.sandbox.as_deref())
                .map(str::to_string),
            profile: input
                .profile
                .sandbox
                .as_ref()
                .map(sandbox_mode_to_str)
                .map(str::to_string),
            alias: None,
        },
    )
}

fn resolve_autocompact(
    input: &PolicyInput<'_>,
    alias: Option<&ModelAlias>,
    overlay: Option<&AgentOverlay>,
    matched_policy: Option<&MatchedModelPolicy>,
    matched_harness_override: Option<&OverrideFields>,
) -> ResolvedField<Option<u32>> {
    resolve_policy_field(
        "autocompact",
        matched_policy,
        policy_u32_override_by_source,
        PolicyFieldLayers {
            cli: None,
            harness_override: matched_harness_override.and_then(|entry| entry.autocompact),
            overlay: overlay
                .and_then(|entry| entry.autocompact)
                .and_then(|value| u32::try_from(value).ok()),
            profile: input.profile.autocompact,
            alias: alias.and_then(|entry| entry.autocompact),
        },
    )
}

fn resolve_autocompact_pct(
    input: &PolicyInput<'_>,
    alias: Option<&ModelAlias>,
    overlay: Option<&AgentOverlay>,
    matched_policy: Option<&MatchedModelPolicy>,
    matched_harness_override: Option<&OverrideFields>,
) -> ResolvedField<Option<u8>> {
    resolve_policy_field(
        "autocompact_pct",
        matched_policy,
        policy_u8_override_by_source,
        PolicyFieldLayers {
            cli: None,
            harness_override: matched_harness_override.and_then(|entry| entry.autocompact_pct),
            overlay: overlay
                .and_then(|entry| entry.autocompact_pct)
                .and_then(|value| u8::try_from(value).ok())
                .filter(|value| (1..=100).contains(value)),
            profile: input.profile.autocompact_pct,
            alias: alias.and_then(|entry| entry.autocompact_pct),
        },
    )
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
