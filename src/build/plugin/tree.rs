use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde::Serialize;

use crate::compiler::agents::HarnessKind;
use crate::compiler::agents::lower::lower_for_harness_with_model;
use crate::diagnostic::DiagnosticCollector;
use crate::error::{ConfigError, MarsError};
use crate::lock::ItemKind;
use crate::resolve::ResolvedGraph;
use crate::sync::rewrite::RenameIndex;
use crate::sync::target::{CollisionRename, TargetItem, TargetState};
use crate::target::ConfigEntry;
use crate::types::SourceName;

use super::PluginComponents;
use super::PluginProfile;
use super::model::resolve_claude_model;
use super::references::qualify_body_references;

pub(super) struct TermSet {
    terms: &'static [&'static str],
    lowercase: Vec<String>,
}

impl TermSet {
    pub(super) fn new(requires_meridian: bool) -> Self {
        let terms: &'static [&'static str] = if requires_meridian {
            &["mars sync", "mars add", ".mars/"]
        } else {
            &["meridian", "mars sync", "mars add", ".mars/"]
        };
        Self {
            terms,
            lowercase: terms.iter().map(|term| term.to_lowercase()).collect(),
        }
    }
}

pub(super) struct TreeInput<'a> {
    pub plugin_name: &'a str,
    pub package: &'a crate::config::PackageInfo,
    pub root_source: &'a SourceName,
    pub agent_overlays: &'a IndexMap<String, crate::config::AgentOverlay>,
    pub aliases: &'a IndexMap<String, crate::models::ModelAlias>,
    pub graph: &'a ResolvedGraph,
    pub target: &'a TargetState,
    pub rename_index: &'a RenameIndex,
    pub collision_renames: &'a [CollisionRename],
    pub dep_precedence: &'a [SourceName],
    pub profile: &'a PluginProfile,
    pub terms: &'a TermSet,
}

pub(super) fn write_tree(
    root: &Path,
    input: &TreeInput<'_>,
    diag: &mut DiagnosticCollector,
) -> Result<PluginComponents, MarsError> {
    write_manifest(root, input.plugin_name, input.package)?;

    let installed_skills: IndexMap<String, String> = input
        .target
        .items
        .values()
        .filter(|item| item.id.kind == ItemKind::Skill)
        .map(|item| {
            let name = item.id.name.to_string();
            (name.clone(), format!("{}:{name}", input.plugin_name))
        })
        .collect();

    let mut summary = PluginComponents::default();
    let mut skipped_bootstrap = false;
    for item in input.target.items.values() {
        match item.id.kind {
            ItemKind::Agent => {
                write_agent(root, item, input, &installed_skills, diag)?;
                summary.agents += 1;
            }
            ItemKind::Skill => {
                write_skill(root, item, input, &installed_skills, diag)?;
                summary.skills += 1;
            }
            ItemKind::BootstrapDoc => skipped_bootstrap = true,
            ItemKind::Hook | ItemKind::McpServer => {}
        }
    }
    if skipped_bootstrap {
        diag.info(
            "plugin-bootstrap-skipped",
            "bootstrap documents have no Claude plugin surface and were skipped",
        );
    }

    let hooks: Vec<_> = input
        .target
        .selected_hooks_for_target(input.profile.target_root)
        .map(|(hook, _)| hook)
        .collect();
    for hook in &hooks {
        let destination = root.join("hooks").join(&hook.def.name);
        crate::platform::fs::atomic_copy_dir(&hook.hook_dir, &destination)?;
        warn_terms_in_tree(
            &destination,
            &format!("hooks/{}", hook.def.name),
            input.terms,
            diag,
        )?;
    }
    summary.hooks = hooks.len();

    let mcp: Vec<_> = input
        .target
        .selected_mcp_for_target(input.profile.target_root)
        .collect();
    for item in &mcp {
        let content = std::fs::read_to_string(&item.source_path)?;
        warn_terms_in_content(
            &format!("mcp/{}/mcp.toml", item.name),
            &content,
            input.terms,
            diag,
        );
    }
    warn_relative_mcp_args(&mcp, diag);
    let entries = crate::compiler::config_entries::lower_config_entries_for_target(
        &hooks,
        &mcp,
        input.profile.adapter,
        |hook| {
            format!(
                "{}/hooks/{}",
                input.profile.plugin_root_token, hook.def.name
            )
        },
        diag,
    );
    write_config_entries(root, &entries)?;
    summary.mcp_servers = entries
        .iter()
        .filter(|entry| matches!(entry, ConfigEntry::McpServer(_)))
        .count();

    normalize_text_tree(root)?;
    Ok(summary)
}

