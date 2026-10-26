//! Claude plugin and marketplace manifests, and Claude's rule for combining them.
//!
//! Rules: <https://code.claude.com/docs/en/plugins/manifest-reference> (a manifest list
//! **adds** to the default skills folder and **replaces** the default Agents and commands
//! folders) and <https://code.claude.com/docs/en/plugins/marketplace-reference#strict-mode>.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::diagnostic::{PathSegment, SchemaDiagnostic, Severity, key_path};

pub(crate) const PLUGIN_MANIFEST: &str = ".claude-plugin/plugin.json";
pub const MARKETPLACE_MANIFEST: &str = ".claude-plugin/marketplace.json";

/// The component lists a manifest or marketplace entry can carry. `None` means the key is
/// absent, which is different from an empty list (an empty Agents list replaces the default).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentLists {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agents: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commands: Option<Vec<String>>,
    /// The manifest declares `hooks` or `mcpServers`; neither is converted yet.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hooks: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mcp: bool,
}

impl ComponentLists {
    fn has_components(&self) -> bool {
        self.skills.is_some() || self.agents.is_some() || self.commands.is_some() || self.hooks
    }

    fn from_object(
        object: &serde_json::Map<String, Value>,
        file: &str,
        diagnostics: &mut Vec<(String, SchemaDiagnostic)>,
    ) -> Self {
        let mut list = |key: &str| {
            object
                .get(key)
                .map(|value| declared_paths(value, key, file, diagnostics))
        };
        Self {
            skills: list("skills"),
            agents: list("agents"),
            commands: list("commands"),
            hooks: object.contains_key("hooks"),
            mcp: object.contains_key("mcpServers"),
        }
    }
}

/// A list value is a path, an array of paths, or (as Mars read it before) `{path}` objects.
/// An entry that is not a `./` path is reported and left out; `.` names the plugin root.
fn declared_paths(
    value: &Value,
    key: &str,
    file: &str,
    diagnostics: &mut Vec<(String, SchemaDiagnostic)>,
) -> Vec<String> {
    let entries: Vec<&Value> = match value {
        Value::Array(values) => values.iter().collect(),
        other => vec![other],
    };
    let mut paths = Vec::new();
    for entry in entries {
        let raw = entry
            .as_str()
            .or_else(|| entry.get("path").and_then(Value::as_str));
        match raw {
            Some(raw) if raw == "." || raw.starts_with("./") => paths.push(raw.to_owned()),
            _ => diagnostics.push((
                file.to_owned(),
                SchemaDiagnostic::new(
                    "source-manifest.path-invalid",
                    Severity::Warning,
                    key_path(key),
                    format!("`{key}` entry {entry} is not a `./` path and was left out"),
                ),
            )),
        }
    }
    paths
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginManifest {
    pub name: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    pub lists: ComponentLists,
}

fn parse_object(
    bytes: &[u8],
    file: &str,
) -> Result<serde_json::Map<String, Value>, SchemaDiagnostic> {
    let fail = |message: String| {
        SchemaDiagnostic::new(
            "source-manifest.invalid-json",
            Severity::Error,
            Vec::new(),
            message,
        )
    };
    match serde_json::from_slice::<Value>(bytes) {
        Ok(Value::Object(object)) => Ok(object),
        Ok(_) => Err(fail(format!("`{file}` must be a JSON object"))),
        Err(error) => Err(fail(format!("failed to parse `{file}`: {error}"))),
    }
}

fn text(object: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    object.get(key).and_then(Value::as_str).map(str::to_owned)
}

impl PluginManifest {
    pub fn parse(
        bytes: &[u8],
        diagnostics: &mut Vec<(String, SchemaDiagnostic)>,
    ) -> Result<Self, SchemaDiagnostic> {
        let object = parse_object(bytes, PLUGIN_MANIFEST)?;
        Ok(Self {
            name: text(&object, "name"),
            version: text(&object, "version"),
            description: text(&object, "description"),
            lists: ComponentLists::from_object(&object, PLUGIN_MANIFEST, diagnostics),
        })
    }
}

/// Where a marketplace plugin's files come from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum PluginSource {
    /// A directory of the marketplace's own checkout; `path` is root-relative (`""` is the root).
    Local { path: String },
    /// A Git repository the caller fetches. `path` roots the plugin inside it.
    Remote {
        repository: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        checkout: Checkout,
    },
    /// `npm`, `archive`, `command`, or a shape this version does not know.
    Unsupported { reason: String },
}

