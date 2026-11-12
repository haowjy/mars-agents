//! The table of config keys: one row per key concept, whichever files it appears in.
//!
//! A row says which Mars first read the key (`since`), whether a value of it restricts, and
//! whether it is current, an alias, deprecated or removed. Readers ask the table instead of
//! keeping their own list of removed keys or version floors, so a new key is one row here.
//! A file's version floor is [`floor`] over the keys it uses.

use std::fmt;

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

/// A Mars release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Release(pub u64, pub u64, pub u64);

impl fmt::Display for Release {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

impl Release {
    pub fn version(self) -> semver::Version {
        semver::Version::new(self.0, self.1, self.2)
    }
}

/// Whether a file that writes the key needs a Mars of at least the row's `since`. For a key
/// that narrows what an agent may do, it is the value that decides: an older Mars drops the
/// key, so a `deny` must not meet one, and an `allow` loses only a grant. For a key that
/// changes how a package reads (`schema`, `plugin`), it is `Always`.
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
    /// The first Mars release that reads the key, for keys an older Mars reads wrongly or
    /// drops. Mars before that release ignores the key or misreads it; a file that uses it
    /// states the release as its floor.
    pub since: Option<Release>,
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

/// A key a Mars release gates: a file that uses it needs a floor at or above `since`.
const fn gated(
    key: &'static str,
    surfaces: &'static [Surface],
    since: Release,
    restricts: Restricts,
) -> KeyRow {
    KeyRow {
        key,
        value: None,
        surfaces,
        since: Some(since),
        restricts,
        status: Status::Current,
    }
}

fn denies_or_asks(action: &str) -> bool {
    matches!(action, "deny" | "ask")
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
    // Keys a Mars release gates; the comment on each says what an older Mars does with it.
    // Mars before 0.20.0 reads a `schema = 1` package as pre-v1 and drops what it cannot place.
    gated("schema", &[Package], Release(0, 20, 0), Restricts::Always),
    // Mars before 0.20.0 fails `skill__provider` refs with `skill not found`.
    gated(
        "uses-providers",
        &[Package],
        Release(0, 20, 0),
        Restricts::Always,
    ),
    // Mars before 0.19.0 drops `[plugin]` when `mars version` rewrites the manifest.
    gated("plugin", &[Manifest], Release(0, 19, 0), Restricts::Always),
    // Mars before 0.25.0 rejects `[hooks]` and fails the whole sync.
    gated("hooks", &[HookToml], Release(0, 25, 0), Restricts::Always),
    // Tool rule tables. Mars before 0.29.0 ignores them without a word; 0.29.x reads `deny`
    // under the older precedence, where a narrower allow can beat it, and fails on `allow` or
    // `ask`; 0.30.0 is the first where the strictest rule wins. `allow` is not a restriction:
    // a Mars that drops one loses a grant only.
    gated(
        "tools",
        &[Package, Settings, AgentOverlay],
        Release(0, 30, 0),
        Restricts::OnValue(denies_or_asks),
    ),
];

/// A key a file uses, as [`floor`] reads it.
pub struct Used<'a> {
    pub surface: Surface,
    pub key: &'a str,
    /// The value written, for a key whose `restricts` depends on it.
    pub value: Option<&'a str>,
    /// Where the file uses it, for the message: "`[package.tools]` `bash` = \"deny\"".
    pub at: String,
}

/// The Mars a file needs, and what in it asks for that.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Floor {
    pub since: Release,
    pub at: String,
}

impl Floor {
    /// The requirement that states it: `>=0.30.0`.
    pub fn requirement(&self) -> String {
        format!(">={}", self.since)
    }
}

/// The highest `since` among the keys used whose row says an older Mars would misread them.
/// The first of equal ones names the floor.
pub fn floor<'a>(used: impl IntoIterator<Item = Used<'a>>) -> Option<Floor> {
    let mut floor: Option<Floor> = None;
    for used in used {
        let Some(row) = KEYS.iter().find(|row| {
            row.key == used.key && on(row, used.surface) && matches!(row.status, Status::Current)
        }) else {
            continue;
        };
        let Some(since) = row.since else { continue };
        let holds = match row.restricts {
            Restricts::Never => false,
            Restricts::Always => true,
            Restricts::OnValue(decides) => used.value.is_some_and(decides),
        };
        if holds && floor.as_ref().is_none_or(|floor| since > floor.since) {
            floor = Some(Floor { since, at: used.at });
        }
    }
    floor
}

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
