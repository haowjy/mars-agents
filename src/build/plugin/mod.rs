//! Build one installable Claude-format plugin from a package's publish graph.

mod model;
pub mod output;
mod references;
mod tree;

pub use output::{SentinelInfo, replace_owned_tree};

use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde::Serialize;

use crate::compiler::agents::HarnessKind;
use crate::config::{
    Config, EffectiveConfig, EffectiveDependency, FilterMode, LocalConfig, Settings, SourceSpec,
};
use crate::diagnostic::{Diagnostic, DiagnosticCollector, DiagnosticLevel, LossinessMode};
use crate::error::{ConfigError, MarsError};
use crate::resolve::{PublishLens, ResolveOptions};
use crate::sync::rewrite::RenameIndex;
use crate::target::TargetAdapter;
use crate::target::claude::ClaudeAdapter;
use crate::types::{RenameMap, SourceId, SourceName};

#[derive(Debug)]
pub struct PluginBuildRequest {
    pub package_root: PathBuf,
    pub out_dir: PathBuf,
}

#[derive(Debug, Serialize)]
pub struct PluginBuildReport {
    pub plugin_name: String,
    pub version: String,
    pub output: PathBuf,
    #[serde(flatten)]
    pub components: PluginComponents,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Serialize)]
pub struct PluginComponents {
    pub agents: usize,
    pub skills: usize,
    pub hooks: usize,
    pub mcp_servers: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PluginTree {
    Own,
    SharesWith(HarnessKind),
}

struct PluginProfile {
    harness: HarnessKind,
    tree: PluginTree,
    target_root: &'static str,
    variant_key: &'static str,
    plugin_root_token: &'static str,
    adapter: &'static dyn TargetAdapter,
}

static CLAUDE_ADAPTER: ClaudeAdapter = ClaudeAdapter;

impl PluginProfile {
    fn claude() -> Self {
        Self {
            harness: HarnessKind::Claude,
            tree: PluginTree::Own,
            target_root: ".claude",
            variant_key: "claude",
            plugin_root_token: "${CLAUDE_PLUGIN_ROOT}",
            adapter: &CLAUDE_ADAPTER,
        }
    }

