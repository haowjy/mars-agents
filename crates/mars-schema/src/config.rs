//! What `mars.toml` holds beyond `[package]` and the dependency tables: `[settings]`,
//! `[models]`, `[agents]`, `[skills]` and `[plugin]`, read as the CLI reads them, and the reader
//! the project's other config files share.
//!
//! One serde pass reads a whole file; a key it leaves unread becomes one `field.unknown` note and
//! stays in the file. Nothing here names a key that was once read (see `retired`).

use indexmap::IndexMap;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_ignored::Path as IgnoredPath;

use crate::alias::ModelAlias;
use crate::dependency::RawEntry;
use crate::diagnostic::{PathSegment, SchemaDiagnostic, unknown_field};
use crate::keys;
use crate::model_policy::ModelPolicyRule;
use crate::names::SourceName;
use crate::network::{NetworkHost, NetworkPolicy};
use crate::package::PackageMetadata as PackageInfo;
use crate::permission_set::PermissionSet;
use crate::tool_rule_map::ToolRuleMap;

/// Top-level mars.toml configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Config {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<PackageInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin: Option<toml::Value>,
    #[serde(default)]
    pub dependencies: IndexMap<SourceName, RawEntry>,
    /// Local-only dependencies — installed when syncing this repo but NOT
    /// exported to consumers via manifest. Use for dev tooling, prompt
    /// authoring helpers, etc.
    #[serde(
        default,
        skip_serializing_if = "IndexMap::is_empty",
        rename = "local-dependencies"
    )]
    pub local_dependencies: IndexMap<SourceName, RawEntry>,
    #[serde(default)]
    pub settings: Settings,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub models: IndexMap<String, ModelAlias>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub agents: IndexMap<String, AgentOverlay>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub skills: IndexMap<String, SkillOverlay>,
    /// A note for each key of the file this Mars did not read, set when the file is read.
    #[serde(skip)]
    pub unknown_keys: Vec<SchemaDiagnostic>,
    /// The Mars the file's tool rules need, set when the file is read.
    #[serde(skip)]
    pub floor: Option<keys::Floor>,
}

/// `approval` and `sandbox` in an overlay take the values an Agent file does, and a bad one
/// fails where it is written.
fn overlay_value<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
    key: &str,
    valid: fn(&str) -> bool,
    allowed: &str,
) -> Result<Option<String>, D::Error> {
    let value = Option::<String>::deserialize(deserializer)?;
    match value {
        Some(value) if !valid(&value) => Err(serde::de::Error::custom(format!(
            "`{key}` has invalid value `{value}`; allowed: {allowed}"
        ))),
        value => Ok(value),
    }
}

/// `effort` in an overlay takes the values an Agent file does; TOML has no null, so the
/// harness default is `auto`.
fn overlay_effort<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<crate::agent::EffortRequest>, D::Error> {
    use crate::agent::EffortRequest;
    let Some(value) = Option::<String>::deserialize(deserializer)? else {
        return Ok(None);
    };
    if let Some(request) = EffortRequest::parse(&value) {
        return Ok(Some(request));
    }
    Err(serde::de::Error::custom(format!(
        "`effort` has invalid value `{value}`; allowed: {}",
        EffortRequest::ALLOWED
    )))
}

fn overlay_approval<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    use crate::agent::ApprovalMode;
    overlay_value(
        deserializer,
        "approval",
        |value| ApprovalMode::parse(value).is_some(),
        ApprovalMode::ALLOWED,
    )
}

fn overlay_sandbox<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    use crate::agent::SandboxMode;
    overlay_value(
        deserializer,
        "sandbox",
        |value| SandboxMode::parse(value).is_some(),
        SandboxMode::ALLOWED,
    )
}

