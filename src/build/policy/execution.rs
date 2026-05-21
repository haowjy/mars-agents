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
    if let Some(effort) = input.effort_override {
        return resolved_field(Some(effort.to_string()), PolicySource::Cli);
    }
    if let Some(effort) = matched_harness_override.and_then(|entry| entry.effort.as_ref()) {
        return resolved_field(
            Some(effort_level_to_str(effort).to_string()),
            PolicySource::ProfileHarnessOverride,
        );
    }
    if let Some(overlay_effort) = overlay.and_then(|entry| entry.effort.as_deref()) {
        return resolved_field(Some(overlay_effort.to_string()), PolicySource::Overlay);
    }
    if let Some(decision) =
        policy_string_override_by_source(matched_policy, "effort", PolicySource::OverlayModelPolicy)
    {
        return decision;
    }
    if let Some(effort) = input.profile.effort.as_ref() {
        return resolved_field(
            Some(effort_level_to_str(effort).to_string()),
            PolicySource::Profile,
        );
    }
    if let Some(decision) =
        policy_string_override_by_source(matched_policy, "effort", PolicySource::ProfileModelPolicy)
    {
        return decision;
    }
    if let Some(decision) = policy_string_override_by_source(
        matched_policy,
        "effort",
        PolicySource::SettingsModelPolicy,
    ) {
        return decision;
    }
    if let Some(effort) = alias.and_then(|entry| entry.default_effort.clone()) {
        return resolved_field(Some(effort), PolicySource::Alias);
    }
    resolved_field(None, PolicySource::Unset)
}

fn resolve_approval(
    input: &PolicyInput<'_>,
    overlay: Option<&AgentOverlay>,
    matched_policy: Option<&MatchedModelPolicy>,
    matched_harness_override: Option<&OverrideFields>,
) -> ResolvedField<Option<String>> {
    if let Some(approval) = input.approval_override {
        return resolved_field(Some(approval.to_string()), PolicySource::Cli);
    }
    if let Some(approval) = matched_harness_override.and_then(|entry| entry.approval.as_ref()) {
        return resolved_field(
            Some(approval_mode_to_str(approval).to_string()),
            PolicySource::ProfileHarnessOverride,
        );
    }
    if let Some(overlay_approval) = overlay.and_then(|entry| entry.approval.as_deref()) {
        return resolved_field(Some(overlay_approval.to_string()), PolicySource::Overlay);
    }
    if let Some(decision) = policy_string_override_by_source(
        matched_policy,
        "approval",
        PolicySource::OverlayModelPolicy,
    ) {
        return decision;
    }
    if let Some(approval) = input.profile.approval.as_ref() {
        return resolved_field(
            Some(approval_mode_to_str(approval).to_string()),
            PolicySource::Profile,
        );
    }
    if let Some(decision) = policy_string_override_by_source(
        matched_policy,
        "approval",
        PolicySource::ProfileModelPolicy,
    ) {
        return decision;
    }
    if let Some(decision) = policy_string_override_by_source(
        matched_policy,
        "approval",
        PolicySource::SettingsModelPolicy,
    ) {
        return decision;
    }
    resolved_field(None, PolicySource::Unset)
}

fn resolve_sandbox(
    input: &PolicyInput<'_>,
    overlay: Option<&AgentOverlay>,
    matched_policy: Option<&MatchedModelPolicy>,
    matched_harness_override: Option<&OverrideFields>,
) -> ResolvedField<Option<String>> {
    if let Some(sandbox) = input.sandbox_override {
        return resolved_field(Some(sandbox.to_string()), PolicySource::Cli);
    }
    if let Some(sandbox) = matched_harness_override.and_then(|entry| entry.sandbox.as_ref()) {
        return resolved_field(
            Some(sandbox_mode_to_str(sandbox).to_string()),
            PolicySource::ProfileHarnessOverride,
        );
    }
    if let Some(overlay_sandbox) = overlay.and_then(|entry| entry.sandbox.as_deref()) {
        return resolved_field(Some(overlay_sandbox.to_string()), PolicySource::Overlay);
    }
    if let Some(decision) = policy_string_override_by_source(
        matched_policy,
        "sandbox",
        PolicySource::OverlayModelPolicy,
    ) {
        return decision;
    }
    if let Some(sandbox) = input.profile.sandbox.as_ref() {
        return resolved_field(
            Some(sandbox_mode_to_str(sandbox).to_string()),
            PolicySource::Profile,
        );
    }
    if let Some(decision) = policy_string_override_by_source(
        matched_policy,
        "sandbox",
        PolicySource::ProfileModelPolicy,
    ) {
        return decision;
    }
    if let Some(decision) = policy_string_override_by_source(
        matched_policy,
        "sandbox",
        PolicySource::SettingsModelPolicy,
    ) {
        return decision;
    }
    resolved_field(None, PolicySource::Unset)
}

