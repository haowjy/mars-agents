use std::path::{Path, PathBuf};

use indexmap::IndexMap;

use crate::compiler::agents::lower::lower_for_harness_with_model;
use crate::compiler::harness_descriptor::{CompilerHarnessDescriptor, descriptor};
use crate::diagnostic::DiagnosticCollector;
use crate::error::{ConfigError, MarsError};
use crate::resolve::ResolvedGraph;
use crate::sync::rewrite::RenameIndex;
use crate::sync::target::{CollisionRename, TargetItem, TargetState};
use crate::target::{ConfigEntry, HookEntry, McpServerEntry};
use crate::types::SourceName;
use mars_schema::HarnessId;
use mars_schema::item::ItemKind;

use super::metadata::PackageMetadata;
use super::model::{ModelFallbackReason, resolve_model};
use super::references::qualify_body_references;
use super::{PluginComponents, PluginTree};

pub(super) mod claude;
mod cursor;
mod opencode;

pub(super) type AgentPolicy = fn(
    &str,
    &mars_schema::frontmatter::Frontmatter,
    &TargetItem,
    &mut DiagnosticCollector,
) -> Result<String, MarsError>;

pub(super) type ConfigWriter =
    fn(&TreeProfile, &Path, &[ConfigEntry], &mut DiagnosticCollector) -> Result<(), MarsError>;

pub(super) type FinishWriter = for<'a> fn(
    &TreeProfile,
    &Path,
    &PackageMetadata,
    &WrittenTreeContents<'a>,
    &mut DiagnosticCollector,
) -> Result<(), MarsError>;

pub(super) type HookCheck = fn(&crate::compiler::hooks::ParsedHookItem, &mut DiagnosticCollector);

pub(super) struct TreeProfile {
    format: &'static crate::plugin::PluginTreeSpec,
    harness: HarnessId,
    pub(super) agents: Option<AgentPolicy>,
    native_model_aliases: &'static [&'static str],
    plugin_root_token: Option<&'static str>,
    check_hook: HookCheck,
    standard_output: Option<StandardTreeOutput>,
    write_config: ConfigWriter,
    finish: FinishWriter,
}

impl TreeProfile {
    pub(super) fn descriptor(&self) -> &'static CompilerHarnessDescriptor {
        descriptor(self.harness)
    }

    pub(super) fn tree(&self) -> PluginTree {
        self.format.tree
    }

    pub(super) fn dropped_agent_fields(&self) -> &'static [&'static str] {
        self.format.dropped_agent_fields
    }
}

pub(super) struct WrittenTreeContents<'a> {
    pub skills: &'a [String],
    pub hooks: &'a [&'a crate::compiler::hooks::ParsedHookItem],
    pub config_entries: &'a [ConfigEntry],
}

pub(super) struct BuiltTree {
    profile: &'static TreeProfile,
    pub components: PluginComponents,
}

impl BuiltTree {
    pub(super) fn tree(&self) -> PluginTree {
        self.profile.tree()
    }
}

static PROFILES: [&TreeProfile; 3] = [&claude::PROFILE, &cursor::PROFILE, &opencode::PROFILE];

pub(super) fn profiles() -> impl Iterator<Item = &'static TreeProfile> {
    PROFILES.into_iter()
}

pub(super) fn profile(tree: PluginTree) -> &'static TreeProfile {
    profiles()
        .find(|profile| profile.tree() == tree)
        .expect("every plugin tree has a writer profile")
}

#[derive(serde::Serialize)]
struct StandardPluginManifest<'a> {
    name: &'a str,
    version: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    author: Option<&'a super::metadata::PackageAuthor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    homepage: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    license: Option<&'a str>,
}

pub(super) fn pretty_json_bytes(
    value: &impl serde::Serialize,
    description: &str,
) -> Result<Vec<u8>, MarsError> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|error| {
        MarsError::Internal(format!("failed to serialize {description}: {error}"))
    })?;
    bytes.push(b'\n');
    Ok(bytes)
}

pub(super) fn write_pretty_json(
    path: &Path,
    value: &impl serde::Serialize,
    description: &str,
) -> Result<(), MarsError> {
    crate::fs::atomic_write(path, &pretty_json_bytes(value, description)?)
}

pub(super) fn finish_standard_tree(
    profile: &TreeProfile,
    root: &Path,
    metadata: &PackageMetadata,
    _contents: &WrittenTreeContents<'_>,
    _diag: &mut DiagnosticCollector,
) -> Result<(), MarsError> {
    let output = profile
        .standard_output
        .expect("standard tree has output policy");
    write_pretty_json(
        &root.join(output.manifest_path),
        &StandardPluginManifest {
            name: &metadata.name,
            version: &metadata.version,
            description: metadata.description.as_deref(),
            author: metadata.author.as_ref(),
            homepage: metadata.homepage.as_deref(),
            license: metadata.license.as_deref(),
        },
        output.manifest_description,
    )
}