/// What to check out: a full commit, else a branch or tag, else the default branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Checkout {
    Sha { sha: String },
    Ref { name: String },
    DefaultBranch,
}

/// One marketplace `plugins[]` entry, as `inspect` reports it and `normalize` takes it back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketplaceEntry {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub source: PluginSource,
    /// `strict` as written; Claude treats an absent value as `true`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
    #[serde(default)]
    pub lists: ComponentLists,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Marketplace {
    pub name: Option<String>,
    pub plugins: Vec<MarketplaceEntry>,
}

impl Marketplace {
    pub fn parse(
        bytes: &[u8],
        diagnostics: &mut Vec<(String, SchemaDiagnostic)>,
    ) -> Result<Self, SchemaDiagnostic> {
        let object = parse_object(bytes, MARKETPLACE_MANIFEST)?;
        let plugin_root = object
            .get("metadata")
            .and_then(|metadata| metadata.get("pluginRoot"))
            .and_then(Value::as_str);
        let mut plugins = Vec::new();
        for (index, entry) in object
            .get("plugins")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            let Some(entry) = entry.as_object() else {
                continue;
            };
            let Some(name) = text(entry, "name") else {
                diagnostics.push((
                    MARKETPLACE_MANIFEST.to_owned(),
                    SchemaDiagnostic::new(
                        "marketplace.plugin-unnamed",
                        Severity::Warning,
                        vec![
                            PathSegment::Key("plugins".into()),
                            PathSegment::Index(index),
                        ],
                        "a plugin entry without a `name` was left out",
                    ),
                ));
                continue;
            };
            plugins.push(MarketplaceEntry {
                source: parse_source(entry.get("source"), plugin_root),
                description: text(entry, "description"),
                version: text(entry, "version"),
                strict: entry.get("strict").and_then(Value::as_bool),
                lists: ComponentLists::from_object(entry, MARKETPLACE_MANIFEST, diagnostics),
                name,
            });
        }
        Ok(Self {
            name: text(&object, "name"),
            plugins,
        })
    }
}

fn unsupported(reason: impl Into<String>) -> PluginSource {
    PluginSource::Unsupported {
        reason: reason.into(),
    }
}

