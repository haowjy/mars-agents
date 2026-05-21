use crate::build::policy::{
    MatchedModelPolicy, PolicyInput, PolicyLayer, policy_override_string, policy_override_u8,
    policy_override_u32,
};
use crate::compiler::agents::{ApprovalMode, EffortLevel, OverrideFields, SandboxMode};
use crate::config::AgentOverlay;
use crate::models::ModelAlias;

pub(super) struct ExecutionResolution {
    pub(super) effort: Option<String>,
    pub(super) approval: Option<String>,
    pub(super) sandbox: Option<String>,
    pub(super) autocompact: Option<u32>,
    pub(super) autocompact_pct: Option<u8>,
    pub(super) native_config: Option<serde_json::Map<String, serde_json::Value>>,
    pub(super) effort_source: String,
    pub(super) approval_source: String,
    pub(super) sandbox_source: String,
    pub(super) autocompact_source: String,
    pub(super) autocompact_pct_source: String,
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

    let (effort, effort_source) = resolve_effort(
        input,
        alias,
        overlay,
        matched_policy,
        matched_harness_override,
    );
    let (approval, approval_source) =
        resolve_approval(input, overlay, matched_policy, matched_harness_override);
    let (sandbox, sandbox_source) =
        resolve_sandbox(input, overlay, matched_policy, matched_harness_override);
    let (autocompact, autocompact_source) = resolve_autocompact(
        input,
        alias,
        overlay,
        matched_policy,
        matched_harness_override,
    );
    let (autocompact_pct, autocompact_pct_source) = resolve_autocompact_pct(
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
        effort_source,
        approval_source,
        sandbox_source,
        autocompact_source,
        autocompact_pct_source,
    }
}

fn resolve_effort(
    input: &PolicyInput<'_>,
    alias: Option<&ModelAlias>,
    overlay: Option<&AgentOverlay>,
    matched_policy: Option<&MatchedModelPolicy>,
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
    if let Some(overlay_effort) = overlay.and_then(|entry| entry.effort.as_deref()) {
        return (Some(overlay_effort.to_string()), "overlay".to_string());
    }
    if let Some((value, source)) =
        policy_string_override_by_layer(matched_policy, "effort", PolicyLayer::Overlay)
    {
        return (Some(value), source.to_string());
    }
    if let Some(effort) = input.profile.effort.as_ref() {
        return (
            Some(effort_level_to_str(effort).to_string()),
            "profile".to_string(),
        );
    }
    if let Some((value, source)) =
        policy_string_override_by_layer(matched_policy, "effort", PolicyLayer::Profile)
    {
        return (Some(value), source.to_string());
    }
    if let Some((value, source)) =
        policy_string_override_by_layer(matched_policy, "effort", PolicyLayer::Settings)
    {
        return (Some(value), source.to_string());
    }
    if let Some(effort) = alias.and_then(|entry| entry.default_effort.clone()) {
        return (Some(effort), "alias".to_string());
    }
    (None, "unset".to_string())
}

fn resolve_approval(
    input: &PolicyInput<'_>,
    overlay: Option<&AgentOverlay>,
    matched_policy: Option<&MatchedModelPolicy>,
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
    if let Some(overlay_approval) = overlay.and_then(|entry| entry.approval.as_deref()) {
        return (Some(overlay_approval.to_string()), "overlay".to_string());
    }
    if let Some((value, source)) =
        policy_string_override_by_layer(matched_policy, "approval", PolicyLayer::Overlay)
    {
        return (Some(value), source.to_string());
    }
    if let Some(approval) = input.profile.approval.as_ref() {
        return (
            Some(approval_mode_to_str(approval).to_string()),
            "profile".to_string(),
        );
    }
    if let Some((value, source)) =
        policy_string_override_by_layer(matched_policy, "approval", PolicyLayer::Profile)
    {
        return (Some(value), source.to_string());
    }
    if let Some((value, source)) =
        policy_string_override_by_layer(matched_policy, "approval", PolicyLayer::Settings)
    {
        return (Some(value), source.to_string());
    }
    (None, "unset".to_string())
}

fn resolve_sandbox(
    input: &PolicyInput<'_>,
    overlay: Option<&AgentOverlay>,
    matched_policy: Option<&MatchedModelPolicy>,
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
    if let Some(overlay_sandbox) = overlay.and_then(|entry| entry.sandbox.as_deref()) {
        return (Some(overlay_sandbox.to_string()), "overlay".to_string());
    }
    if let Some((value, source)) =
        policy_string_override_by_layer(matched_policy, "sandbox", PolicyLayer::Overlay)
    {
        return (Some(value), source.to_string());
    }
    if let Some(sandbox) = input.profile.sandbox.as_ref() {
        return (
            Some(sandbox_mode_to_str(sandbox).to_string()),
            "profile".to_string(),
        );
    }
    if let Some((value, source)) =
        policy_string_override_by_layer(matched_policy, "sandbox", PolicyLayer::Profile)
    {
        return (Some(value), source.to_string());
    }
    if let Some((value, source)) =
        policy_string_override_by_layer(matched_policy, "sandbox", PolicyLayer::Settings)
    {
        return (Some(value), source.to_string());
    }
    (None, "unset".to_string())
}

