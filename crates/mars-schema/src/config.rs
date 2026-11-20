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
    /// The v0.36 spelling of one target. Retired: nothing reads it as a target. It is a field only
    /// so a warning can name the `targets` to write instead (`retired::managed_root_note`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_root: Option<String>,
    /// Managed target directories materialized from .mars/ canonical store.
    /// When set, only listed targets are populated. When unset, no target-sync
    /// targets are enabled by default.
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
    /// built-in default (`claude`, `pi`, `codex`, `opencode`).
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
pub fn read<T: DeserializeOwned>(
    content: &str,
    document: keys::Document,
) -> Result<Read<T>, toml::de::Error> {
    let mut ignored = Vec::new();
    let value = serde_ignored::deserialize(toml::Deserializer::new(content), |at| {
        ignored.push(segments(&at));
    })?;
    let mut unknown_keys = unknown_notes(ignored, &[]);
    let table = content.parse::<toml::Table>().ok();
    unknown_keys.extend(table.iter().flat_map(crate::retired::config_notes));
    let floor = table.and_then(|table| keys::floor(keys::used(&table, document)));
    Ok(Read {
        value,
        unknown_keys,
        floor,
    })
}

/// One `field.unknown` note for each key a serde pass left unread, `prefix` being where the
/// pass started.
fn unknown_notes(ignored: Vec<Vec<PathSegment>>, prefix: &[PathSegment]) -> Vec<SchemaDiagnostic> {
    let mut notes = Vec::new();
    for at in ignored {
        let mut at: Vec<PathSegment> = prefix.iter().cloned().chain(at).collect();
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
        notes.push(unknown_field(path, &table, &key));
    }
    notes
}

/// A table of a `mars.toml` this Mars refused, and why. `path` names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigIssue {
    pub path: Vec<PathSegment>,
    pub message: String,
}

/// A `mars.toml` read for its consumer configuration.
pub struct ConsumerRead {
    /// Every table that is valid on its own. A table the file breaks is absent (`models.a` of
    /// `models`, or the whole of `settings`); `package` and the dependency tables come from the
    /// whole-file pass only when it succeeds.
    pub config: Config,
    /// What was refused: a table that failed, `[settings.tools]` with a `*`, a
    /// `min_mars_version` that is not a requirement.
    pub issues: Vec<ConfigIssue>,
    /// The error of the whole-file pass, when it failed. Its text points at the line, and the
    /// CLI, which refuses the project's own file whole, reports it as it always has.
    pub parse_error: Option<toml::de::Error>,
}

/// Read a project's `mars.toml`: the one reading the CLI's `parse_config` and the WASM
/// `read('mars.toml')` share. The whole file is read in one serde pass; when that fails, each
/// table is read apart, so one broken value costs its own table and nothing else.
pub fn read_consumer(content: &str) -> ConsumerRead {
    match read::<Config>(content, keys::Document::Consumer) {
        Ok(read) => {
            let mut config = read.value;
            config.unknown_keys = read.unknown_keys;
            config.unknown_keys.extend(config.dependency_unknown_keys());
            config.floor = read.floor;
            let issues = settings_refusals(
                Some(&config.settings.tools),
                config.settings.min_mars_version.as_deref(),
            );
            ConsumerRead {
                config,
                issues,
                parse_error: None,
            }
        }
        Err(error) => {
            let (config, issues) = read_tables(content, &error);
            ConsumerRead {
                config,
                issues,
                parse_error: Some(error),
            }
        }
    }
}

fn key_path(parts: &[&str]) -> Vec<PathSegment> {
    parts
        .iter()
        .map(|part| PathSegment::Key((*part).to_owned()))
        .collect()
}

