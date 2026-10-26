use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;

use crate::compiler::agents::HarnessKind;
use crate::compiler::hooks::{ParsedHookItem, default_fragment_name};
use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::target::ConfigEntry;

use super::super::metadata::PackageMetadata;
use super::{TreeProfile, WrittenTreeContents};

const TARGET_ROOT: &str = ".opencode";
const INDEX_TEMPLATE: &str = include_str!("../opencode/index.mjs");
const DATA_PLACEHOLDER: &str = "/*__MARS_PLUGIN_DATA__*/";

pub(super) static PROFILE: TreeProfile = TreeProfile {
    format: &crate::plugin::OPENCODE_TREE_SPEC,
    harness: HarnessKind::OpenCode,
    agents: None,
    native_model_aliases: &[],
    plugin_root_token: None,
    check_hook,
    standard_output: None,
    write_config: write_config_entries,
    finish: write_package,
};

fn check_hook(hook: &ParsedHookItem, diag: &mut DiagnosticCollector) {
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
            return;
        }
    };
    crate::target::opencode::check_hook_fragment(
        &hook.def.name,
        &hook.source_name,
        &fragment,
        &content,
        diag,
    );
}

fn fragment_name(hook: &ParsedHookItem) -> String {
    hook.def.targets[TARGET_ROOT]
        .fragment
        .clone()
        .unwrap_or_else(|| default_fragment_name(TARGET_ROOT))
}

fn write_config_entries(
    _profile: &TreeProfile,
    root: &Path,
    entries: &[ConfigEntry],
    _harnesses: &[&'static crate::plugin::PluginHarnessSpec],
    _diag: &mut DiagnosticCollector,
) -> Result<(), MarsError> {
    let servers = entries
        .iter()
        .filter_map(|entry| match entry {
            ConfigEntry::McpServer(server) => Some((
                server.name.clone(),
                crate::target::opencode::McpServer::from(server),
            )),
            ConfigEntry::Hook(_) => None,
        })
        .collect::<BTreeMap<_, _>>();
    if servers.is_empty() {
        return Ok(());
    }

    super::write_pretty_json(
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
    _profile: &TreeProfile,
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
    super::write_pretty_json(
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