fn resolve_autocompact(
    input: &PolicyInput<'_>,
    alias: Option<&ModelAlias>,
    overlay: Option<&AgentOverlay>,
    matched_policy: Option<&MatchedModelPolicy>,
    matched_harness_override: Option<&OverrideFields>,
) -> ResolvedField<Option<u32>> {
    if let Some(autocompact) = matched_harness_override.and_then(|entry| entry.autocompact) {
        return resolved_field(Some(autocompact), PolicySource::ProfileHarnessOverride);
    }
    if let Some(autocompact) = overlay
        .and_then(|entry| entry.autocompact)
        .and_then(|value| u32::try_from(value).ok())
    {
        return resolved_field(Some(autocompact), PolicySource::Overlay);
    }
    if let Some(decision) = policy_u32_override_by_source(
        matched_policy,
        "autocompact",
        PolicySource::OverlayModelPolicy,
    ) {
        return decision;
    }
    if let Some(autocompact) = input.profile.autocompact {
        return resolved_field(Some(autocompact), PolicySource::Profile);
    }
    if let Some(decision) = policy_u32_override_by_source(
        matched_policy,
        "autocompact",
        PolicySource::ProfileModelPolicy,
    ) {
        return decision;
    }
    if let Some(decision) = policy_u32_override_by_source(
        matched_policy,
        "autocompact",
        PolicySource::SettingsModelPolicy,
    ) {
        return decision;
    }
    if let Some(autocompact) = alias.and_then(|entry| entry.autocompact) {
        return resolved_field(Some(autocompact), PolicySource::Alias);
    }
    resolved_field(None, PolicySource::Unset)
}

fn resolve_autocompact_pct(
    input: &PolicyInput<'_>,
    alias: Option<&ModelAlias>,
    overlay: Option<&AgentOverlay>,
    matched_policy: Option<&MatchedModelPolicy>,
    matched_harness_override: Option<&OverrideFields>,
) -> ResolvedField<Option<u8>> {
    if let Some(autocompact_pct) = matched_harness_override.and_then(|entry| entry.autocompact_pct)
    {
        return resolved_field(Some(autocompact_pct), PolicySource::ProfileHarnessOverride);
    }
    if let Some(autocompact_pct) = overlay
        .and_then(|entry| entry.autocompact_pct)
        .and_then(|value| u8::try_from(value).ok())
        .filter(|value| (1..=100).contains(value))
    {
        return resolved_field(Some(autocompact_pct), PolicySource::Overlay);
    }
    if let Some(decision) = policy_u8_override_by_source(
        matched_policy,
        "autocompact_pct",
        PolicySource::OverlayModelPolicy,
    ) {
        return decision;
    }
    if let Some(autocompact_pct) = input.profile.autocompact_pct {
        return resolved_field(Some(autocompact_pct), PolicySource::Profile);
    }
    if let Some(decision) = policy_u8_override_by_source(
        matched_policy,
        "autocompact_pct",
        PolicySource::ProfileModelPolicy,
    ) {
        return decision;
    }
    if let Some(decision) = policy_u8_override_by_source(
        matched_policy,
        "autocompact_pct",
        PolicySource::SettingsModelPolicy,
    ) {
        return decision;
    }
    if let Some(autocompact_pct) = alias.and_then(|entry| entry.autocompact_pct) {
        return resolved_field(Some(autocompact_pct), PolicySource::Alias);
    }
    resolved_field(None, PolicySource::Unset)
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
