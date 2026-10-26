use crate::build::bundle::ExecutionNetworkPolicy;
use crate::build::policy::PolicySource::{
    Alias, Cli, Config, Overlay, OverlayModelPolicy, Profile, ProfileModelPolicy,
    SettingsModelPolicy, Unset,
};
use crate::build::policy::{
    MatchedModelPolicy, PolicyInput, PolicySource, ResolvedField, matched_policy_string_override,
};
use crate::config::{AgentOverlay, Settings};
use crate::models::ModelAlias;
use mars_schema::agent::{AgentProfile, ApprovalMode, EffortLevel, SandboxMode};
use mars_schema::network::{NetworkHost, NetworkPolicy};
use mars_schema::permission_set::{PermissionSet, merge_permission_set};

pub(super) struct ExecutionResolution {
    pub(super) effort: ResolvedField<Option<String>>,
    pub(super) approval: ResolvedField<Option<String>>,
    pub(super) sandbox: ResolvedField<Option<String>>,
    pub(super) network: ResolvedNetwork,
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
    let effort = resolve_effort(
        input.effort_override,
        input.profile,
        alias,
        overlay,
        matched_policy,
    );
    let approval = resolve_approval(
        input.approval_override,
        input.profile,
        overlay,
        matched_policy,
    );
    let sandbox = resolve_sandbox(
        input.sandbox_override,
        input.profile,
        overlay,
        matched_policy,
    );
    let network = resolve_network(
        CliNetwork {
            mode: input.network_override,
            allow: input.network_allow,
            deny: input.network_deny,
        },
        input.profile,
        overlay,
        matched_policy,
        settings,
    );
    let autocompact = alias_field(alias.and_then(|entry| entry.autocompact));
    let autocompact_pct = alias_field(alias.and_then(|entry| entry.autocompact_pct));

    ExecutionResolution {
        effort,
        approval,
        sandbox,
        network,
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
    /// `[settings]` defaults: below every model policy, above the alias.
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
        policy_override_by_source(matched_policy, key, ProfileModelPolicy),
        field_layer(Profile, layers.profile),
        policy_override_by_source(matched_policy, key, SettingsModelPolicy),
        field_layer(Config, layers.settings),
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
    let field = policy_string_override_by_source(matched_policy, key, source)?;
    Some(ResolvedField {
        value: Some(NetworkPolicy::parse(field.value.as_deref()?)?),
        source: field.source,
        matched_rule: field.matched_rule,
    })
}

fn alias_field<T>(value: Option<T>) -> ResolvedField<Option<T>> {
    let source = if value.is_some() { Alias } else { Unset };
    resolved_field(value, source)
}

pub(super) fn resolve_effort(
    cli: Option<&str>,
    profile: &AgentProfile,
    alias: Option<&ModelAlias>,
    overlay: Option<&AgentOverlay>,
    matched_policy: Option<&MatchedModelPolicy>,
) -> ResolvedField<Option<String>> {
    resolve_policy_field(
        "effort",
        matched_policy,
        policy_string_override_by_source,
        PolicyFieldLayers {
            cli: cli.map(str::to_string),
            overlay: overlay
                .and_then(|entry| entry.effort.as_ref())
                .map(|level| level.as_str().to_string()),
            profile: profile
                .effort
                .as_ref()
                .map(EffortLevel::as_str)
                .map(str::to_string),
            settings: None,
            alias: alias.and_then(|entry| entry.default_effort.clone()),
        },
    )
}

pub(super) fn resolve_approval(
    cli: Option<&str>,
    profile: &AgentProfile,
    overlay: Option<&AgentOverlay>,
    matched_policy: Option<&MatchedModelPolicy>,
) -> ResolvedField<Option<String>> {
    resolve_policy_field(
        "approval",
        matched_policy,
        policy_string_override_by_source,
        PolicyFieldLayers {
            cli: cli.map(str::to_string),
            overlay: overlay
                .and_then(|entry| entry.approval.as_deref())
                .map(str::to_string),
            profile: profile
                .approval
                .as_ref()
                .map(ApprovalMode::as_str)
                .map(str::to_string),
            settings: None,
            alias: None,
        },
    )
}

pub(super) fn resolve_sandbox(
    cli: Option<&str>,
    profile: &AgentProfile,
    overlay: Option<&AgentOverlay>,
    matched_policy: Option<&MatchedModelPolicy>,
) -> ResolvedField<Option<String>> {
    resolve_policy_field(
        "sandbox",
        matched_policy,
        policy_string_override_by_source,
        PolicyFieldLayers {
            cli: cli.map(str::to_string),
            overlay: overlay
                .and_then(|entry| entry.sandbox.as_deref())
                .map(str::to_string),
            profile: profile
                .sandbox
                .as_ref()
                .map(SandboxMode::as_str)
                .map(str::to_string),
            settings: None,
            alias: None,
        },
    )
}

/// The CLI layer of network policy: `--network`, `--network-allow`, `--network-deny`.
pub(super) struct CliNetwork<'a> {
    pub(super) mode: Option<&'a NetworkPolicy>,
    pub(super) allow: &'a [NetworkHost],
    pub(super) deny: &'a [NetworkHost],
}

pub(super) struct ResolvedNetwork {
    pub(super) mode: ResolvedField<Option<NetworkPolicy>>,
    pub(super) policy: ExecutionNetworkPolicy,
    pub(super) warnings: Vec<String>,
}

/// Network mode follows the shared precedence chain, with `[settings] default-network` below
/// the model policies. Host sets merge in layer order (settings, profile, overlay, CLI) and
/// come out sorted: the project's deny entries are never replaced by a higher layer.
pub(super) fn resolve_network(
    cli: CliNetwork<'_>,
    profile: &AgentProfile,
    overlay: Option<&AgentOverlay>,
    matched_policy: Option<&MatchedModelPolicy>,
    settings: &Settings,
) -> ResolvedNetwork {
    let mut mode = resolve_policy_field(
        "network",
        matched_policy,
        policy_network_override_by_source,
        PolicyFieldLayers {
            cli: cli.mode.cloned(),
            overlay: overlay.and_then(|entry| entry.network.clone()),
            profile: profile.network.clone(),
            settings: settings.default_network.clone(),
            alias: None,
        },
    );
    let mut allow = merge_network_hosts(
        &settings.network_allow,
        &profile.network_allow,
        overlay.map(|entry| &entry.network_allow),
        cli.allow,
    );
    let mut deny = merge_network_hosts(
        &settings.network_deny,
        &profile.network_deny,
        overlay.map(|entry| &entry.network_deny),
        cli.deny,
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
    let policy = ExecutionNetworkPolicy {
        mode: mode.value.clone(),
        allow,
        deny,
    };
    ResolvedNetwork {
        mode,
        policy,
        warnings,
    }
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
