use std::path::Path;

use serde::Serialize;

use crate::compiler::hooks::ParsedHookItem;
use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::target::opencode::OpencodeAdapter;
use crate::target::{ConfigEntry, TargetAdapter};
use mars_schema::HarnessId;
use mars_schema::frontmatter::Frontmatter;
use mars_schema::plugin_metadata::OPENCODE_ROOT_FILE;

use super::super::metadata::PackageMetadata;
use super::super::spec::{Finish, PluginRoot, RootFile, TreeProfile};
use super::{LoweredAgent, WrittenTreeContents};
use crate::sync::target::TargetItem;

const TARGET_ROOT: &str = ".opencode";
const INDEX_TEMPLATE: &str = include_str!("../opencode/index.mjs");
const DATA_PLACEHOLDER: &str = "/*__MARS_PLUGIN_DATA__*/";

pub(in crate::build::plugin) static PROFILE: TreeProfile = TreeProfile {
    key: "opencode",
    harness: HarnessId::OpenCode,
    root_file: RootFile::NpmPackage {
        file: OPENCODE_ROOT_FILE,
    },
    dropped_agent_fields: &[],
    approximate_lowers_to_dropped: false,
    agents: Some(keep_plugin_agent_fields),
    native_model_aliases: &[],
    plugin_root: PluginRoot::RuntimeResolved,
    check_hook,
    write_config: write_config_entries,
    finish: Finish::Custom(write_package),
};

fn check_hook(hook: &ParsedHookItem, diag: &mut DiagnosticCollector) {
    let fragment = hook.fragment_name(TARGET_ROOT);
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
    if let Some(problem) = OpencodeAdapter.hook_fragment_problem(&content) {
        diag.warn(
            "hook-fragment-contract",
            format!(
                "hook `{}` from `{}`: OpenCode fragment `{fragment}` {problem}; the hook will still be shipped and isolated at runtime",
                hook.def.name, hook.source_name
            ),
        );
    }
}