/// Normalize a marketplace-relative path: `.` and `./` are the root, `..` and absolute
/// paths are refused.
fn local_path(raw: &str) -> Option<String> {
    let trimmed = raw.strip_prefix("./").unwrap_or(raw).trim_end_matches('/');
    let trimmed = if trimmed == "." { "" } else { trimmed };
    if trimmed
        .split('/')
        .any(|part| matches!(part, ".." | "") && !trimmed.is_empty())
        || trimmed.starts_with('/')
        || trimmed.contains('\\')
    {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

fn parse_source(source: Option<&Value>, plugin_root: Option<&str>) -> PluginSource {
    let Some(source) = source else {
        return unsupported("the entry has no `source`");
    };
    if let Some(raw) = source.as_str() {
        let relative = if raw == "." || raw.starts_with("./") {
            raw.to_owned()
        } else if let Some(root) = plugin_root {
            // A bare name resolves under `metadata.pluginRoot`.
            format!("{}/{raw}", root.trim_end_matches('/'))
        } else {
            return unsupported(format!("`{raw}` is not a `./` path"));
        };
        return match local_path(&relative) {
            Some(path) => PluginSource::Local { path },
            None => unsupported(format!("`{raw}` leaves the marketplace")),
        };
    }
    let Some(object) = source.as_object() else {
        return unsupported("`source` must be a path or an object");
    };
    let field = |key: &str| object.get(key).and_then(Value::as_str);
    let checkout = || match (field("sha"), field("ref")) {
        (Some(sha), _) if is_full_sha(sha) => Checkout::Sha {
            sha: sha.to_owned(),
        },
        (_, Some(name)) => Checkout::Ref {
            name: name.to_owned(),
        },
        _ => Checkout::DefaultBranch,
    };
    let kind = field("source").unwrap_or_default();
    match kind {
        "github" => match field("repo") {
            Some(repo) => PluginSource::Remote {
                repository: repository_url(repo),
                path: None,
                checkout: checkout(),
            },
            None => unsupported("a `github` source needs `repo`"),
        },
        "url" => match field("url") {
            Some(url) => PluginSource::Remote {
                repository: repository_url(url),
                path: None,
                checkout: checkout(),
            },
            None => unsupported("a `url` source needs `url`"),
        },
        "git-subdir" => match (field("url"), field("path").and_then(local_path)) {
            (Some(url), Some(path)) => PluginSource::Remote {
                repository: repository_url(url),
                path: Some(path).filter(|path| !path.is_empty()),
                checkout: checkout(),
            },
            _ => unsupported("a `git-subdir` source needs `url` and an in-repository `path`"),
        },
        "" => unsupported("`source` has no `source` type"),
        other => unsupported(format!("`{other}` sources cannot be imported")),
    }
}

fn is_full_sha(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// `owner/repo` shorthand means GitHub; anything else is a URL as written, minus `.git`.
fn repository_url(value: &str) -> String {
    let value = value.strip_suffix(".git").unwrap_or(value);
    let shorthand = !value.contains("://")
        && !value.starts_with("git@")
        && value.matches('/').count() == 1
        && !value.starts_with('/');
    if shorthand {
        format!("https://github.com/{value}")
    } else {
        value.to_owned()
    }
}

/// What one plugin contains once Claude's rules have combined `plugin.json` with a
/// marketplace entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginSpec {
    pub name: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    pub skills: SkillSources,
    /// `None` reads the default `agents/` folder.
    pub agents: Option<Vec<String>>,
    /// `None` reads the default `commands/` folder; commands are only reported.
    pub commands: Option<Vec<String>>,
    pub hooks: bool,
    pub mcp: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillSources {
    /// No manifest: every `SKILL.md` directory within five levels of the root.
    Walk,
    /// A manifest: the default `skills/<name>/SKILL.md` (unless `default` is off) plus `listed`.
    Declared { default: bool, listed: Vec<String> },
}

impl PluginSpec {
    /// The spec of a tree with no manifest, where every `SKILL.md` directory is a skill.
    pub(crate) fn is_bare_tree(&self) -> bool {
        self.skills == SkillSources::Walk
    }

    pub fn walk() -> Self {
        Self {
            name: None,
            version: None,
            description: None,
            skills: SkillSources::Walk,
            agents: None,
            commands: None,
            hooks: false,
            mcp: false,
        }
    }
}

/// The marketplace entry and `strict` rules; `entry_at_root` is whether the entry's source is
/// the marketplace root itself.
pub(crate) fn resolve_spec(
    manifest: Option<&PluginManifest>,
    entry: Option<&MarketplaceEntry>,
    entry_at_root: bool,
) -> Result<PluginSpec, String> {
    let name = manifest
        .and_then(|manifest| manifest.name.clone())
        .or_else(|| entry.map(|entry| entry.name.clone()));
    let version = manifest
        .and_then(|manifest| manifest.version.clone())
        .or_else(|| entry.and_then(|entry| entry.version.clone()));
    let description = manifest
        .and_then(|manifest| manifest.description.clone())
        .or_else(|| entry.and_then(|entry| entry.description.clone()));
    let empty = ComponentLists::default();
    let entry_lists = entry.map_or(&empty, |entry| &entry.lists);

    let (lists, default_skills) = match manifest {
        None => (
            entry_lists.clone(),
            // Only the marketplace-root entry with a `skills` list narrows the default scan.
            !(entry_at_root && entry_lists.skills.is_some()),
        ),
        Some(manifest) => {
            if entry.is_some_and(|entry| entry.strict == Some(false))
                && entry_lists.has_components()
            {
                return Err("conflicting manifests: both plugin.json and the marketplace entry (with `strict: false`) specify components".to_owned());
            }
            // A non-strict entry with no components adds nothing, so appending is uniform.
            let append = |own: &Option<Vec<String>>, entry: &Option<Vec<String>>| {
                (own.is_some() || entry.is_some()).then(|| {
                    own.iter()
                        .flatten()
                        .chain(entry.iter().flatten())
                        .cloned()
                        .collect()
                })
            };
            (
                ComponentLists {
                    skills: append(&manifest.lists.skills, &entry_lists.skills),
                    agents: append(&manifest.lists.agents, &entry_lists.agents),
                    commands: append(&manifest.lists.commands, &entry_lists.commands),
                    hooks: manifest.lists.hooks || entry_lists.hooks,
                    mcp: manifest.lists.mcp,
                },
                true,
            )
        }
    };
    Ok(PluginSpec {
        name,
        version,
        description,
        skills: SkillSources::Declared {
            default: default_skills,
            listed: lists.skills.unwrap_or_default(),
        },
        agents: lists.agents,
        commands: lists.commands,
        hooks: lists.hooks,
        mcp: lists.mcp,
    })
}