#[derive(Clone, Copy)]
pub(super) struct StandardTreeOutput {
    manifest_path: &'static str,
    manifest_description: &'static str,
}

pub(super) fn standard_config_entries(
    entries: &[ConfigEntry],
) -> (Vec<&McpServerEntry>, Vec<&HookEntry>) {
    let mut mcp = Vec::new();
    let mut hooks = Vec::new();
    for entry in entries {
        match entry {
            ConfigEntry::McpServer(entry) => mcp.push(entry),
            ConfigEntry::Hook(entry) => hooks.push(entry),
        }
    }
    (mcp, hooks)
}

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
    pub metadata: &'a PackageMetadata,
    pub root_source: &'a SourceName,
    pub agent_overlays: &'a IndexMap<String, crate::config::AgentOverlay>,
    pub aliases: &'a IndexMap<String, crate::models::ModelAlias>,
    pub graph: &'a ResolvedGraph,
    pub target: &'a TargetState,
    pub rename_index: &'a RenameIndex,
    pub collision_renames: &'a [CollisionRename],
    pub dep_precedence: &'a [SourceName],
    pub terms: &'a TermSet,
}

pub(super) fn check_sources(
    input: &TreeInput<'_>,
    profiles: &[&TreeProfile],
    diag: &mut DiagnosticCollector,
) -> Result<(), MarsError> {
    for item in input.target.items.values() {
        match item.id.kind {
            ItemKind::Agent => check_agent_source(item, input.terms, diag)?,
            ItemKind::Skill => {
                crate::compiler::skills::emit_plugin_skill_schema_diags_from_source(
                    &item.source_path,
                    item.id.name.as_str(),
                    diag,
                );
                let excluded = if item.is_flat_skill {
                    crate::fs::FLAT_SKILL_EXCLUDED_TOP_LEVEL
                } else {
                    &[]
                };
                warn_terms_in_tree_excluding(
                    &item.source_path,
                    &format!("skills/{}", item.id.name),
                    excluded,
                    input.terms,
                    diag,
                )?;
            }
            ItemKind::BootstrapDoc | ItemKind::Hook | ItemKind::McpServer => {}
        }
    }

    for hook in input.target.exported_hooks() {
        warn_terms_in_tree(
            &hook.hook_dir,
            &format!("hooks/{}", hook.def.name),
            input.terms,
            diag,
        )?;
    }

    let mut seen_mcp = std::collections::BTreeSet::new();
    let mut selected_mcp = Vec::new();
    for profile in profiles {
        for item in input
            .target
            .selected_mcp_for_target(profile.descriptor().target_dir)
        {
            if seen_mcp.insert(item.source_path.clone()) {
                let content = std::fs::read_to_string(&item.source_path)?;
                warn_terms_in_content(
                    &format!("mcp/{}/mcp.toml", item.name),
                    &content,
                    input.terms,
                    diag,
                );
                selected_mcp.push(item);
            }
        }
    }
    warn_relative_mcp_args(&selected_mcp, diag);
    for profile in profiles {
        for (hook, _) in input
            .target
            .selected_hooks_for_target(profile.descriptor().target_dir)
        {
            (profile.check_hook)(hook, diag);
        }
    }
    Ok(())
}

fn check_agent_source(
    item: &TargetItem,
    terms: &TermSet,
    diag: &mut DiagnosticCollector,
) -> Result<(), MarsError> {
    let authored_content = std::fs::read_to_string(&item.source_path)?;
    warn_terms_in_content(
        &format!("agents/{}.md", item.id.name),
        &authored_content,
        terms,
        diag,
    );
    let content = item
        .rewritten_content
        .as_deref()
        .unwrap_or(&authored_content);
    let mut parse_diags = Vec::new();
    crate::compiler::agents::parse_agent_content(content, &mut parse_diags)
        .map_err(|error| invalid_item("agent", item, error.to_string()))?;
    emit_agent_parse_diagnostics(item, &parse_diags, diag)
}