fn write_agent(
    root: &Path,
    item: &TargetItem,
    input: &TreeInput<'_>,
    installed_skills: &IndexMap<String, String>,
    diag: &mut DiagnosticCollector,
) -> Result<(), MarsError> {
    let authored_content = std::fs::read_to_string(&item.source_path)?;
    warn_terms_in_content(
        &format!("agents/{}.md", item.id.name),
        &authored_content,
        input.terms,
        diag,
    );
    let content = item.rewritten_content.clone().unwrap_or(authored_content);
    let mut parse_diags = Vec::new();
    let (profile, frontmatter) =
        crate::compiler::agents::parse_agent_content(&content, &mut parse_diags)
            .map_err(|error| invalid_item("agent", item, error.to_string()))?;
    emit_agent_parse_diagnostics(item, &parse_diags, diag)?;

    let overlay = (item.source_name == *input.root_source)
        .then(|| agent_overlay_name(item, input.collision_renames))
        .and_then(|name| input.agent_overlays.get(name));
    let body_refs = body_reference_map(item, input, installed_skills);
    let mut effective = crate::compiler::effective_native_profile(&profile, overlay);
    qualify_agent_skills(&mut effective.skills, installed_skills);
    let model = resolve_claude_model(
        item.id.name.as_str(),
        effective.model.as_deref(),
        input.aliases,
        diag,
    );
    let lowered = lower_for_harness_with_model(
        &HarnessKind::Claude,
        &effective,
        &frontmatter,
        frontmatter.body(),
        &model,
    );
    crate::compiler::lossiness::emit_agent_lossiness_warnings(
        item.id.name.as_str(),
        &lowered.lossy_fields,
        diag,
    );

    let lowered = String::from_utf8(lowered.bytes).map_err(|error| {
        MarsError::Internal(format!(
            "Claude agent lowering produced invalid UTF-8 for `{}`: {error}",
            item.id.name
        ))
    })?;
    let lowered = apply_plugin_agent_policy(&lowered, &frontmatter, item, diag)?;
    let qualified = qualify_body_references(&lowered, &body_refs)?;

    let path = root.join("agents").join(format!("{}.md", item.id.name));
    crate::fs::atomic_write(&path, qualified.as_bytes())
}

fn qualify_agent_skills(
    skills: &mut crate::frontmatter::SkillsSpec,
    references: &IndexMap<String, String>,
) {
    for skill in skills.load.iter_mut().chain(&mut skills.available) {
        if let Some(qualified) = references.get(skill) {
            *skill = qualified.clone();
        }
    }
}

fn apply_plugin_agent_policy(
    lowered: &str,
    source: &crate::frontmatter::Frontmatter,
    item: &TargetItem,
    diag: &mut DiagnosticCollector,
) -> Result<String, MarsError> {
    let mut plugin = crate::frontmatter::parse(lowered)
        .map_err(|error| invalid_item("agent", item, error.to_string()))?;

    // The universal agent schema does not interpret these Claude-native fields,
    // but Claude plugins do. Preserve only the documented plugin-safe subset.
    for field in [
        "maxTurns",
        "autoCompactWindow",
        "memory",
        "background",
        "omitClaudeMd",
        "isolation",
        "color",
    ] {
        if let Some(value) = source.get(field) {
            plugin.insert(field, value.clone());
        }
    }
    if let Some(value) = plugin.remove("disallowed-tools") {
        plugin.insert("disallowedTools", value);
    }

    for field in ["hooks", "mcpServers", "permissionMode", "initialPrompt"] {
        if source.get(field).is_some() {
            diag.warn(
                "plugin-agent-field-dropped",
                format!(
                    "agent `{}`: `{field}` is ignored by Claude plugin agents and was dropped",
                    item.id.name
                ),
            );
        }
        plugin.remove(field);
    }

    Ok(plugin.render())
}

