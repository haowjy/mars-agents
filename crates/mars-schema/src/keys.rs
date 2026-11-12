//! The tables of config keys: what Mars removed, what it gates by release, and what belongs in a
//! project's files.
//!
//! A row says on which surfaces (files and tables) it holds. Readers ask the tables instead of
//! keeping their own list of removed keys or version floors, so a new key is one row here. A
//! file's version floor is [`floor`] over the keys [`used`] finds in it, plus the uses a
//! caller derives from other files (a hook, a profile's tool rule).

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

/// A key (or one value of a key) Mars no longer reads. It is an error on every surface the row
/// lists.
#[derive(Debug, Clone, Copy)]
pub struct Removed {
    pub key: &'static str,
    /// Set when the row is one value of `key` (`approval = "yolo"`), not the whole key.
    pub value: Option<&'static str>,
    pub surfaces: &'static [Surface],
    /// What the reader tells the author.
    pub advice: &'static str,
    pub rewrite: Rewrite,
    /// More advice that depends on what the author wrote under the key.
    pub detail: Option<fn(&toml::Value) -> String>,
}

/// A key that belongs in a project's files. The user config warns about it and drops it:
/// ignoring it loses no restriction.
#[derive(Debug, Clone, Copy)]
pub struct Elsewhere {
    pub key: &'static str,
    pub surfaces: &'static [Surface],
    pub advice: &'static str,
}

/// When a file that writes a gated key needs the row's `since`.
#[derive(Debug, Clone, Copy)]
pub enum Gate {
    /// The key's presence: an older Mars misreads how the package reads (`schema`, `plugin`).
    Present,
    /// The key holds tool rules, and the rule's action decides: an older Mars drops the rule,
    /// so a restricting one (`deny`, `ask`) must not meet one, and an `allow` loses only a grant.
    Rules(fn(&str) -> bool),
}

/// A key a Mars release gates: a file that uses it needs a floor at or above `since`.
#[derive(Debug, Clone, Copy)]
pub struct Gated {
    pub key: &'static str,
    pub surfaces: &'static [Surface],
    /// The first Mars release that reads the key. Mars before it ignores the key or misreads
    /// it; the row's comment says which.
    pub since: Release,
    pub gate: Gate,
}

use Surface::*;

const fn removed(
    key: &'static str,
    surfaces: &'static [Surface],
    advice: &'static str,
    rewrite: Rewrite,
) -> Removed {
    Removed {
        key,
        value: None,
        surfaces,
        advice,
        rewrite,
        detail: None,
    }
}

const fn removed_value(
    key: &'static str,
    value: &'static str,
    replacement: &'static str,
) -> Removed {
    Removed {
        value: Some(value),
        ..removed(
            key,
            &[AgentProfile, AgentOverlay],
            "",
            Rewrite::Value(replacement),
        )
    }
}

const fn with_detail(mut row: Removed, detail: fn(&toml::Value) -> String) -> Removed {
    row.detail = Some(detail);
    row
}

fn denies_or_asks(action: &str) -> bool {
    matches!(action, "deny" | "ask")
}

const DISCARDED_SKILL_POLICY: &str = "remove this unused skill policy";

/// Within a surface, table order is the order readers report and rewrite in.
pub static REMOVED: &[Removed] = &[
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
        "run `mars migrate`, which rewrites the lists as rules without widening any agent",
        Rewrite::None,
    ),
    removed(
        "disallowed",
        &[AgentOverlayTools],
        "run `mars migrate`, which rewrites the lists as rules without widening any agent",
        Rewrite::None,
    ),
    removed(
        "fanout_agents",
        &[AgentCopy],
        "use `[settings.meridian.fanout].agents`",
        Rewrite::None,
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
];

pub static ELSEWHERE: &[Elsewhere] = &[
    // Keys of the project's files that a user-wide `config.toml` would apply to every project.
    Elsewhere {
        key: "managed_root",
        surfaces: &[UserSettings],
        advice: "put it in the project's mars.toml",
    },
    Elsewhere {
        key: "targets",
        surfaces: &[UserSettings],
        advice: "put it in the project's mars.toml",
    },
    Elsewhere {
        key: "overrides",
        surfaces: &[UserConfig],
        advice: "put it in the project's mars.local.toml, since a user-wide override would point every project at a local checkout unseen",
    },
];

/// The first Mars that honours `min_mars_version`. A floor at or below it protects nothing a
/// `min_mars_version` line could: Mars before it ignores the line.
pub const MIN_MARS_VERSION_SINCE: Release = Release(0, 30, 0);

/// Keys a Mars release gates; the comment on each says what an older Mars does with it.
pub static GATED: &[Gated] = &[
    // Mars before 0.20.0 reads a `schema = 1` package as pre-v1 and drops what it cannot place.
    Gated {
        key: "schema",
        surfaces: &[Package],
        since: Release(0, 20, 0),
        gate: Gate::Present,
    },
    // Mars before 0.20.0 fails `skill__provider` refs with `skill not found`.
    Gated {
        key: "uses-providers",
        surfaces: &[Package],
        since: Release(0, 20, 0),
        gate: Gate::Present,
    },
    // Mars before 0.19.0 drops `[plugin]` when `mars version` rewrites the manifest.
    Gated {
        key: "plugin",
        surfaces: &[Manifest],
        since: Release(0, 19, 0),
        gate: Gate::Present,
    },
    // Mars before 0.25.0 rejects `[hooks]` and fails the whole sync.
    Gated {
        key: "hooks",
        surfaces: &[HookToml],
        since: Release(0, 25, 0),
        gate: Gate::Present,
    },
    // Tool rule tables. Mars before 0.29.0 ignores them without a word; 0.29.x reads `deny`
    // under the older precedence, where a narrower allow can beat it, and fails on `allow` or
    // `ask`; 0.30.0 is the first where the strictest rule wins. `allow` is not a restriction:
    // a Mars that drops one loses a grant only.
    Gated {
        key: "tools",
        surfaces: &[Package, Settings, AgentOverlay, AgentProfile],
        since: Release(0, 30, 0),
        gate: Gate::Rules(denies_or_asks),
    },
];

