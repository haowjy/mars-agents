use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;

use crate::compiler::agents::HarnessKind;
use crate::compiler::hooks::{ParsedHookItem, default_fragment_name};
use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::target::ConfigEntry;

use super::super::PluginTree;
use super::super::metadata::PackageMetadata;
use super::{TreeProfile, WrittenTreeContents};

const TARGET_ROOT: &str = ".opencode";
const INDEX_TEMPLATE: &str = include_str!("../opencode/index.mjs");
const DATA_PLACEHOLDER: &str = "/*__MARS_PLUGIN_DATA__*/";

pub(super) static PROFILE: TreeProfile = TreeProfile {
    tree: PluginTree::OpenCode,
    harness: HarnessKind::OpenCode,
    agents: None,
    native_model_aliases: &[],
    plugin_root_token: None,
    accept_hook,
    write_config: write_config_entries,
    finish: write_package,
};

fn accept_hook(hook: &ParsedHookItem, diag: &mut DiagnosticCollector) -> bool {
    let fragment = fragment_name(hook);
    let path = hook.hook_dir.join(&fragment);
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) => {
            diag.warn(
                "hook-fragment",
                format!(
                    "hook `{}` from `{}`: could not check OpenCode fragment `{fragment}` for a default export: {error}; the hook will still be shipped and isolated at runtime",
                    hook.def.name, hook.source_name
                ),
            );
            return true;
        }
    };
    if has_default_export(&content) {
        return true;
    }

    diag.warn(
        "plugin-opencode-hook-default-export",
        format!(
            "hook `{}` from `{}`: OpenCode fragment `{fragment}` has no recognizable default export; add a default `{{ id, setup }}` export; the hook will still be shipped and isolated at runtime",
            hook.def.name, hook.source_name
        ),
    );
    true
}

fn has_default_export(content: &str) -> bool {
    content.lines().any(|line| {
        let line = line.trim_start();
        let Some(after_export) = line.strip_prefix("export") else {
            return false;
        };
        if after_export
            .chars()
            .next()
            .is_some_and(|character| !character.is_whitespace() && character != '{')
        {
            return false;
        }
        let exported = after_export.trim_start();
        exported.strip_prefix("default").is_some_and(|rest| {
            rest.chars().next().is_none_or(|character| {
                !character.is_ascii_alphanumeric() && !matches!(character, '_' | '$')
            })
        }) || (exported.starts_with('{')
            && exported.split_once('}').is_some_and(|(bindings, _)| {
                bindings.split(',').any(|binding| {
                    binding.split_whitespace().collect::<Vec<_>>() == ["{", "default"]
                        || binding
                            .split_whitespace()
                            .collect::<Vec<_>>()
                            .windows(2)
                            .any(|tokens| tokens == ["as", "default"])
                })
            }))
    })
}

fn fragment_name(hook: &ParsedHookItem) -> String {
    hook.def.targets[TARGET_ROOT]
        .fragment
        .clone()
        .unwrap_or_else(|| default_fragment_name(TARGET_ROOT))
}

#[derive(Serialize)]
struct McpServer {
    #[serde(rename = "type")]
    kind: &'static str,
    command: Vec<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    environment: BTreeMap<String, String>,
}

fn write_config_entries(
    root: &Path,
    entries: &[ConfigEntry],
    _diag: &mut DiagnosticCollector,
) -> Result<(), MarsError> {
    let servers = entries
        .iter()
        .filter_map(|entry| match entry {
            ConfigEntry::McpServer(server) => Some((
                server.name.clone(),
                McpServer {
                    kind: "local",
                    command: std::iter::once(server.command.clone())
                        .chain(server.args.iter().cloned())
                        .collect(),
                    environment: server
                        .env
                        .iter()
                        .map(|(key, variable)| (key.clone(), format!("${{env:{variable}}}")))
                        .collect(),
                },
            )),
            ConfigEntry::Hook(_) => None,
        })
        .collect::<BTreeMap<_, _>>();
    if servers.is_empty() {
        return Ok(());
    }

    write_json(
        &root.join("mcp.json"),
        &servers,
        "OpenCode MCP configuration",
    )
}