fn write_skill(
    root: &Path,
    item: &TargetItem,
    input: &TreeInput<'_>,
    installed_skills: &IndexMap<String, String>,
    diag: &mut DiagnosticCollector,
) -> Result<(), MarsError> {
    let destination = root.join("skills").join(item.id.name.as_str());
    let flat_source = item
        .is_flat_skill
        .then(tempfile::TempDir::new)
        .transpose()?;
    let source = if let Some(temp) = &flat_source {
        let filtered = temp.path().join("skill");
        crate::fs::atomic_install_dir_filtered(
            &item.source_path,
            &filtered,
            crate::fs::FLAT_SKILL_EXCLUDED_TOP_LEVEL,
        )?;
        filtered
    } else {
        item.source_path.clone()
    };
    crate::compiler::variants::project_skill_for_target(
        &source,
        &destination,
        Some(input.profile.variant_key),
        diag,
        item.id.name.as_str(),
    )?;
    if let Some(content) = &item.rewritten_content {
        crate::fs::atomic_write_preserving_permissions(
            &destination.join("SKILL.md"),
            content.as_bytes(),
        )?;
    }

    warn_terms_in_tree(
        &destination,
        &format!("skills/{}", item.id.name),
        input.terms,
        diag,
    )?;

    let body_refs = body_reference_map(item, input, installed_skills);
    for entry in walkdir::WalkDir::new(&destination).sort_by_file_name() {
        let entry = entry.map_err(|error| MarsError::Internal(error.to_string()))?;
        if !entry.file_type().is_file()
            || entry
                .path()
                .extension()
                .and_then(|extension| extension.to_str())
                != Some("md")
        {
            continue;
        }
        let content = std::fs::read_to_string(entry.path())?;
        let qualified = qualify_body_references(&content, &body_refs)?;
        if qualified != content {
            std::fs::write(entry.path(), qualified)?;
        }
    }

    let skill_md = destination.join("SKILL.md");
    let qualified = std::fs::read_to_string(&skill_md)?;
    crate::frontmatter::parse(&qualified)
        .map_err(|error| invalid_item("skill", item, error.to_string()))?;
    Ok(())
}

fn body_reference_map(
    item: &TargetItem,
    input: &TreeInput<'_>,
    installed_skills: &IndexMap<String, String>,
) -> IndexMap<String, String> {
    let visible_skill_renames = input.rename_index.visible_renames(
        input.target,
        input.graph,
        input.dep_precedence,
        &item.source_name,
        ItemKind::Skill,
    );
    let mut references = installed_skills.clone();
    for (original, installed) in visible_skill_renames {
        if let Some(qualified) = installed_skills.get(&installed) {
            references.insert(original, qualified.clone());
        }
    }
    references
}

fn agent_overlay_name<'a>(
    item: &'a TargetItem,
    collision_renames: &'a [CollisionRename],
) -> &'a str {
    collision_renames
        .iter()
        .find(|rename| {
            rename.kind == ItemKind::Agent
                && rename.source_name == item.source_name
                && rename.new_name == item.id.name
        })
        .map(|rename| rename.original_name.as_str())
        .unwrap_or(item.id.name.as_str())
}

fn emit_agent_parse_diagnostics(
    item: &TargetItem,
    findings: &[crate::compiler::agents::AgentDiagnostic],
    diag: &mut DiagnosticCollector,
) -> Result<(), MarsError> {
    for finding in findings {
        let message = format!("agent `{}`: {}", item.id.name, finding.message());
        if finding.is_error() {
            return Err(invalid_item("agent", item, message));
        }
        diag.warn("plugin-agent-schema", message);
    }
    Ok(())
}

fn write_manifest(
    root: &Path,
    plugin_name: &str,
    package: &crate::config::PackageInfo,
) -> Result<(), MarsError> {
    #[derive(Serialize)]
    struct PluginManifest<'a> {
        name: &'a str,
        version: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<&'a str>,
    }

    let manifest = PluginManifest {
        name: plugin_name,
        version: &package.version,
        description: package.description.as_deref(),
    };
    let mut bytes = serde_json::to_vec_pretty(&manifest).map_err(|error| {
        MarsError::Internal(format!(
            "failed to serialize Claude plugin manifest: {error}"
        ))
    })?;
    bytes.push(b'\n');
    crate::fs::atomic_write(&root.join(".claude-plugin/plugin.json"), &bytes)
}

