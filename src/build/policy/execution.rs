use crate::build::policy::PolicyInput;
use crate::compiler::agents::{ApprovalMode, EffortLevel, OverrideFields, SandboxMode};
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
    matched_harness_override: Option<&OverrideFields>,
) -> ExecutionResolution {
    let native_config = matched_harness_override
        .and_then(|fields| fields.native_config.clone())
        .filter(|map| !map.is_empty());

    let (effort, effort_source) = resolve_effort(input, alias, matched_harness_override);
    let (approval, approval_source) = resolve_approval(input, matched_harness_override);
    let (sandbox, sandbox_source) = resolve_sandbox(input, matched_harness_override);
    let (autocompact, autocompact_source) =
        resolve_autocompact(input, alias, matched_harness_override);
    let (autocompact_pct, autocompact_pct_source) =
        resolve_autocompact_pct(input, alias, matched_harness_override);

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
    if let Some(effort) = input.profile.effort.as_ref() {
        return (
            Some(effort_level_to_str(effort).to_string()),
            "profile".to_string(),
        );
    }
    if let Some(effort) = alias.and_then(|entry| entry.default_effort.clone()) {
        return (Some(effort), "alias".to_string());
    }
    (None, "unset".to_string())
}

fn resolve_approval(
    input: &PolicyInput<'_>,
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
    if let Some(approval) = input.profile.approval.as_ref() {
        return (
            Some(approval_mode_to_str(approval).to_string()),
            "profile".to_string(),
        );
    }
    (None, "unset".to_string())
}

fn resolve_sandbox(
    input: &PolicyInput<'_>,
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
    if let Some(sandbox) = input.profile.sandbox.as_ref() {
        return (
            Some(sandbox_mode_to_str(sandbox).to_string()),
            "profile".to_string(),
        );
    }
    (None, "unset".to_string())
}

fn resolve_autocompact(
    input: &PolicyInput<'_>,
    alias: Option<&ModelAlias>,
    matched_harness_override: Option<&OverrideFields>,
) -> (Option<u32>, String) {
    if let Some(autocompact) = matched_harness_override.and_then(|entry| entry.autocompact) {
        return (Some(autocompact), "profile-harness-override".to_string());
    }
    if let Some(autocompact) = input.profile.autocompact {
        return (Some(autocompact), "profile".to_string());
    }
    if let Some(autocompact) = alias.and_then(|entry| entry.autocompact) {
        return (Some(autocompact), "alias".to_string());
    }
    (None, "unset".to_string())
}

fn resolve_autocompact_pct(
    input: &PolicyInput<'_>,
    alias: Option<&ModelAlias>,
    matched_harness_override: Option<&OverrideFields>,
) -> (Option<u8>, String) {
    if let Some(autocompact_pct) = matched_harness_override.and_then(|entry| entry.autocompact_pct)
    {
        return (
            Some(autocompact_pct),
            "profile-harness-override".to_string(),
        );
    }
    if let Some(autocompact_pct) = input.profile.autocompact_pct {
        return (Some(autocompact_pct), "profile".to_string());
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