#[derive(Serialize)]
struct PackageManifest<'a> {
    name: &'a str,
    version: &'a str,
    main: &'static str,
}

#[derive(Serialize)]
struct PluginData<'a> {
    plugin: &'a str,
    skills: Vec<SkillData>,
    mcp: Vec<&'a str>,
    hooks: Vec<HookData>,
}

#[derive(Serialize)]
struct SkillData {
    directory: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

#[derive(Serialize)]
struct HookData {
    name: String,
    fragment: String,
}

fn write_package(
    root: &Path,
    metadata: &PackageMetadata,
    contents: &WrittenTreeContents<'_>,
    _diag: &mut DiagnosticCollector,
) -> Result<(), MarsError> {
    let manifest = PackageManifest {
        name: &metadata.name,
        version: &metadata.version,
        main: "index.mjs",
    };
    write_json(
        &root.join("package.json"),
        &manifest,
        "OpenCode plugin manifest",
    )?;

    let mut skills = contents
        .skills
        .iter()
        .map(|name| skill_data(root, name))
        .collect::<Result<Vec<_>, _>>()?;
    skills.sort_by(|left, right| left.directory.cmp(&right.directory));

    let mut mcp = contents
        .config_entries
        .iter()
        .filter_map(|entry| match entry {
            ConfigEntry::McpServer(server) => Some(server.name.as_str()),
            ConfigEntry::Hook(_) => None,
        })
        .collect::<Vec<_>>();
    mcp.sort_unstable();

    let mut hooks = contents.hooks.to_vec();
    hooks.sort_by(|left, right| {
        (
            left.package_depth,
            left.decl_order,
            left.def.order,
            left.def.name.as_str(),
        )
            .cmp(&(
                right.package_depth,
                right.decl_order,
                right.def.order,
                right.def.name.as_str(),
            ))
    });
    let hooks = hooks
        .into_iter()
        .map(|hook| HookData {
            name: hook.def.name.clone(),
            fragment: fragment_name(hook),
        })
        .collect();

    let data = serde_json::to_string_pretty(&PluginData {
        plugin: &metadata.name,
        skills,
        mcp,
        hooks,
    })
    .map_err(|error| {
        MarsError::Internal(format!("failed to serialize OpenCode plugin data: {error}"))
    })?;
    let index = INDEX_TEMPLATE.replacen(DATA_PLACEHOLDER, &data, 1);
    if index == INDEX_TEMPLATE {
        return Err(MarsError::Internal(
            "OpenCode entry template is missing its data placeholder".to_string(),
        ));
    }
    crate::fs::atomic_write(&root.join("index.mjs"), index.as_bytes())
}

fn skill_data(root: &Path, directory: &str) -> Result<SkillData, MarsError> {
    let path = root.join("skills").join(directory).join("SKILL.md");
    let content = std::fs::read_to_string(&path)?;
    let frontmatter = crate::frontmatter::parse(&content).map_err(|error| {
        MarsError::Internal(format!(
            "generated OpenCode skill {} has invalid frontmatter: {error}",
            path.display()
        ))
    })?;
    Ok(SkillData {
        directory: directory.to_string(),
        name: frontmatter.name().unwrap_or(directory).to_string(),
        description: frontmatter
            .get("description")
            .and_then(serde_yaml::Value::as_str)
            .map(str::to_owned),
    })
}

fn write_json(path: &Path, value: &impl Serialize, description: &str) -> Result<(), MarsError> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|error| {
        MarsError::Internal(format!("failed to serialize {description}: {error}"))
    })?;
    bytes.push(b'\n');
    crate::fs::atomic_write(path, &bytes)
}
