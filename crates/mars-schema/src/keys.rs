//! The table of config keys: one row per key concept, whichever files it appears in.
//!
//! A row says which Mars first read the key (`since`), whether a value of it restricts, and
//! whether it is current, an alias, deprecated or removed. Readers ask the table instead of
//! keeping their own list of removed keys or version floors, so a new key is one row here.

/// Where a key appears.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    /// Agent frontmatter.
    AgentProfile,
    /// Skill frontmatter.
    SkillProfile,
    /// `[package]` in `mars.toml`.
    Package,
    /// A top-level table of a package's `mars.toml`, such as `[plugin]`.
    Manifest,
    /// `hook.toml`.
    HookToml,
    /// `[settings]` in `mars.toml`, `mars.local.toml` and the user config.
    Settings,
    /// `[agents.<name>]` in the consumer config files.
    AgentOverlay,
    /// `[agents.<name>.tools]` in the consumer config files.
    AgentOverlayTools,
    /// `[settings.meridian.agent_copy]` in the consumer config files.
    AgentCopy,
    /// `[skills.<name>]` in the consumer config files.
    SkillOverlay,
    /// The tables of the user `config.toml` outside `[settings]`.
    UserConfig,
    /// `[settings]` in the user `config.toml`.
    UserSettings,
}

/// Whether a value of the key narrows what an agent may do.
#[derive(Debug, Clone, Copy)]
pub enum Restricts {
    Never,
    Always,
    /// Decided by the value, rendered as text.
    OnValue(fn(&str) -> bool),
}