/// Per-agent launch-bundle overlay policy in mars.toml `[agents.<name>]`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AgentOverlay {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(
        default,
        deserialize_with = "overlay_effort",
        skip_serializing_if = "Option::is_none"
    )]
    pub effort: Option<crate::agent::EffortRequest>,
    #[serde(
        default,
        deserialize_with = "overlay_approval",
        skip_serializing_if = "Option::is_none"
    )]
    pub approval: Option<String>,
    #[serde(
        default,
        deserialize_with = "overlay_sandbox",
        skip_serializing_if = "Option::is_none"
    )]
    pub sandbox: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<NetworkPolicy>,
    #[serde(
        default,
        rename = "network-allow",
        alias = "network_allow",
        skip_serializing_if = "PermissionSet::is_noop"
    )]
    pub network_allow: PermissionSet<NetworkHost>,
    #[serde(
        default,
        rename = "network-deny",
        alias = "network_deny",
        skip_serializing_if = "PermissionSet::is_noop"
    )]
    pub network_deny: PermissionSet<NetworkHost>,
    #[serde(
        default,
        rename = "model-invocable",
        alias = "model_invocable",
        skip_serializing_if = "Option::is_none"
    )]
    pub model_invocable: Option<bool>,
    #[serde(
        default,
        rename = "user-invocable",
        alias = "user_invocable",
        skip_serializing_if = "Option::is_none"
    )]
    pub user_invocable: Option<bool>,
    #[serde(default, skip_serializing_if = "ToolRuleMap::is_empty")]
    pub tools: ToolRuleMap,
    #[serde(
        default,
        rename = "model-policies",
        skip_serializing_if = "Option::is_none"
    )]
    pub model_policies: Option<Vec<ModelPolicyRule>>,
}

/// Per-skill overlay policy in mars.toml `[skills.<name>]`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SkillOverlay {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(
        default,
        rename = "model-invocable",
        alias = "model_invocable",
        skip_serializing_if = "Option::is_none"
    )]
    pub model_invocable: Option<bool>,
    #[serde(
        default,
        rename = "user-invocable",
        alias = "user_invocable",
        skip_serializing_if = "Option::is_none"
    )]
    pub user_invocable: Option<bool>,
}

impl SkillOverlay {
    pub fn is_empty(&self) -> bool {
        self.description.is_none()
            && self.model_invocable.is_none()
            && self.user_invocable.is_none()
    }
}

/// Global settings — extensible via additional fields.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Settings {
    /// Custom managed output directory (e.g. ".claude").
    ///
    /// When unset, mars no longer creates a generic `.agents` target by default;
    /// `.mars/` is the canonical compiled store and native emission is handled
    /// by target-specific compiler paths.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_root: Option<String>,
    /// Managed target directories materialized from .mars/ canonical store.
    /// When set, only listed targets are populated. When unset, `managed_root`
    /// is used for backwards compatibility; otherwise no target-sync targets
    /// are enabled by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub targets: Option<Vec<String>>,
    /// Unset means `DEFAULT_MODELS_CACHE_TTL_HOURS`; read it through `models_cache_ttl_hours()`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models_cache_ttl_hours: Option<u32>,
    /// Minimum mars binary version required to use this project.
    /// Old binary + new package with this set → compatibility error.
    /// New binary + old package without this set → succeeds with defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_mars_version: Option<String>,
    /// Default harness for launch routing when profile/alias/provider cannot resolve one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_harness: Option<String>,
    /// Project-wide default model token when no CLI override or profile model is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    /// Project-wide network mode when no higher policy layer sets one.
    #[serde(
        default,
        alias = "default-network",
        skip_serializing_if = "Option::is_none"
    )]
    pub default_network: Option<NetworkPolicy>,
    #[serde(
        default,
        rename = "network-allow",
        alias = "network_allow",
        skip_serializing_if = "PermissionSet::is_noop"
    )]
    pub network_allow: PermissionSet<NetworkHost>,
    #[serde(
        default,
        rename = "network-deny",
        alias = "network_deny",
        skip_serializing_if = "PermissionSet::is_noop"
    )]
    pub network_deny: PermissionSet<NetworkHost>,
    /// Tool rules for every agent and for a launch with no agent. Takes `allow`, `ask` and `deny`; never `*`.
    #[serde(default, skip_serializing_if = "ToolRuleMap::is_empty")]
    pub tools: ToolRuleMap,
    /// Ordered harness preference for launch-bundle candidate selection.
    ///
    /// When set, replaces built-in provider preference ordering for candidate
    /// selection. First installed candidate wins. When unset, Mars uses the
    /// built-in default (`claude`, `pi`, `codex`, `opencode`, `cursor`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_order: Option<Vec<String>>,
    /// Ordered provider preference for model-first routing tie-breaks.
    ///
    /// Optional soft preference used only after model-name matching. Empty means
    /// preserve harness-reported model order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_order: Option<Vec<String>>,
    /// models.dev provider keys ingested into the models cache.
    ///
    /// Unset uses the built-in default (`anthropic`, `openai`, `google`, `meta`,
    /// `deepseek`, `xai`, `openrouter`). Set replaces that list. `["*"]` ingests
    /// every provider. Catalog-only: a pinned alias still works if the harness
    /// can resolve the provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_providers: Option<Vec<String>>,
    /// Controls whether harness-bound agents are emitted to native harness dirs.
    ///
    /// `auto` (the default when unset) emits for standalone mars syncs and
    /// suppresses native agent artifacts when Meridian invokes mars with
    /// `MERIDIAN_MANAGED=1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_emission: Option<AgentEmission>,
    #[serde(default, skip_serializing_if = "MeridianSettings::is_empty")]
    pub meridian: MeridianSettings,
    #[serde(
        default,
        rename = "model-policies",
        skip_serializing_if = "Option::is_none"
    )]
    pub model_policies: Option<Vec<ModelPolicyRule>>,
}