/// A key a file uses, as [`floor`] reads it.
pub struct Used {
    pub surface: Surface,
    pub key: String,
    /// The rule's action, for a key whose gate depends on it.
    pub value: Option<String>,
    /// Where the file uses it, for the message: "`[package.tools]` `bash` = \"deny\"".
    pub at: String,
}

impl Used {
    /// A portable hook in a `hook.toml`, which an older Mars rejects.
    pub fn hook(name: &str) -> Self {
        Self {
            surface: HookToml,
            key: "hooks".into(),
            value: None,
            at: format!("hook `{name}` using `[hooks]`"),
        }
    }

    /// A tool rule in an agent's frontmatter `tools`.
    pub fn profile_rule(agent: &str, rule: &str, action: &str) -> Self {
        Self {
            surface: AgentProfile,
            key: "tools".into(),
            value: Some(action.into()),
            at: format!("agent `{agent}` `tools` `{rule}` = \"{action}\""),
        }
    }
}

/// Which file [`used`] reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Document {
    /// A package's `mars.toml`.
    Package,
    /// `mars.toml`, `mars.local.toml` or the user `config.toml` as a project's configuration.
    Consumer,
}

/// The gated keys `document` uses: every key of a table that has a gated row on that table's
/// surface, and for a key that holds rules, each rule.
pub fn used(document: &toml::Table, kind: Document) -> Vec<Used> {
    let mut used = Vec::new();
    let mut scan = |surface: Surface, label: String, table: &toml::Table| {
        for row in GATED.iter().filter(|row| on(row.surfaces, surface)) {
            let Some(value) = table.get(row.key) else {
                continue;
            };
            match (row.gate, value) {
                (Gate::Rules(_), toml::Value::Table(rules)) => {
                    let table = if label.is_empty() {
                        row.key.to_string()
                    } else {
                        format!("{label}.{}", row.key)
                    };
                    for (name, action) in rules {
                        let Some(action) = action.as_str() else {
                            continue;
                        };
                        used.push(Used {
                            surface,
                            key: row.key.into(),
                            value: Some(action.into()),
                            at: format!("`[{table}]` `{name}` = \"{action}\""),
                        });
                    }
                }
                (Gate::Rules(_), _) => {}
                (Gate::Present, value) => used.push(Used {
                    surface,
                    key: row.key.into(),
                    value: None,
                    at: match value {
                        toml::Value::Table(_) => format!("`[{}]`", row.key),
                        toml::Value::Integer(_) | toml::Value::Boolean(_) => {
                            format!("`{} = {value}`", row.key)
                        }
                        _ => format!("`{}`", row.key),
                    },
                }),
            }
        }
    };
    match kind {
        Document::Package => {
            scan(Manifest, String::new(), document);
            if let Some(package) = document.get("package").and_then(toml::Value::as_table) {
                scan(Package, "package".into(), package);
            }
        }
        Document::Consumer => {
            if let Some(settings) = document.get("settings").and_then(toml::Value::as_table) {
                scan(Settings, "settings".into(), settings);
            }
            let agents = document.get("agents").and_then(toml::Value::as_table);
            for (name, agent) in agents.into_iter().flatten() {
                if let Some(agent) = agent.as_table() {
                    scan(AgentOverlay, format!("agents.{name}"), agent);
                }
            }
        }
    }
    used
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
pub fn floor(used: impl IntoIterator<Item = Used>) -> Option<Floor> {
    let mut floor: Option<Floor> = None;
    for used in used {
        let Some(row) = GATED
            .iter()
            .find(|row| row.key == used.key && on(row.surfaces, used.surface))
        else {
            continue;
        };
        let holds = match row.gate {
            Gate::Present => true,
            Gate::Rules(restricts) => used.value.as_deref().is_some_and(restricts),
        };
        if holds && floor.as_ref().is_none_or(|floor| row.since > floor.since) {
            floor = Some(Floor {
                since: row.since,
                at: used.at,
            });
        }
    }
    floor
}

fn on(surfaces: &[Surface], surface: Surface) -> bool {
    surfaces.contains(&surface)
}

/// Every removal on `surface`, in table order.
pub fn removals(surface: Surface) -> impl Iterator<Item = &'static Removed> {
    REMOVED.iter().filter(move |row| on(row.surfaces, surface))
}

/// The row that removes the whole of `key` on `surface`.
pub fn removed_key(surface: Surface, key: &str) -> Option<&'static Removed> {
    removals(surface).find(|row| row.value.is_none() && row.key == key)
}

/// The row that removes `value` of `key` on `surface`.
pub fn removed_value_of(surface: Surface, key: &str, value: &str) -> Option<&'static Removed> {
    removals(surface).find(|row| row.key == key && row.value == Some(value))
}

/// The advice for `key` when `surface` does not hold it and ignoring it loses nothing.
pub fn elsewhere_key(surface: Surface, key: &str) -> Option<&'static str> {
    ELSEWHERE
        .iter()
        .find(|row| row.key == key && on(row.surfaces, surface))
        .map(|row| row.advice)
}

/// The keys `surface` deletes outright.
pub fn dropped_keys(surface: Surface) -> impl Iterator<Item = &'static str> {
    removals(surface)
        .filter(|row| row.value.is_none() && row.rewrite == Rewrite::Drop)
        .map(|row| row.key)
}