    fn codex() -> Self {
        Self {
            harness: HarnessKind::Codex,
            tree: PluginTree::SharesWith(HarnessKind::Claude),
            target_root: ".claude",
            variant_key: "claude",
            plugin_root_token: "${CLAUDE_PLUGIN_ROOT}",
            adapter: &CLAUDE_ADAPTER,
        }
    }
}

pub fn build_plugin(request: &PluginBuildRequest) -> Result<PluginBuildReport, MarsError> {
    let package_root =
        dunce::canonicalize(&request.package_root).map_err(|source| MarsError::Io {
            operation: "resolve plugin package root".to_string(),
            path: request.package_root.clone(),
            source,
        })?;
    let out_dir = absolute_from_current_dir(&request.out_dir)?;

    let mut diag = DiagnosticCollector::with_lossiness_mode(LossinessMode::Verbose);
    let lens = PublishLens::load(&package_root, &mut diag)?;
    let manifest = lens.manifest.as_ref().ok_or_else(|| {
        MarsError::Config(ConfigError::Invalid {
            message: format!(
                "{} is not a publishable package: mars.toml needs a [package] table",
                package_root.display()
            ),
        })
    })?;
    let package = manifest.package.as_ref().ok_or_else(|| {
        MarsError::Config(ConfigError::Invalid {
            message: "mars.toml needs a [package] table to build a plugin".to_string(),
        })
    })?;
    let plugin_name = package.name.clone();
    let root_source = SourceName::from(plugin_name.clone());

    let graph_staging = tempfile::TempDir::new()?;
    let effective = implicit_consumer_config(&lens, &root_source)?;
    let mut graph = crate::resolve::resolve_publish_graph_from_config(
        &lens,
        &effective,
        &ResolveOptions::default(),
        &mut diag,
    )?;
    stage_graph(
        &mut graph,
        &effective,
        &graph_staging.path().join("graph"),
        &mut diag,
    )?;
    warn_hooks_without_claude_fragment(&graph, &mut diag)?;
    let (mut target, explicit_renames, collision_renames) =
        crate::sync::target::build_with_collisions_and_diag(&graph, &effective, &mut diag)?;
    let rename_index = RenameIndex::new(&explicit_renames, &collision_renames, &target);
    if !rename_index.is_empty() {
        let dep_precedence: Vec<SourceName> = effective.dependencies.keys().cloned().collect();
        for warning in crate::sync::rewrite::apply_renames(
            &mut target,
            &rename_index,
            &graph,
            &dep_precedence,
        )? {
            diag.warn("rewrite-warning", warning);
        }
    }
    crate::sync::validate::validate_skill_frontmatter_in_target(&target, &mut diag);
    emit_missing_skill_warnings(&target, &mut diag);

    let aliases = crate::models::merged_model_aliases(
        &graph,
        &effective,
        &Config::default(),
        &LocalConfig::default(),
        &mut diag,
    );
    let dep_precedence: Vec<SourceName> = effective.dependencies.keys().cloned().collect();
    let profiles = [PluginProfile::claude(), PluginProfile::codex()];
    let profile = profiles
        .iter()
        .find(|profile| profile.tree == PluginTree::Own)
        .expect("Claude owns the v1 plugin tree");
    debug_assert_eq!(profile.harness, HarnessKind::Claude);
    debug_assert!(profiles.iter().any(|candidate| {
        candidate.harness == HarnessKind::Codex
            && candidate.tree == PluginTree::SharesWith(HarnessKind::Claude)
            && candidate.target_root == profile.target_root
    }));

    let prepared = tempfile::TempDir::new()?;
    let summary = tree::write_tree(
        prepared.path(),
        &tree::TreeInput {
            plugin_name: &plugin_name,
            package,
            root_source: &root_source,
            agent_overlays: &manifest.agents,
            aliases: &aliases,
            graph: &graph,
            target: &target,
            rename_index: &rename_index,
            collision_renames: &collision_renames,
            dep_precedence: &dep_precedence,
            profile,
        },
        &mut diag,
    )?;

    let diagnostics = diag.drain();
    fail_on_diagnostic_errors(&diagnostics)?;
    let inputs_hash = semantic_inputs_fingerprint(prepared.path(), &graph)?;
    let sentinel = output::SentinelInfo {
        plugin_name: plugin_name.clone(),
        version: package.version.clone(),
        inputs_hash,
    };
    output::replace_owned_tree(&out_dir, &sentinel, |staging| {
        copy_tree_contents(prepared.path(), staging)
    })?;

    Ok(PluginBuildReport {
        plugin_name,
        version: package.version.clone(),
        output: out_dir,
        components: PluginComponents {
            agents: summary.agents,
            skills: summary.skills,
            hooks: summary.hooks,
            mcp_servers: summary.mcp_servers,
        },
        diagnostics,
    })
}

fn implicit_consumer_config(
    lens: &PublishLens,
    root_source: &SourceName,
) -> Result<EffectiveConfig, MarsError> {
    let package_root = dunce::canonicalize(lens.package_root())?;
    let source_id = SourceId::path_with_subpath(lens.package_root(), &package_root, None)?;
    let mut dependencies = IndexMap::from([(
        root_source.clone(),
        EffectiveDependency {
            declared_source_id: source_id.clone(),
            source_id,
            spec: SourceSpec::Path(package_root),
            subpath: None,
            filter: FilterMode::All,
            rename: RenameMap::new(),
            dialect: None,
        },
    )]);
    // The resolver intentionally does not seed filtered manifest dependencies
    // from a transitive package. An implicit consumer does request those filters
    // directly, so mirror the building package's exported dependency entries at
    // the consumer edge while retaining the package itself as the unfiltered root.
    for (source_name, dependency) in &lens.effective.dependencies {
        // Keep the building package as the root if a malformed manifest reuses
        // its package name; the resolver can then report the self-dependency.
        dependencies
            .entry(source_name.clone())
            .or_insert_with(|| dependency.clone());
    }
    Ok(EffectiveConfig {
        dependencies,
        settings: Settings {
            targets: Some(vec![".claude".to_string()]),
            ..Settings::default()
        },
        skills: lens.effective.skills.clone(),
    })
}

fn stage_graph(
    graph: &mut crate::resolve::ResolvedGraph,
    effective: &EffectiveConfig,
    staging_root: &Path,
    diag: &mut DiagnosticCollector,
) -> Result<(), MarsError> {
    for source_name in graph.order.clone() {
        let node = graph
            .nodes
            .get_mut(&source_name)
            .expect("resolved graph order names an existing node");
        let dependency = effective.dependencies.get(&source_name);
        let dialect = crate::dialect::Dialect::resolve(
            dependency.and_then(|entry| entry.dialect),
            &node.rooted_ref.package_root,
        );
        let empty_renames = RenameMap::new();
        let renames = dependency
            .map(|entry| &entry.rename)
            .unwrap_or(&empty_renames);
        let staged = crate::staging::stage_rooted_source(
            &source_name,
            node.rooted_ref.clone(),
            crate::staging::RootedStageOptions { dialect },
            &effective.skills,
            renames,
            staging_root,
            diag,
        )?;
        node.rooted_ref = staged.rooted;
        if let crate::staging::HookSurfaceState::Unreadable { hook_names } = staged.hook_surface {
            graph
                .unreadable_hook_surfaces
                .insert(source_name, hook_names);
        }
    }
    Ok(())
}

fn warn_hooks_without_claude_fragment(
    graph: &crate::resolve::ResolvedGraph,
    diag: &mut DiagnosticCollector,
) -> Result<(), MarsError> {
    for source_name in &graph.order {
        let Some(filters) = graph.filters.get(source_name) else {
            continue;
        };
        let node = &graph.nodes[source_name];
        for hook_dir in crate::discover::discover_hook_directories(&node.rooted_ref.package_root)? {
            let hook_name = hook_dir
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let source_path = format!("hooks/{hook_name}");
            let selected = filters.iter().any(|filter| match filter {
                FilterMode::All => true,
                FilterMode::Exclude(excluded) => !excluded.iter().any(|entry| {
                    entry.as_str() == hook_name
                        || crate::target::paths_equivalent(entry.as_str(), &source_path)
                }),
                FilterMode::Include { .. } | FilterMode::OnlySkills | FilterMode::OnlyAgents => {
                    false
                }
            });
            if !selected {
                continue;
            }

            let raw = std::fs::read_to_string(hook_dir.join("hook.toml"))?;
            let value: toml::Value = toml::from_str(&raw).map_err(ConfigError::Parse)?;
            if value.get("visibility").and_then(toml::Value::as_str) != Some("exported") {
                continue;
            }
            let targets = value.get("targets").and_then(toml::Value::as_table);
            if targets.is_some_and(|targets| !targets.contains_key(".claude")) {
                diag.warn(
                    "plugin-hook-no-claude-fragment",
                    format!(
                        "hook `{hook_name}` from `{source_name}` has no Claude fragment and is omitted from the shared plugin tree"
                    ),
                );
            }
        }
    }
    Ok(())
}

fn emit_missing_skill_warnings(
    target: &crate::sync::target::TargetState,
    diag: &mut DiagnosticCollector,
) {
    for warning in crate::sync::validate::validate_skill_refs(target) {
        let crate::validate::ValidationWarning::MissingSkill {
            agent,
            skill_name,
            suggestion,
        } = warning;
        let suffix = suggestion
            .map(|name| format!(" (did you mean `{name}`?)"))
            .unwrap_or_default();
        diag.warn(
            "unresolved-reference",
            format!(
                "agent `{}` references skill `{skill_name}`, which is not shipped{suffix}",
                agent.name
            ),
        );
    }
}

fn fail_on_diagnostic_errors(diagnostics: &[Diagnostic]) -> Result<(), MarsError> {
    let errors: Vec<_> = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.level == DiagnosticLevel::Error)
        .map(|diagnostic| diagnostic.message.as_str())
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(MarsError::Config(ConfigError::Invalid {
            message: errors.join("\n"),
        }))
    }
}