/// Selective native agent emission under managed mode or `agent_emission = "never"`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AgentCopyConfig {
    #[serde(default)]
    pub harnesses: Vec<String>,
    #[serde(default)]
    pub include_fanout: bool,
}

/// Fanout agent list for native emission qualification and `mars agents` routing output.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct FanoutConfig {
    #[serde(default)]
    pub agents: Vec<String>,
}

/// Meridian-managed settings nested under `[settings.meridian]`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MeridianSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_copy: Option<AgentCopyConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fanout: Option<FanoutConfig>,
}

impl MeridianSettings {
    pub fn is_empty(&self) -> bool {
        self.agent_copy.is_none() && self.fanout.is_none()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum AgentEmission {
    Auto,
    Always,
    Never,
}

const DEFAULT_MODELS_CACHE_TTL_HOURS: u32 = 24;

impl Settings {
    pub fn models_cache_ttl_hours(&self) -> u32 {
        self.models_cache_ttl_hours
            .unwrap_or(DEFAULT_MODELS_CACHE_TTL_HOURS)
    }

    pub fn model_policies(&self) -> &[ModelPolicyRule] {
        self.model_policies.as_deref().unwrap_or_default()
    }

    /// Selective native agent copy config from `[settings.meridian.agent_copy]`.
    pub fn meridian_agent_copy(&self) -> Option<&AgentCopyConfig> {
        self.meridian.agent_copy.as_ref()
    }

    /// The network keys a project sets in `[settings]` (or inherits from the user config), each
    /// with whether it restricts: `default_network = "allow"` is a grant.
    pub fn network_keys(&self) -> Vec<(&'static str, bool)> {
        [
            (
                "default-network",
                self.default_network
                    .as_ref()
                    .map(|policy| *policy != crate::network::NetworkPolicy::Allow),
            ),
            (
                "network-allow",
                (!self.network_allow.entries().is_empty()).then_some(true),
            ),
            (
                "network-deny",
                (!self.network_deny.entries().is_empty()).then_some(true),
            ),
        ]
        .into_iter()
        .filter_map(|(key, restricts)| restricts.map(|restricts| (key, restricts)))
        .collect()
    }

    /// Fanout agent list from `[settings.meridian.fanout]`.
    pub fn meridian_fanout_agents(&self) -> &[String] {
        self.meridian
            .fanout
            .as_ref()
            .map(|f| f.agents.as_slice())
            .unwrap_or(&[])
    }
}

impl Config {
    /// One note for each key of a dependency entry that Mars did not read.
    pub fn dependency_unknown_keys(&self) -> Vec<SchemaDiagnostic> {
        [
            ("dependencies", &self.dependencies),
            ("local-dependencies", &self.local_dependencies),
        ]
        .into_iter()
        .flat_map(|(table, dependencies)| {
            dependencies.iter().flat_map(move |(name, entry)| {
                entry.unknown.keys().map(move |key| {
                    let path = vec![
                        PathSegment::Key(table.to_owned()),
                        PathSegment::Key(name.to_string()),
                        PathSegment::Key(key.clone()),
                    ];
                    unknown_field(path, &format!("{table}.{name}"), key)
                })
            })
        })
        .collect()
    }
}

/// A file read: the value, one `Info` for each key Mars did not read, and the Mars its tool
/// rules need.
pub struct Read<T> {
    pub value: T,
    pub unknown_keys: Vec<SchemaDiagnostic>,
    pub floor: Option<keys::Floor>,
}

/// Read a `mars.toml`-shaped file. `Err` is the TOML or serde error as the parser words it.
pub fn read<T: DeserializeOwned>(content: &str) -> Result<Read<T>, toml::de::Error> {
    let mut ignored = Vec::new();
    let value = serde_ignored::deserialize(toml::Deserializer::new(content), |at| {
        ignored.push(segments(&at));
    })?;

    let mut unknown_keys = Vec::new();
    for mut at in ignored {
        let Some(PathSegment::Key(key)) = at.pop() else {
            continue;
        };
        // `[package]` and `[plugin]` belong to the package readers, which report their own.
        if matches!(at.first(), Some(PathSegment::Key(table)) if table == "package" || table == "plugin")
        {
            continue;
        }
        let table = table_name(&at);
        let mut path = at;
        path.push(PathSegment::Key(key.clone()));
        unknown_keys.push(unknown_field(path, &table, &key));
    }
    let floor = content
        .parse::<toml::Table>()
        .ok()
        .and_then(|table| keys::floor(keys::used(&table, keys::Document::Consumer)));
    Ok(Read {
        value,
        unknown_keys,
        floor,
    })
}

/// Only the `[models]` table of a manifest: a dependency's other consumer tables belong to its
/// own project, not to its dependents.
pub fn read_models(content: &str) -> Result<IndexMap<String, ModelAlias>, toml::de::Error> {
    #[derive(Deserialize)]
    struct Models {
        #[serde(default)]
        models: IndexMap<String, ModelAlias>,
    }
    toml::from_str::<Models>(content).map(|parsed| parsed.models)
}

/// `[settings.tools]` takes no `*`: it would turn every agent into an allowlist or sit ahead of
/// every profile's `approval`, which already is the default for calls no rule matches.
pub fn settings_tools_refusal(tools: Option<&ToolRuleMap>) -> Option<String> {
    tools?.wildcard_refusal(
        "[settings.tools]",
        "`approval` is the default for calls no rule matches",
    )
}

/// A project's `min_mars_version`, read as `requires-mars` is: a bare version means that version
/// or newer, and a range is accepted. One that does not parse would let an older Mars through
/// the one key whose job is to stop it, so the file does not load.
pub fn parse_min_mars_version(raw: &str) -> Result<semver::VersionReq, String> {
    crate::package::parse_requirement(raw).map_err(|error| {
        format!("`min_mars_version = \"{raw}\"` is not a version requirement: {error}")
    })
}

fn segments(at: &IgnoredPath<'_>) -> Vec<PathSegment> {
    match at {
        IgnoredPath::Root => Vec::new(),
        IgnoredPath::Seq { parent, index } => {
            let mut out = segments(parent);
            out.push(PathSegment::Index(*index));
            out
        }
        IgnoredPath::Map { parent, key } => {
            let mut out = segments(parent);
            out.push(PathSegment::Key(key.clone()));
            out
        }
        IgnoredPath::Some { parent }
        | IgnoredPath::NewtypeStruct { parent }
        | IgnoredPath::NewtypeVariant { parent } => segments(parent),
    }
}

/// `agents.a.model-policies[0]`: the dotted table, with list positions in brackets.
fn table_name(at: &[PathSegment]) -> String {
    let mut name = String::new();
    for segment in at {
        match segment {
            PathSegment::Key(key) => {
                if !name.is_empty() {
                    name.push('.');
                }
                name.push_str(key);
            }
            PathSegment::Index(index) => name.push_str(&format!("[{index}]")),
        }
    }
    name
}