fn write_config_entries(
    _profile: &TreeProfile,
    root: &Path,
    entries: &[ConfigEntry],
    _harnesses: &[&'static super::super::spec::PluginHarnessSpec],
    _diag: &mut DiagnosticCollector,
) -> Result<(), MarsError> {
    let (mcp, _) = super::standard_config_entries(entries);
    let servers = super::render_servers(&OpencodeAdapter, &mcp);
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
    /// Authored directory markers the runtime substitutes per installed file.
    tokens: DirTokens,
    skills: Vec<SkillData>,
    agents: Vec<AgentData>,
    mcp: Vec<&'a str>,
    hooks: Vec<HookData>,
}

#[derive(Serialize)]
struct DirTokens {
    #[serde(rename = "hookDir")]
    hook_dir: &'static str,
    #[serde(rename = "mcpDir")]
    mcp_dir: &'static str,
}

#[derive(Serialize)]
struct SkillData {
    directory: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

/// One agent for `ctx.agent`'s `editor.update`: the fields of OpenCode's `Agent.Info` that the
/// native lowering produces.
#[derive(Serialize)]
struct AgentData {
    id: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<ModelData>,
    system: String,
    permissions: Vec<PermissionRule>,
}

#[derive(Serialize)]
struct ModelData {
    #[serde(rename = "providerID")]
    provider_id: String,
    id: String,
}

#[derive(Serialize)]
struct PermissionRule {
    action: String,
    resource: String,
    effect: String,
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
    diag: &mut DiagnosticCollector,
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

    let mut agents = contents
        .agents
        .iter()
        .map(|agent| agent_data(&metadata.name, agent, diag))
        .collect::<Vec<_>>();
    agents.sort_by(|left, right| left.id.cmp(&right.id));

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
            fragment: hook.fragment_name(TARGET_ROOT),
        })
        .collect();

    let data = serde_json::to_string_pretty(&PluginData {
        plugin: &metadata.name,
        tokens: DirTokens {
            hook_dir: crate::compiler::hooks::HOOK_DIR_TOKEN,
            mcp_dir: mars_schema::mcp_server::MCP_DIR_TOKEN,
        },
        skills,
        agents,
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
    let reading = mars_schema::skill::read(&content);
    let fields = reading.fields.ok_or_else(|| {
        MarsError::Internal(format!(
            "generated OpenCode skill {} has invalid frontmatter: {}",
            path.display(),
            reading
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        ))
    })?;
    Ok(SkillData {
        directory: directory.to_string(),
        name: fields.name.unwrap_or_else(|| directory.to_string()),
        description: fields.description,
    })
}

/// The frontmatter keys `ctx.agent` takes; `name` and `permission` are read apart from the
/// plain strings. Any other key, from the lowering or an author override, is dropped.
const AGENT_KEYS: &[&str] = &["name", "description", "mode", "model", "permission"];

fn keep_plugin_agent_fields(
    doc: &mut Frontmatter,
    _source: &Frontmatter,
    item: &TargetItem,
    diag: &mut DiagnosticCollector,
) {
    for key in doc.keys() {
        if !AGENT_KEYS.contains(&key.as_str()) {
            super::warn_agent_field_dropped(diag, item.id.name.as_str(), &key, "OpenCode");
            doc.remove(&key);
        }
    }
}

/// One lowered agent as `ctx.agent` data. OpenCode loads no agent files from a plugin, and the
/// fixed `index.mjs` reads only `DATA`, so the lowered agent travels as data.
fn agent_data(plugin: &str, agent: &LoweredAgent, diag: &mut DiagnosticCollector) -> AgentData {
    let doc = &agent.doc;
    let text = |key: &str| doc.get(key).and_then(serde_yaml::Value::as_str);
    // OpenCode's own agent files default to `all`; a plugin agent defaults to `primary`,
    // which the `subagent` tool refuses.
    let model = text("model")
        .and_then(|model| model.split_once('/'))
        .map(|(provider, id)| ModelData {
            provider_id: provider.to_string(),
            id: id.to_string(),
        });
    let permissions = match doc.get("permission") {
        Some(serde_yaml::Value::Mapping(map)) => permission_rules(&agent.name, map, diag),
        Some(_) => {
            super::warn_agent_field_dropped(diag, &agent.name, "permission", "OpenCode");
            Vec::new()
        }
        None => Vec::new(),
    };
    AgentData {
        id: format!("{plugin}:{}", agent.name),
        name: agent.name.clone(),
        description: text("description").map(str::to_string),
        mode: text("mode").unwrap_or("all").to_string(),
        model,
        system: doc.body().to_string(),
        permissions,
    }
}

/// OpenCode's `permission` map as `Agent.Info` rules: `tool: effect` or `tool: {pattern: effect}`.
/// The lowering writes v1 keys, which OpenCode's file loader translates and `ctx.agent` does not.
fn permission_rules(
    agent: &str,
    map: &serde_yaml::Mapping,
    diag: &mut DiagnosticCollector,
) -> Vec<PermissionRule> {
    let mut rules = Vec::new();
    for (action, value) in map {
        let Some(action) = action.as_str() else {
            super::warn_agent_field_dropped(diag, agent, "permission", "OpenCode");
            continue;
        };
        let v2_action = mars_schema::tool_natives::opencode_v2_action(action);
        let entries: Vec<(Option<&str>, &serde_yaml::Value)> = match value {
            serde_yaml::Value::Mapping(patterns) => patterns
                .iter()
                .map(|(resource, effect)| (resource.as_str(), effect))
                .collect(),
            effect => vec![(Some("*"), effect)],
        };
        for (resource, effect) in entries {
            match (resource, effect.as_str()) {
                (Some(resource), Some(effect)) => rules.push(PermissionRule {
                    action: v2_action.to_string(),
                    resource: resource.to_string(),
                    effect: effect.to_string(),
                }),
                _ => super::warn_agent_field_dropped(
                    diag,
                    agent,
                    &format!("permission.{action}"),
                    "OpenCode",
                ),
            }
        }
    }
    rules
}
