//! The keys a Mars release gates: a file that uses one needs a floor at or above the release that
//! first read it. This lists keys Mars reads today and when they arrived, nothing about keys it
//! no longer reads. A file's version floor is [`floor`] over the keys [`used`] finds in it, plus
//! the uses a caller derives from other files (a hook, a profile's tool rule).

use std::fmt;

/// Where a gated key appears.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    /// Agent frontmatter.
    AgentProfile,
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

fn denies_or_asks(action: &str) -> bool {
    matches!(action, "deny" | "ask")
}

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
