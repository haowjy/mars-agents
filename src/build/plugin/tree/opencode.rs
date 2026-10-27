use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;

use crate::compiler::hooks::ParsedHookItem;
use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::target::opencode::{McpServer, OpencodeAdapter};
use crate::target::{ConfigEntry, TargetAdapter};
use mars_schema::HarnessId;
use mars_schema::plugin_metadata::OPENCODE_ROOT_FILE;

use super::super::metadata::PackageMetadata;
use super::super::spec::{Finish, PluginRoot, RootFile, TreeProfile};
use super::WrittenTreeContents;

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
    agents: Some(super::keep_lowered),
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
    let servers = entries
        .iter()
        .filter_map(|entry| match entry {
            ConfigEntry::McpServer(server) => Some((server.name.clone(), McpServer::from(server))),
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

    let agents = agent_data(root, &metadata.name, diag)?;

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
            mcp_dir: crate::compiler::mcp::MCP_DIR_TOKEN,
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

/// Read the natively lowered `agents/<name>.md` files into plugin data, then delete them: OpenCode
/// loads no agent files from a plugin, and the fixed `index.mjs` reads only `DATA`.
fn agent_data(
    root: &Path,
    plugin: &str,
    diag: &mut DiagnosticCollector,
) -> Result<Vec<AgentData>, MarsError> {
    let directory = root.join("agents");
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut paths = std::fs::read_dir(&directory)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    paths.sort();
    let mut agents = Vec::new();
    for path in paths {
        let name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default()
            .to_string();
        let content = std::fs::read_to_string(&path)?;
        let invalid = |message: String| {
            MarsError::Internal(format!(
                "generated OpenCode agent {} is invalid: {message}",
                path.display()
            ))
        };
        let mut parsed = mars_schema::frontmatter::parse(&content)
            .map_err(|error| invalid(error.to_string()))?;
        let mut take_string = |key: &str| match parsed.remove(key) {
            Some(serde_yaml::Value::String(value)) => Some(value),
            _ => None,
        };
        let description = take_string("description");
        // OpenCode's own agent files default to `all`; a plugin agent defaults to `primary`,
        // which the `subagent` tool refuses.
        let mode = take_string("mode").unwrap_or_else(|| "all".to_string());
        let model = take_string("model").and_then(|model| {
            let split = model.split_once('/');
            if split.is_none() {
                diag.warn(
                    "plugin-opencode-agent-model",
                    format!(
                        "agent `{name}`: model `{model}` has no `provider/model` form, so the plugin API cannot set it; the agent uses the harness default"
                    ),
                );
            }
            split.map(|(provider, id)| ModelData {
                provider_id: provider.to_string(),
                id: id.to_string(),
            })
        });
        parsed.remove("name");
        let mut permissions = vec![PermissionRule {
            action: "skill".to_string(),
            resource: format!("{plugin}:*"),
            effect: "allow".to_string(),
        }];
        if let Some(serde_yaml::Value::Mapping(map)) = parsed.remove("permission") {
            permissions.extend(permission_rules(&map));
        }
        for key in parsed.keys() {
            diag.warn(
                "plugin-opencode-agent-field",
                format!("agent `{name}`: field `{key}` has no plugin agent API equivalent and was dropped"),
            );
        }
        agents.push(AgentData {
            id: format!("{plugin}:{name}"),
            name,
            description,
            mode,
            model,
            system: parsed.body().to_string(),
            permissions,
        });
    }
    std::fs::remove_dir_all(&directory)?;
    Ok(agents)
}

/// OpenCode's `permission` map as `Agent.Info` rules: `tool: effect` or `tool: {pattern: effect}`.
fn permission_rules(map: &serde_yaml::Mapping) -> Vec<PermissionRule> {
    let text = |value: &serde_yaml::Value| value.as_str().map(str::to_string);
    let mut rules = Vec::new();
    for (action, value) in map {
        let Some(action) = text(action) else { continue };
        match value {
            serde_yaml::Value::Mapping(patterns) => {
                for (resource, effect) in patterns {
                    if let (Some(resource), Some(effect)) = (text(resource), text(effect)) {
                        rules.push(PermissionRule {
                            action: action.clone(),
                            resource,
                            effect,
                        });
                    }
                }
            }
            effect => {
                if let Some(effect) = text(effect) {
                    rules.push(PermissionRule {
                        action,
                        resource: "*".to_string(),
                        effect,
                    });
                }
            }
        }
    }
    rules
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowered_agent_becomes_plugin_data() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("agents")).unwrap();
        std::fs::write(
            root.path().join("agents/reader.md"),
            "---\nname: reader\ndescription: Reads\nmodel: p/m\npermission:\n  edit: deny\n  bash:\n    'rm *': deny\n---\nBody\n",
        )
        .unwrap();

        let mut diag = DiagnosticCollector::new();
        let agents = agent_data(root.path(), "pkg", &mut diag).unwrap();

        let json = serde_json::to_value(&agents).unwrap();
        assert_eq!(json[0]["id"], "pkg:reader");
        assert_eq!(json[0]["mode"], "all");
        assert_eq!(json[0]["model"]["providerID"], "p");
        assert_eq!(json[0]["system"], "Body\n");
        assert_eq!(json[0]["permissions"][0]["resource"], "pkg:*");
        assert_eq!(json[0]["permissions"][1]["action"], "edit");
        assert_eq!(json[0]["permissions"][2]["resource"], "rm *");
        assert!(!root.path().join("agents").exists());
    }
}