/// The tables of `content` that are valid alone. `package` and `[plugin]` are the package
/// readers' and the dependency entries are only read for their unknown keys: their refusals are
/// reported where the package and its dependencies are read.
fn read_tables(content: &str, whole: &toml::de::Error) -> (Config, Vec<ConfigIssue>) {
    let mut config = Config::default();
    let Ok(table) = content.parse::<toml::Table>() else {
        let issue = ConfigIssue {
            path: Vec::new(),
            message: whole.to_string(),
        };
        return (config, vec![issue]);
    };
    let mut issues = Vec::new();
    let mut unknown = Vec::new();
    config.floor = keys::floor(keys::used(&table, keys::Document::Consumer));
    config.plugin = table.get("plugin").cloned();
    unknown.extend(crate::retired::config_notes(&table));
    if let Some(value) = table.get("settings") {
        match read_value::<Settings>(value, key_path(&["settings"]), &mut unknown) {
            Ok(settings) => {
                issues.extend(settings_refusals(
                    Some(&settings.tools),
                    settings.min_mars_version.as_deref(),
                ));
                config.settings = settings;
            }
            Err(issue) => issues.push(issue),
        }
    }
    config.models = read_entries(&table, "models", &mut unknown, &mut issues);
    config.agents = read_entries(&table, "agents", &mut unknown, &mut issues);
    config.skills = read_entries(&table, "skills", &mut unknown, &mut issues);
    config.dependencies = read_dependency_entries(&table, "dependencies", &mut unknown);
    config.local_dependencies = read_dependency_entries(&table, "local-dependencies", &mut unknown);
    config.unknown_keys = unknown;
    config.unknown_keys.extend(config.dependency_unknown_keys());
    (config, issues)
}

fn read_value<T: DeserializeOwned>(
    value: &toml::Value,
    at: Vec<PathSegment>,
    unknown: &mut Vec<SchemaDiagnostic>,
) -> Result<T, ConfigIssue> {
    let mut ignored = Vec::new();
    match serde_ignored::deserialize(value.clone(), |path| ignored.push(segments(&path))) {
        Ok(read) => {
            unknown.extend(unknown_notes(ignored, &at));
            Ok(read)
        }
        Err(error) => Err(ConfigIssue {
            path: at,
            message: error.to_string().trim_end().to_owned(),
        }),
    }
}

/// The entries of a table of named entries (`[models.<alias>]`), each read alone.
fn read_entries<T: DeserializeOwned>(
    table: &toml::Table,
    key: &str,
    unknown: &mut Vec<SchemaDiagnostic>,
    issues: &mut Vec<ConfigIssue>,
) -> IndexMap<String, T> {
    let mut entries = IndexMap::new();
    match table.get(key) {
        None => {}
        Some(toml::Value::Table(named)) => {
            for (name, value) in named {
                match read_value(value, key_path(&[key, name]), unknown) {
                    Ok(entry) => {
                        entries.insert(name.clone(), entry);
                    }
                    Err(issue) => issues.push(issue),
                }
            }
        }
        Some(_) => issues.push(ConfigIssue {
            path: key_path(&[key]),
            message: format!("`{key}` must be a table"),
        }),
    }
    entries
}

fn read_dependency_entries(
    table: &toml::Table,
    key: &str,
    unknown: &mut Vec<SchemaDiagnostic>,
) -> IndexMap<SourceName, RawEntry> {
    let mut entries = IndexMap::new();
    let Some(toml::Value::Table(named)) = table.get(key) else {
        return entries;
    };
    for (name, value) in named {
        let Ok(source) = toml::Value::String(name.clone()).try_into::<SourceName>() else {
            continue;
        };
        if let Ok(entry) = read_value(value, key_path(&[key, name]), unknown) {
            entries.insert(source, entry);
        }
    }
    entries
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

/// What a file's `[settings]` refuses. `[settings.tools]` takes no `*`: it would turn every agent
/// into an allowlist or sit ahead of every profile's `approval`, which already is the default for
/// calls no rule matches. A `min_mars_version` that is not a requirement is refused because the
/// key's job is to stop an older Mars (see [`parse_min_mars_version`]).
pub fn settings_refusals(
    tools: Option<&ToolRuleMap>,
    min_mars_version: Option<&str>,
) -> Vec<ConfigIssue> {
    let tools = tools
        .and_then(|tools| {
            tools.wildcard_refusal(
                "[settings.tools]",
                "`approval` is the default for calls no rule matches",
            )
        })
        .map(|message| ConfigIssue {
            path: key_path(&["settings", "tools"]),
            message,
        });
    let min = min_mars_version
        .and_then(|raw| parse_min_mars_version(raw).err())
        .map(|message| ConfigIssue {
            path: key_path(&["settings", "min_mars_version"]),
            message,
        });
    tools.into_iter().chain(min).collect()
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
