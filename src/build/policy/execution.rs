use crate::build::bundle::ExecutionNetworkPolicy;
use crate::build::policy::PolicySource::{
    Alias, Cli, Overlay, OverlayModelPolicy, Profile, ProfileModelPolicy, SettingsModelPolicy,
    Unset,
};
use crate::build::policy::{
    MatchedModelPolicy, PolicyInput, PolicySource, ResolvedField, matched_policy_network_override,
    matched_policy_string_override, matched_policy_u8_override, matched_policy_u32_override,
};
use crate::compiler::agents::{ApprovalMode, EffortLevel, NetworkPolicy, SandboxMode};
use crate::config::{AgentOverlay, NetworkHost, Settings};
use crate::models::ModelAlias;
use crate::permission_set::{PermissionSet, merge_permission_set};

pub(super) struct ExecutionResolution {
    pub(super) effort: ResolvedField<Option<String>>,
    pub(super) approval: ResolvedField<Option<String>>,
    pub(super) sandbox: ResolvedField<Option<String>>,
    pub(super) network: ResolvedField<Option<NetworkPolicy>>,
    pub(super) resolved_network: ExecutionNetworkPolicy,
    pub(super) warnings: Vec<String>,
    pub(super) autocompact: ResolvedField<Option<u32>>,
    pub(super) autocompact_pct: ResolvedField<Option<u8>>,
    pub(super) native_config: Option<serde_json::Map<String, serde_json::Value>>,
}