fn semantic_inputs_fingerprint(
    root: &Path,
    graph: &crate::resolve::ResolvedGraph,
) -> Result<String, MarsError> {
    let mut entries = Vec::new();
    for entry in walkdir::WalkDir::new(root).sort_by_file_name() {
        let entry = entry.map_err(|error| MarsError::Internal(error.to_string()))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = portable_relative(root, entry.path());
        let mut bytes = std::fs::read(entry.path())?;
        if relative == ".claude-plugin/plugin.json" {
            let mut manifest: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|error| {
                    MarsError::Internal(format!("generated plugin manifest is invalid: {error}"))
                })?;
            if let Some(object) = manifest.as_object_mut() {
                object.remove("version");
            }
            bytes = serde_json::to_vec(&manifest).map_err(|error| {
                MarsError::Internal(format!("failed to fingerprint plugin manifest: {error}"))
            })?;
        }
        entries.push((relative, crate::hash::hash_bytes(&bytes)));
    }
    for source_name in &graph.order {
        let resolved = &graph.nodes[source_name].resolved_ref;
        if resolved.version.is_none() && resolved.version_tag.is_none() && resolved.commit.is_none()
        {
            continue;
        }
        let identity = serde_json::to_vec(&serde_json::json!({
            "version": resolved.version.as_ref().map(ToString::to_string),
            "version_tag": resolved.version_tag,
            "commit": resolved.commit.as_ref().map(ToString::to_string),
        }))
        .map_err(|error| {
            MarsError::Internal(format!(
                "failed to fingerprint resolved dependency: {error}"
            ))
        })?;
        entries.push((
            format!("@resolved/{source_name}"),
            crate::hash::hash_bytes(&identity),
        ));
    }
    Ok(crate::hash::hash_file_manifest(entries))
}

fn copy_tree_contents(source: &Path, destination: &Path) -> Result<(), MarsError> {
    for entry in walkdir::WalkDir::new(source)
        .sort_by_file_name()
        .min_depth(1)
    {
        let entry = entry.map_err(|error| MarsError::Internal(error.to_string()))?;
        let relative = entry.path().strip_prefix(source).map_err(|error| {
            MarsError::Internal(format!("prepared plugin path escaped its root: {error}"))
        })?;
        let target = destination.join(relative);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target)?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(entry.path(), &target)?;
        } else {
            return Err(MarsError::Config(ConfigError::Invalid {
                message: format!(
                    "generated plugin contains unsupported filesystem entry `{}`",
                    entry.path().display()
                ),
            }));
        }
    }
    Ok(())
}

fn absolute_from_current_dir(path: &Path) -> Result<PathBuf, MarsError> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn portable_relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}