fn resolve_autocompact(
    input: &PolicyInput<'_>,
    alias: Option<&ModelAlias>,
    overlay: Option<&AgentOverlay>,
    matched_policy: Option<&MatchedModelPolicy>,
    matched_harness_override: Option<&OverrideFields>,
) -> (Option<u32>, String) {
    if let Some(autocompact) = matched_harness_override.and_then(|entry| entry.autocompact) {
        return (Some(autocompact), "profile-harness-override".to_string());
    }
    if let Some(autocompact) = overlay
        .and_then(|entry| entry.autocompact)
        .and_then(|value| u32::try_from(value).ok())
    {
        return (Some(autocompact), "overlay".to_string());
    }
    if let Some((value, source)) =
        policy_u32_override_by_layer(matched_policy, "autocompact", PolicyLayer::Overlay)
    {
        return (Some(value), source.to_string());
    }
    if let Some(autocompact) = input.profile.autocompact {
        return (Some(autocompact), "profile".to_string());
    }
    if let Some((value, source)) =
        policy_u32_override_by_layer(matched_policy, "autocompact", PolicyLayer::Profile)
    {
        return (Some(value), source.to_string());
    }
    if let Some((value, source)) =
        policy_u32_override_by_layer(matched_policy, "autocompact", PolicyLayer::Settings)
    {
        return (Some(value), source.to_string());
    }
    if let Some(autocompact) = alias.and_then(|entry| entry.autocompact) {
        return (Some(autocompact), "alias".to_string());
    }
    (None, "unset".to_string())
}

fn resolve_autocompact_pct(
    input: &PolicyInput<'_>,
    alias: Option<&ModelAlias>,
    overlay: Option<&AgentOverlay>,
    matched_policy: Option<&MatchedModelPolicy>,
    matched_harness_override: Option<&OverrideFields>,
) -> (Option<u8>, String) {
    if let Some(autocompact_pct) = matched_harness_override.and_then(|entry| entry.autocompact_pct)
    {
        return (
            Some(autocompact_pct),
            "profile-harness-override".to_string(),
        );
    }
    if let Some(autocompact_pct) = overlay
        .and_then(|entry| entry.autocompact_pct)
        .and_then(|value| u8::try_from(value).ok())
        .filter(|value| (1..=100).contains(value))
    {
        return (Some(autocompact_pct), "overlay".to_string());
    }
    if let Some((value, source)) =
        policy_u8_override_by_layer(matched_policy, "autocompact_pct", PolicyLayer::Overlay)
    {
        return (Some(value), source.to_string());
    }
    if let Some(autocompact_pct) = input.profile.autocompact_pct {
        return (Some(autocompact_pct), "profile".to_string());
    }
    if let Some((value, source)) =
        policy_u8_override_by_layer(matched_policy, "autocompact_pct", PolicyLayer::Profile)
    {
        return (Some(value), source.to_string());
    }
    if let Some((value, source)) =
        policy_u8_override_by_layer(matched_policy, "autocompact_pct", PolicyLayer::Settings)
    {
        return (Some(value), source.to_string());
    }
    if let Some(autocompact_pct) = alias.and_then(|entry| entry.autocompact_pct) {
        return (Some(autocompact_pct), "alias".to_string());
    }
    (None, "unset".to_string())
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

fn policy_string_override_by_layer(
    matched_policy: Option<&MatchedModelPolicy>,
    key: &str,
    layer: PolicyLayer,
) -> Option<(String, &'static str)> {
    let policy = matched_policy.filter(|entry| entry.layer == layer)?;
    let value = policy_override_string(&policy.rule, key)?;
    Some((value, layer.field_source_label()))
}

fn policy_u32_override_by_layer(
    matched_policy: Option<&MatchedModelPolicy>,
    key: &str,
    layer: PolicyLayer,
) -> Option<(u32, &'static str)> {
    let policy = matched_policy.filter(|entry| entry.layer == layer)?;
    let value = policy_override_u32(&policy.rule, key)?;
    Some((value, layer.field_source_label()))
}

fn policy_u8_override_by_layer(
    matched_policy: Option<&MatchedModelPolicy>,
    key: &str,
    layer: PolicyLayer,
) -> Option<(u8, &'static str)> {
    let policy = matched_policy.filter(|entry| entry.layer == layer)?;
    let value = policy_override_u8(&policy.rule, key)?;
    Some((value, layer.field_source_label()))
}