pub(super) fn write_tree(
    root: &Path,
    input: &TreeInput<'_>,
    profile: &'static TreeProfile,
    diag: &mut DiagnosticCollector,
) -> Result<BuiltTree, MarsError> {
    let descriptor = profile.descriptor();

    let installed_skills: IndexMap<String, String> = input
        .target
        .items
        .values()
        .filter(|item| item.id.kind == ItemKind::Skill)
        .map(|item| {
            let name = item.id.name.to_string();
            (name.clone(), format!("{}:{name}", input.metadata.name))
        })
        .collect();
    let skill_names = installed_skills.keys().cloned().collect::<Vec<_>>();

    let mut summary = PluginComponents::default();
    let mut model_fallbacks = ModelFallbackCounts::default();
    for item in input.target.items.values() {
        match item.id.kind {
            ItemKind::Agent => {
                if let Some(agent_policy) = profile.agents {
                    let fallback = write_agent(
                        root,
                        item,
                        input,
                        profile,
                        agent_policy,
                        &installed_skills,
                        diag,
                    )?;
                    model_fallbacks.record(fallback);
                    summary.agents += 1;
                }
            }
            ItemKind::Skill => {
                write_skill(root, item, input, profile, &installed_skills, diag)?;
            }
            ItemKind::BootstrapDoc => {}
            ItemKind::Hook | ItemKind::McpServer => {}
        }
    }
    model_fallbacks.emit(profile, diag);

    let hooks: Vec<_> = input
        .target
        .selected_hooks_for_target(descriptor.target_dir)
        .map(|(hook, _)| hook)
        .collect();
    for hook in &hooks {
        let destination = root.join("hooks").join(&hook.def.name);
        let excluded = other_hook_fragments(hook, descriptor.target_dir);
        crate::fs::copy_dir_filtered(&hook.hook_dir, &destination, &excluded)?;
    }
    let mcp: Vec<_> = input
        .target
        .selected_mcp_for_target(descriptor.target_dir)
        .collect();
    let registry = crate::target::TargetRegistry::new();
    let adapter = registry
        .get(descriptor.target_dir)
        .expect("compiler harness descriptor names a registered target");
    let entries = crate::compiler::config_entries::lower_config_entries_for_target(
        &hooks,
        &mcp,
        adapter,
        |hook| {
            format!(
                "{}/hooks/{}",
                profile
                    .plugin_root_token
                    .expect("merged hook config needs a plugin root token"),
                hook.def.name
            )
        },
        diag,
    );
    (profile.write_config)(profile, root, &entries, diag)?;
    let contents = WrittenTreeContents {
        skills: &skill_names,
        hooks: &hooks,
        config_entries: &entries,
    };
    summary.skills = contents.skills.len();
    summary.hooks = contents.hooks.len();
    summary.mcp_servers = contents
        .config_entries
        .iter()
        .filter(|entry| matches!(entry, ConfigEntry::McpServer(_)))
        .count();

    (profile.finish)(profile, root, input.metadata, &contents, diag)?;

    normalize_text_tree(root)?;
    Ok(BuiltTree {
        profile,
        components: summary,
    })
}

#[derive(Default)]
struct ModelFallbackCounts {
    not_package_alias: usize,
    no_deterministic_id: usize,
}

impl ModelFallbackCounts {
    fn record(&mut self, reason: Option<ModelFallbackReason>) {
        match reason {
            Some(ModelFallbackReason::NotPackageAlias) => self.not_package_alias += 1,
            Some(ModelFallbackReason::NoDeterministicId) => self.no_deterministic_id += 1,
            None => {}
        }
    }

    fn emit(&self, profile: &TreeProfile, diag: &mut DiagnosticCollector) {
        let total = self.not_package_alias + self.no_deterministic_id;
        if total == 0 {
            return;
        }
        let descriptor = profile.descriptor();
        let agents = if total == 1 { "agent" } else { "agents" };
        let verb = if total == 1 { "uses" } else { "use" };
        let reason = match (self.not_package_alias, self.no_deterministic_id) {
            (0, _) => format!(
                "configured models have no deterministic {} id",
                descriptor.display_name
            ),
            (_, 0) => "configured model tokens are not package aliases".to_string(),
            (not_aliases, no_ids) => format!(
                "{not_aliases} model tokens are not package aliases and {no_ids} have no deterministic {} id",
                descriptor.display_name
            ),
        };
        diag.info(
            "plugin-model-default",
            format!(
                "{total} {agents} {verb} the {} harness default because {reason}",
                descriptor.display_name
            ),
        );
    }
}