/// What Mars does to a removed key when it rewrites a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rewrite {
    /// Left for the author, or rewritten by code that needs more than the table says.
    None,
    /// The key is deleted.
    Drop,
    /// The row's `value` of the key becomes this one.
    Value(&'static str),
    /// The key is spelled as this instead; its value is unchanged.
    Rename(&'static str),
}

#[derive(Debug, Clone, Copy)]
pub struct Removal {
    /// What the reader tells the author.
    pub advice: &'static str,
    pub rewrite: Rewrite,
    /// More advice that depends on what the author wrote under the key.
    pub detail: Option<fn(&toml::Value) -> String>,
}

#[derive(Debug, Clone, Copy)]
pub enum Status {
    Current,
    /// Read as the named key.
    Alias(&'static str),
    /// Still read; the advice names its replacement.
    Deprecated(&'static str),
    Removed(Removal),
    /// Not read on this surface because it belongs in the project's files. Ignoring it loses no
    /// restriction (`restricts` is `Never`), so the reader warns and drops it.
    Elsewhere(&'static str),
}

#[derive(Debug, Clone, Copy)]
pub struct KeyRow {
    pub key: &'static str,
    /// Set when the row is one value of `key` (`approval = "yolo"`), not the whole key.
    pub value: Option<&'static str>,
    pub surfaces: &'static [Surface],
    /// The first Mars release that reads the key, for keys an older Mars reads wrongly.
    pub since: Option<&'static str>,
    pub restricts: Restricts,
    pub status: Status,
}

use Surface::*;

const fn removed(
    key: &'static str,
    surfaces: &'static [Surface],
    advice: &'static str,
    rewrite: Rewrite,
) -> KeyRow {
    KeyRow {
        key,
        value: None,
        surfaces,
        since: None,
        restricts: Restricts::Never,
        status: Status::Removed(Removal {
            advice,
            rewrite,
            detail: None,
        }),
    }
}

const fn removed_value(
    key: &'static str,
    value: &'static str,
    replacement: &'static str,
) -> KeyRow {
    KeyRow {
        value: Some(value),
        ..removed(key, &[AgentProfile], "", Rewrite::Value(replacement))
    }
}

const fn elsewhere(
    key: &'static str,
    surfaces: &'static [Surface],
    advice: &'static str,
) -> KeyRow {
    KeyRow {
        key,
        value: None,
        surfaces,
        since: None,
        restricts: Restricts::Never,
        status: Status::Elsewhere(advice),
    }
}

const fn with_detail(mut row: KeyRow, detail: fn(&toml::Value) -> String) -> KeyRow {
    if let Status::Removed(removal) = &mut row.status {
        removal.detail = Some(detail);
    }
    row
}

/// A key a Mars release gates: a package that uses it needs `requires-mars` at or above `since`.
const fn gated(key: &'static str, surfaces: &'static [Surface], since: &'static str) -> KeyRow {
    KeyRow {
        key,
        value: None,
        surfaces,
        since: Some(since),
        restricts: Restricts::Always,
        status: Status::Current,
    }
}

const DISCARDED_SKILL_POLICY: &str = "remove this unused skill policy";

/// Within a surface, table order is the order readers report and rewrite in.
pub static KEYS: &[KeyRow] = &[
    // Agent keys only pre-v1 Mars defined: a definition read as any dialect loses them.
    removed(
        "fanout",
        &[AgentProfile],
        "use consumer `[settings.meridian.fanout]`",
        Rewrite::Drop,
    ),
    removed(
        "models",
        &[AgentProfile],
        "use `model-policies`",
        Rewrite::Drop,
    ),
    removed(
        "mcp-tools",
        &[AgentProfile],
        "put MCP capability references in `tools`",
        Rewrite::Drop,
    ),
    removed(
        "autocompact",
        &[AgentProfile, AgentOverlay],
        "configure compaction on the model alias",
        Rewrite::Drop,
    ),
    removed(
        "autocompact_pct",
        &[AgentProfile, AgentOverlay],
        "configure compaction on the model alias",
        Rewrite::Drop,
    ),
    // Agent and skill keys the legacy upgrade renames or moves, or leaves for the author.
    removed(
        "disallowed-tools",
        &[AgentProfile],
        "use `tools: {<capability>: deny}`",
        Rewrite::None,
    ),
    removed(
        "model_invocable",
        &[AgentProfile, SkillProfile, AgentOverlay, SkillOverlay],
        "use `model-invocable`",
        Rewrite::Rename("model-invocable"),
    ),
    removed(
        "user_invocable",
        &[AgentProfile, SkillProfile, AgentOverlay, SkillOverlay],
        "use `user-invocable`",
        Rewrite::Rename("user-invocable"),
    ),
    removed(
        "network_allow",
        &[AgentProfile, AgentOverlay, Settings],
        "use `network-allow`",
        Rewrite::Rename("network-allow"),
    ),
    removed(
        "network_deny",
        &[AgentProfile, AgentOverlay, Settings],
        "use `network-deny`",
        Rewrite::Rename("network-deny"),
    ),
    removed(
        "permission",
        &[AgentProfile],
        "move it under `harness-overrides.meridian-flow.permission`",
        Rewrite::None,
    ),
    removed(
        "resident-rearm-budget",
        &[AgentProfile],
        "move it under `harness-overrides.meridian-cli.resident-rearm-budget`",
        Rewrite::None,
    ),
    removed(
        "meridian-capabilities",
        &[AgentProfile],
        "move it under `harness-overrides.meridian-cli.capabilities`",
        Rewrite::None,
    ),
    removed_value("approval", "yolo", "never"),
    removed_value("effort", "adaptive", "medium"),
    removed_value("effort", "disabled", "none"),
    // Schema v1 skills carry no tool policy, so every spelling of one is dropped.
    removed(
        "tools",
        &[SkillProfile, SkillOverlay],
        DISCARDED_SKILL_POLICY,
        Rewrite::Drop,
    ),
    removed(
        "allowed-tools",
        &[SkillProfile],
        DISCARDED_SKILL_POLICY,
        Rewrite::Drop,
    ),
    removed(
        "allowed_tools",
        &[SkillProfile],
        DISCARDED_SKILL_POLICY,
        Rewrite::Drop,
    ),
    removed(
        "disallowed-tools",
        &[SkillProfile],
        DISCARDED_SKILL_POLICY,
        Rewrite::Drop,
    ),
    removed(
        "disallowed_tools",
        &[SkillProfile],
        DISCARDED_SKILL_POLICY,
        Rewrite::Drop,
    ),
    removed(
        "disallowedTools",
        &[SkillProfile],
        DISCARDED_SKILL_POLICY,
        Rewrite::Drop,
    ),
    removed(
        "type",
        &[SkillProfile],
        "use `metadata.type`",
        Rewrite::None,
    ),
    removed(
        "when_to_use",
        &[SkillProfile],
        "fold it into `description`",
        Rewrite::None,
    ),
    removed(
        "invocation",
        &[SkillProfile],
        "use `model-invocable` and `user-invocable`",
        Rewrite::None,
    ),
    removed(
        "disable-model-invocation",
        &[SkillProfile],
        "use `model-invocable`",
        Rewrite::None,
    ),
    removed(
        "allow_implicit_invocation",
        &[SkillProfile],
        "use `model-invocable`",
        Rewrite::None,
    ),
    // `[package]`.
    removed("visibility", &[Package], "remove it", Rewrite::Drop),
    // Docs once showed `[package.bootstrap] path = ...`; nothing reads it.
    removed(
        "bootstrap",
        &[Package],
        "Mars finds bootstrap docs in `bootstrap/<name>/BOOTSTRAP.md` — move the doc there and remove the field",
        Rewrite::None,
    ),
    // Consumer config.
    removed(
        "mcp",
        &[AgentOverlayTools],
        "write an MCP grant as a rule: `\"mcp(server)\" = \"allow\"`",
        Rewrite::None,
    ),
    // The lists `mars migrate` rewrites into rule entries; what it cannot rewrite it says.
    removed(
        "allowed",
        &[AgentOverlayTools],
        "write a rule per tool: `\"<tool>\" = \"allow\"`; `mars migrate` rewrites the lists",
        Rewrite::None,
    ),
    removed(
        "disallowed",
        &[AgentOverlayTools],
        "write a rule per tool: `\"<tool>\" = \"deny\"`; `mars migrate` rewrites the lists",
        Rewrite::None,
    ),
    removed(
        "fanout_agents",
        &[AgentCopy],
        "use `[settings.meridian.fanout].agents`",
        Rewrite::None,
    ),
    // Keys of the project's files that a user-wide `config.toml` would apply to every project.
    elsewhere(
        "managed_root",
        &[UserSettings],
        "put it in the project's mars.toml",
    ),
    elsewhere(
        "targets",
        &[UserSettings],
        "put it in the project's mars.toml",
    ),
    elsewhere(
        "overrides",
        &[UserConfig],
        "put it in the project's mars.local.toml, since a user-wide override would point every project at a local checkout unseen",
    ),
    with_detail(
        removed(
            "model_visibility",
            &[Settings],
            "move these display rules to `mars.curated.toml` (or `mars.curated.local.toml` for local overrides) after reviewing the harness×model semantics; Mars does not write that file, and the nearest entries are:",
            Rewrite::None,
        ),
        crate::model_visibility::migration,
    ),
    // Keys a Mars release gates: an older Mars reads a package that uses one wrongly.
    gated("schema", &[Package], "0.20.0"),
    gated("uses-providers", &[Package], "0.20.0"),
    gated("plugin", &[Manifest], "0.19.0"),
    gated("hooks", &[HookToml], "0.25.0"),
    gated("tools", &[Package], "0.30.0"),
];

fn on(row: &KeyRow, surface: Surface) -> bool {
    row.surfaces.contains(&surface)
}

/// Every removal on `surface`, in table order.
pub fn removals(surface: Surface) -> impl Iterator<Item = (&'static KeyRow, &'static Removal)> {
    KEYS.iter().filter_map(move |row| match &row.status {
        Status::Removed(removal) if on(row, surface) => Some((row, removal)),
        _ => None,
    })
}

/// The row that removes the whole of `key` on `surface`.
pub fn removed_key(surface: Surface, key: &str) -> Option<(&'static KeyRow, &'static Removal)> {
    removals(surface).find(|(row, _)| row.value.is_none() && row.key == key)
}

/// The row that removes `value` of `key` on `surface`.
pub fn removed_value_of(
    surface: Surface,
    key: &str,
    value: &str,
) -> Option<(&'static KeyRow, &'static Removal)> {
    removals(surface).find(|(row, _)| row.key == key && row.value == Some(value))
}

/// The advice for `key` when `surface` does not hold it and ignoring it loses nothing.
pub fn elsewhere_key(surface: Surface, key: &str) -> Option<&'static str> {
    KEYS.iter().find_map(|row| match row.status {
        Status::Elsewhere(advice) if row.key == key && on(row, surface) => Some(advice),
        _ => None,
    })
}

/// The keys `surface` deletes outright.
pub fn dropped_keys(surface: Surface) -> impl Iterator<Item = &'static str> {
    removals(surface)
        .filter(|(row, removal)| row.value.is_none() && removal.rewrite == Rewrite::Drop)
        .map(|(row, _)| row.key)
}

/// The first Mars that reads `key` on `surface`.
///
/// # Panics
/// When the table has no gated row for it: a call site names a floor it never added.
pub fn since(surface: Surface, key: &str) -> &'static str {
    KEYS.iter()
        .find(|row| row.key == key && on(row, surface))
        .and_then(|row| row.since)
        .unwrap_or_else(|| panic!("no `since` row for `{key}`"))
}