fn write_config_entries(root: &Path, entries: &[ConfigEntry]) -> Result<(), MarsError> {
    let mcp: Vec<_> = entries
        .iter()
        .filter_map(|entry| match entry {
            ConfigEntry::McpServer(entry) => Some(entry),
            ConfigEntry::Hook(_) => None,
        })
        .collect();
    let hooks: Vec<_> = entries
        .iter()
        .filter_map(|entry| match entry {
            ConfigEntry::Hook(entry) => Some(entry),
            ConfigEntry::McpServer(_) => None,
        })
        .collect();

    if !mcp.is_empty() {
        crate::target::claude::write_mcp_json(&root.join(".mcp.json"), &mcp)?;
    }
    if !hooks.is_empty() {
        crate::target::claude::write_hooks_settings(&root.join("hooks/hooks.json"), &hooks)?;
    }
    Ok(())
}

fn warn_relative_mcp_args(
    items: &[&crate::compiler::mcp::ParsedMcpItem],
    diag: &mut DiagnosticCollector,
) {
    for item in items {
        for arg in &item.def.args {
            if ["./", "../", ".\\", "..\\"]
                .iter()
                .any(|prefix| arg.starts_with(prefix))
            {
                diag.warn(
                    "plugin-mcp-relative-path",
                    format!(
                        "MCP server `{}` from `{}` has package-relative argument `{arg}`; plugin roots are not rewritten",
                        item.name, item.source_name
                    ),
                );
            }
        }
    }
}

pub(super) fn emit_codex_warnings(summary: &PluginComponents, diag: &mut DiagnosticCollector) {
    if summary.agents > 0 {
        diag.warn(
            "plugin-codex-agents-dropped",
            format!(
                "codex: {} agents are present in the shared Claude tree but are not loaded by Codex plugins",
                summary.agents
            ),
        );
    }
    if summary.mcp_servers > 0 {
        diag.warn(
            "plugin-codex-mcp-unavailable",
            format!(
                "codex: {} MCP servers are listed from the shared Claude tree but are not callable in Codex 0.164.0",
                summary.mcp_servers
            ),
        );
    }
}

fn warn_terms_in_tree(
    root: &Path,
    display_prefix: &str,
    terms: &TermSet,
    diag: &mut DiagnosticCollector,
) -> Result<(), MarsError> {
    let mut files: Vec<PathBuf> = walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.into_path())
        .collect();
    files.sort_by_key(|path| crate::hash::portable_relative(root, path));

    for path in files {
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let relative = crate::hash::portable_relative(root, &path)
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        let display = if relative.is_empty() {
            display_prefix.to_string()
        } else {
            format!("{display_prefix}/{relative}")
        };
        warn_terms_in_content(&display, &content, terms, diag);
    }
    Ok(())
}

fn warn_terms_in_content(
    display_path: &str,
    content: &str,
    terms: &TermSet,
    diag: &mut DiagnosticCollector,
) {
    for (index, line) in content.lines().enumerate() {
        let lower = line.to_lowercase();
        if let Some((position, _)) = terms
            .lowercase
            .iter()
            .enumerate()
            .find(|(_, term)| lower.contains(term.as_str()))
        {
            diag.warn(
                "plugin-term",
                format!(
                    "{display_path}:{} mentions {:?}",
                    index + 1,
                    terms.terms[position]
                ),
            );
        }
    }
}

fn normalize_text_tree(root: &Path) -> Result<(), MarsError> {
    for entry in walkdir::WalkDir::new(root).sort_by_file_name() {
        let entry = entry.map_err(|error| MarsError::Internal(error.to_string()))?;
        if !entry.file_type().is_file() {
            continue;
        }
        if entry
            .path()
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                extension.eq_ignore_ascii_case("bat") || extension.eq_ignore_ascii_case("cmd")
            })
        {
            continue;
        }
        let bytes = std::fs::read(entry.path())?;
        let Ok(content) = String::from_utf8(bytes) else {
            continue;
        };
        let mut normalized = content.replace("\r\n", "\n").replace('\r', "\n");
        if entry
            .path()
            .extension()
            .and_then(|extension| extension.to_str())
            == Some("json")
            && !normalized.ends_with('\n')
        {
            normalized.push('\n');
        }
        if normalized != content {
            std::fs::write(entry.path(), normalized)?;
        }
    }
    Ok(())
}

fn invalid_item(kind: &str, item: &TargetItem, message: String) -> MarsError {
    MarsError::Config(ConfigError::Invalid {
        message: format!(
            "plugin {kind} `{}` from `{}` is invalid: {message}",
            item.id.name, item.source_name
        ),
    })
}