pub(super) fn resolve_execution_policy(
    input: &PolicyInput<'_>,
    alias: Option<&ModelAlias>,
    overlay: Option<&AgentOverlay>,
    matched_policy: Option<&MatchedModelPolicy>,
    settings: &Settings,
    native_config: Option<serde_json::Map<String, serde_json::Value>>,
) -> ExecutionResolution {
    let effort = resolve_effort(input, alias, overlay, matched_policy);
    let approval = resolve_approval(input, overlay, matched_policy);
    let sandbox = resolve_sandbox(input, overlay, matched_policy);
    let (network, resolved_network, warnings) =
        resolve_network(input, overlay, matched_policy, settings);
    let autocompact = resolve_autocompact(input, alias, overlay, matched_policy);
    let autocompact_pct = resolve_autocompact_pct(input, alias, overlay, matched_policy);

    ExecutionResolution {
        effort,
        approval,
        sandbox,
        network,
        resolved_network,
        warnings,
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
    overlay: Option<T>,
    profile: Option<T>,
    settings: Option<T>,
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
        field_layer(Overlay, layers.overlay),
        policy_override_by_source(matched_policy, key, OverlayModelPolicy),
        field_layer(Profile, layers.profile),
        policy_override_by_source(matched_policy, key, ProfileModelPolicy),
        policy_override_by_source(matched_policy, key, SettingsModelPolicy),
        field_layer(PolicySource::Config, layers.settings),
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

fn policy_network_override_by_source(
    matched_policy: Option<&MatchedModelPolicy>,
    key: &str,
    source: PolicySource,
) -> Option<ResolvedField<Option<NetworkPolicy>>> {
    matched_policy_network_override(matched_policy, key)
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
) -> ResolvedField<Option<String>> {
    resolve_policy_field(
        "effort",
        matched_policy,
        policy_string_override_by_source,
        PolicyFieldLayers {
            cli: input.effort_override.map(str::to_string),
            overlay: overlay
                .and_then(|entry| entry.effort.as_deref())
                .map(str::to_string),
            profile: input
                .profile
                .effort
                .as_ref()
                .map(effort_level_to_str)
                .map(str::to_string),
            settings: None,
            alias: alias.and_then(|entry| entry.default_effort.clone()),
        },
    )
}

fn resolve_approval(
    input: &PolicyInput<'_>,
    overlay: Option<&AgentOverlay>,
    matched_policy: Option<&MatchedModelPolicy>,
) -> ResolvedField<Option<String>> {
    resolve_policy_field(
        "approval",
        matched_policy,
        policy_string_override_by_source,
        PolicyFieldLayers {
            cli: input.approval_override.map(str::to_string),
            overlay: overlay
                .and_then(|entry| entry.approval.as_deref())
                .map(str::to_string),
            profile: input
                .profile
                .approval
                .as_ref()
                .map(approval_mode_to_str)
                .map(str::to_string),
            settings: None,
            alias: None,
        },
    )
}

fn resolve_sandbox(
    input: &PolicyInput<'_>,
    overlay: Option<&AgentOverlay>,
    matched_policy: Option<&MatchedModelPolicy>,
) -> ResolvedField<Option<String>> {
    resolve_policy_field(
        "sandbox",
        matched_policy,
        policy_string_override_by_source,
        PolicyFieldLayers {
            cli: input.sandbox_override.map(str::to_string),
            overlay: overlay
                .and_then(|entry| entry.sandbox.as_deref())
                .map(str::to_string),
            profile: input
                .profile
                .sandbox
                .as_ref()
                .map(sandbox_mode_to_str)
                .map(str::to_string),
            settings: None,
            alias: None,
        },
    )
}

fn resolve_network(
    input: &PolicyInput<'_>,
    overlay: Option<&AgentOverlay>,
    matched_policy: Option<&MatchedModelPolicy>,
    settings: &Settings,
) -> (
    ResolvedField<Option<NetworkPolicy>>,
    ExecutionNetworkPolicy,
    Vec<String>,
) {
    let mut mode = resolve_policy_field(
        "network",
        matched_policy,
        policy_network_override_by_source,
        PolicyFieldLayers {
            cli: input.network_override.cloned(),
            overlay: overlay.and_then(|entry| entry.network.clone()),
            profile: input.profile.network.clone(),
            settings: settings.default_network.clone(),
            alias: None,
        },
    );
    let mut allow = merge_network_hosts(
        &settings.network_allow,
        &input.profile.network_allow,
        overlay.map(|entry| &entry.network_allow),
        input.network_allow,
    );
    let mut deny = merge_network_hosts(
        &settings.network_deny,
        &input.profile.network_deny,
        overlay.map(|entry| &entry.network_deny),
        input.network_deny,
    );
    let mut warnings = Vec::new();
    if mode.value == Some(NetworkPolicy::Allowlist) && allow.is_empty() {
        mode.value = Some(NetworkPolicy::Deny);
        warnings.push(
            "`network: allowlist` resolved with no allowed hosts; resolved to `deny`".to_string(),
        );
    }
    match mode.value {
        Some(NetworkPolicy::Deny) => {
            allow.clear();
            deny.clear();
        }
        Some(NetworkPolicy::Allow) => allow.clear(),
        Some(NetworkPolicy::Allowlist) | None => {}
    }
    let resolved = ExecutionNetworkPolicy {
        mode: mode.value.clone(),
        allow,
        deny,
    };
    (mode, resolved, warnings)
}

fn merge_network_hosts(
    settings: &PermissionSet<NetworkHost>,
    profile: &PermissionSet<NetworkHost>,
    overlay: Option<&PermissionSet<NetworkHost>>,
    cli: &[NetworkHost],
) -> Vec<NetworkHost> {
    let mut merged = Vec::new();
    for layer in [Some(settings), Some(profile), overlay]
        .into_iter()
        .flatten()
    {
        merge_permission_set(&mut merged, layer);
    }
    merge_permission_set(&mut merged, &PermissionSet::Add(cli.to_vec()));
    merged.sort();
    merged
}

fn resolve_autocompact(
    input: &PolicyInput<'_>,
    alias: Option<&ModelAlias>,
    overlay: Option<&AgentOverlay>,
    matched_policy: Option<&MatchedModelPolicy>,
) -> ResolvedField<Option<u32>> {
    resolve_policy_field(
        "autocompact",
        matched_policy,
        policy_u32_override_by_source,
        PolicyFieldLayers {
            cli: None,
            overlay: overlay
                .and_then(|entry| entry.autocompact)
                .and_then(|value| u32::try_from(value).ok()),
            profile: input.profile.autocompact,
            settings: None,
            alias: alias.and_then(|entry| entry.autocompact),
        },
    )
}

fn resolve_autocompact_pct(
    input: &PolicyInput<'_>,
    alias: Option<&ModelAlias>,
    overlay: Option<&AgentOverlay>,
    matched_policy: Option<&MatchedModelPolicy>,
) -> ResolvedField<Option<u8>> {
    resolve_policy_field(
        "autocompact_pct",
        matched_policy,
        policy_u8_override_by_source,
        PolicyFieldLayers {
            cli: None,
            overlay: overlay
                .and_then(|entry| entry.autocompact_pct)
                .and_then(|value| u8::try_from(value).ok())
                .filter(|value| (1..=100).contains(value)),
            profile: input.profile.autocompact_pct,
            settings: None,
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
        ApprovalMode::Never => "never",
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