fn write_agent(
    root: &Path,
    item: &TargetItem,
    input: &TreeInput<'_>,
    tree_profile: &TreeProfile,
    agent_policy: AgentPolicy,
    installed_skills: &IndexMap<String, String>,
    diag: &mut DiagnosticCollector,
) -> Result<Option<ModelFallbackReason>, MarsError> {
    let authored_content = std::fs::read_to_string(&item.source_path)?;
    let content = item.rewritten_content.clone().unwrap_or(authored_content);
    let mut ignored_parse_diags = Vec::new();
    let (profile, frontmatter) =
        crate::compiler::agents::parse_agent_content(&content, &mut ignored_parse_diags)
            .map_err(|error| invalid_item("agent", item, error.to_string()))?;

    let overlay = (item.source_name == *input.root_source)
        .then(|| agent_overlay_name(item, input.collision_renames))
        .and_then(|name| input.agent_overlays.get(name));
    let body_refs = body_reference_map(item, input, installed_skills);
    let mut effective = crate::compiler::effective_native_profile(&profile, overlay);
    qualify_agent_skills(&mut effective.skills, installed_skills);
    let model = resolve_model(
        effective.model.as_deref(),
        tree_profile.harness,
        tree_profile.native_model_aliases,
        input.aliases,
    );
    let lowered = lower_for_harness_with_model(
        &tree_profile.harness,
        &effective,
        &frontmatter,
        frontmatter.body(),
        &model.native,
    );
    let plugin_lossy_fields;
    let lossy_fields = if tree_profile.format.approximate_lowers_to_dropped {
        plugin_lossy_fields = lowered
            .lossy_fields
            .iter()
            .cloned()
            .map(|mut field| {
                if matches!(
                    field.classification,
                    crate::compiler::lossiness::Lossiness::Approximate { .. }
                ) {
                    field.classification = crate::compiler::lossiness::Lossiness::Dropped;
                }
                field
            })
            .collect::<Vec<_>>();
        plugin_lossy_fields.as_slice()
    } else {
        lowered.lossy_fields.as_slice()
    };
    crate::compiler::lossiness::emit_agent_lossiness_warnings(
        item.id.name.as_str(),
        lossy_fields,
        diag,
    );

    let lowered = String::from_utf8(lowered.bytes).map_err(|error| {
        MarsError::Internal(format!(
            "{} agent lowering produced invalid UTF-8 for `{}`: {error}",
            tree_profile.descriptor().canonical_id,
            item.id.name
        ))
    })?;
    let lowered = agent_policy(&lowered, &frontmatter, item, diag)?;
    let qualified = qualify_body_references(&lowered, &body_refs)?;

    let path = root.join("agents").join(format!("{}.md", item.id.name));
    crate::fs::atomic_write(&path, qualified.as_bytes())?;
    Ok(model.fallback)
}

fn qualify_agent_skills(
    skills: &mut mars_schema::frontmatter::SkillsSpec,
    references: &IndexMap<String, String>,
) {
    for skill in skills.load.iter_mut().chain(&mut skills.available) {
        if let Some(qualified) = references.get(skill) {
            *skill = qualified.clone();
        }
    }
}

fn write_skill(
    root: &Path,
    item: &TargetItem,
    input: &TreeInput<'_>,
    profile: &TreeProfile,
    installed_skills: &IndexMap<String, String>,
    diag: &mut DiagnosticCollector,
) -> Result<(), MarsError> {
    let destination = root.join("skills").join(item.id.name.as_str());
    crate::compiler::variants::project_skill_for_target_with_checked_schema(
        &item.source_path,
        &destination,
        Some(profile.descriptor().variant_key),
        diag,
        item.id.name.as_str(),
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
        let qualified = qualify_body_references(&content, &body_refs)
            .map_err(|error| invalid_item("skill", item, error.to_string()))?;
        if qualified != content {
            std::fs::write(entry.path(), qualified)?;
        }
    }

    let skill_md = destination.join("SKILL.md");
    let qualified = std::fs::read_to_string(&skill_md)?;
    mars_schema::frontmatter::parse(&qualified)
        .map_err(|error| invalid_item("skill", item, error.to_string()))?;
    Ok(())
}

fn other_hook_fragments(
    hook: &crate::compiler::hooks::ParsedHookItem,
    target_dir: &str,
) -> Vec<PathBuf> {
    let current = hook.def.targets[target_dir]
        .fragment
        .clone()
        .unwrap_or_else(|| crate::compiler::hooks::default_fragment_name(target_dir));
    hook.def
        .targets
        .iter()
        .filter(|(target, _)| target.as_str() != target_dir)
        .map(|(target, config)| {
            config
                .fragment
                .clone()
                .unwrap_or_else(|| crate::compiler::hooks::default_fragment_name(target))
        })
        .filter(|fragment| fragment != &current)
        .map(PathBuf::from)
        .collect()
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
    findings: &[mars_schema::diagnostic::SchemaDiagnostic],
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

fn warn_terms_in_tree(
    root: &Path,
    display_prefix: &str,
    terms: &TermSet,
    diag: &mut DiagnosticCollector,
) -> Result<(), MarsError> {
    warn_terms_in_tree_excluding(root, display_prefix, &[], terms, diag)
}

fn warn_terms_in_tree_excluding(
    root: &Path,
    display_prefix: &str,
    excluded_top_level: &[&str],
    terms: &TermSet,
    diag: &mut DiagnosticCollector,
) -> Result<(), MarsError> {
    let mut files: Vec<PathBuf> = walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|entry| {
            entry.depth() != 1
                || entry
                    .file_name()
                    .to_str()
                    .is_none_or(|name| !excluded_top_level.contains(&name))
        })
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
