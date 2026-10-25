//! Build one installable Claude-format plugin from a package's publish graph.

mod model;
pub(crate) mod output;
mod references;
mod tree;

use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde::Serialize;

use crate::config::{
    Config, EffectiveConfig, EffectiveDependency, FilterMode, LocalConfig, Settings, SourceSpec,
};
use crate::diagnostic::{Diagnostic, DiagnosticCollector, DiagnosticLevel, LossinessMode};
use crate::error::{ConfigError, MarsError};
use crate::resolve::{PublishLens, ResolveOptions};
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

struct PluginProfile {
    target_root: &'static str,
    variant_key: &'static str,
    plugin_root_token: &'static str,
    adapter: &'static dyn TargetAdapter,
}

static CLAUDE_ADAPTER: ClaudeAdapter = ClaudeAdapter;

impl PluginProfile {
    fn claude() -> Self {
        Self {
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
    let prepared_target =
        crate::sync::prepare_target_state(&graph, &effective, &mut diag, |_, _| Ok(()))?;
    warn_hooks_without_claude_fragment(&prepared_target.target, &mut diag);
    emit_missing_skill_warnings(&prepared_target.warnings, &mut diag);

    let aliases = crate::models::merged_model_aliases(
        &graph,
        &effective,
        &Config::default(),
        &LocalConfig::default(),
        &mut diag,
    );
    let dep_precedence: Vec<SourceName> = effective.dependencies.keys().cloned().collect();
    let profile = PluginProfile::claude();
    let tree_input = tree::TreeInput {
        plugin_name: &plugin_name,
        package,
        root_source: &root_source,
        agent_overlays: &manifest.agents,
        aliases: &aliases,
        graph: &graph,
        target: &prepared_target.target,
        rename_index: &prepared_target.rename_index,
        collision_renames: &prepared_target.collision_renames,
        dep_precedence: &dep_precedence,
        profile: &profile,
    };
    let inputs_hash = plugin_inputs_hash(&package_root)?;
    let sentinel = crate::plugin_metadata::PluginSentinel {
        schema_version: crate::plugin_metadata::SENTINEL_SCHEMA_VERSION,
        mars_version: env!("CARGO_PKG_VERSION").to_string(),
        plugin_name: plugin_name.clone(),
        version: package.version.clone(),
        inputs_hash,
        dependencies: resolved_dependency_identities(&graph, &root_source),
        files: std::collections::BTreeMap::new(),
    };
    let mut summary = None;
    let mut diagnostics = None;
    output::replace_owned_tree(&out_dir, &sentinel, |staging| {
        let built = tree::write_tree(staging, &tree_input, &mut diag)?;
        tree::emit_codex_warnings(&built, &mut diag);
        let collected = diag.drain();
        fail_on_diagnostic_errors(&collected)?;
        summary = Some(built);
        diagnostics = Some(collected);
        Ok(())
    })?;
    let summary = summary.expect("plugin writer completed before publication");
    let diagnostics = diagnostics.expect("plugin writer collected diagnostics");

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
            targets: Some(vec![".claude".to_string(), ".codex".to_string()]),
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
    target: &crate::sync::target::TargetState,
    diag: &mut DiagnosticCollector,
) {
    let claude_hooks: std::collections::HashSet<_> = target
        .selected_hooks_for_target(".claude")
        .map(|(hook, _)| (hook.source_name.as_str(), hook.def.name.as_str()))
        .collect();
    for (hook, _) in target.selected_hooks_for_target(".codex") {
        if claude_hooks.contains(&(hook.source_name.as_str(), hook.def.name.as_str())) {
            continue;
        }
        diag.warn(
            "plugin-hook-no-claude-fragment",
            format!(
                "hook `{}` from `{}` has no Claude fragment and is omitted from the shared plugin tree",
                hook.def.name, hook.source_name
            ),
        );
    }
}

fn emit_missing_skill_warnings(
    warnings: &[crate::validate::ValidationWarning],
    diag: &mut DiagnosticCollector,
) {
    for warning in warnings {
        let crate::validate::ValidationWarning::MissingSkill {
            agent,
            skill_name,
            suggestion,
        } = warning;
        let suffix = suggestion
            .as_ref()
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

/// Hash package-owned build inputs without resolving or fetching dependencies.
pub(crate) fn plugin_inputs_hash(package_root: &Path) -> Result<String, MarsError> {
    let mut entries = Vec::new();
    let manifest_path = package_root.join("mars.toml");
    let raw_manifest = std::fs::read_to_string(&manifest_path)?;
    let mut manifest: toml::Value = toml::from_str(&raw_manifest).map_err(ConfigError::Parse)?;
    if let Some(package) = manifest
        .get_mut("package")
        .and_then(toml::Value::as_table_mut)
    {
        package.remove("version");
    }
    for table_name in [
        "package",
        "plugin",
        "dependencies",
        "models",
        "skills",
        "agents",
    ] {
        if let Some(value) = manifest.get(table_name) {
            let value = serde_json::to_value(value).map_err(|error| {
                MarsError::Internal(format!("failed to fingerprint [{table_name}]: {error}"))
            })?;
            let bytes = serde_json::to_vec(&canonical_json(value)).map_err(|error| {
                MarsError::Internal(format!("failed to fingerprint [{table_name}]: {error}"))
            })?;
            entries.push((
                format!("@config/{table_name}"),
                crate::hash::hash_bytes(&bytes),
            ));
        }
    }

    let mut ignored_diagnostics = DiagnosticCollector::new();
    let excluded = [PathBuf::from(".git"), PathBuf::from(".mars")];
    for entry in
        crate::fs::authored_tree_entries(package_root, &excluded, &mut ignored_diagnostics)?
    {
        if entry.kind != crate::fs::AuthoredEntryKind::File {
            continue;
        }
        let relative = entry
            .relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        if matches!(
            relative.as_str(),
            "mars.toml" | "mars.local.toml" | "mars.lock"
        ) {
            continue;
        }
        entries.push((
            format!("@source/{relative}"),
            crate::hash::hash_bytes(&std::fs::read(entry.source)?),
        ));
    }
    Ok(crate::hash::hash_file_manifest(entries))
}

fn canonical_json(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(canonical_json).collect())
        }
        serde_json::Value::Object(values) => {
            let sorted: std::collections::BTreeMap<_, _> = values
                .into_iter()
                .map(|(key, value)| (key, canonical_json(value)))
                .collect();
            serde_json::to_value(sorted).expect("JSON values serialize infallibly")
        }
        scalar => scalar,
    }
}

fn resolved_dependency_identities(
    graph: &crate::resolve::ResolvedGraph,
    root_source: &SourceName,
) -> std::collections::BTreeMap<String, crate::plugin_metadata::ResolvedDependency> {
    let mut dependencies = std::collections::BTreeMap::new();
    for source_name in &graph.order {
        if source_name == root_source {
            continue;
        }
        let resolved = &graph.nodes[source_name].resolved_ref;
        let version = resolved
            .version
            .as_ref()
            .map(ToString::to_string)
            .or_else(|| resolved.version_tag.clone());
        let commit = resolved.commit.as_ref().map(ToString::to_string);
        if version.is_none() && commit.is_none() {
            continue;
        }
        dependencies.insert(
            source_name.to_string(),
            crate::plugin_metadata::ResolvedDependency { version, commit },
        );
    }
    dependencies
}

fn absolute_from_current_dir(path: &Path) -> Result<PathBuf, MarsError> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}
