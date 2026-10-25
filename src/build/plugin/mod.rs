//! Build installable harness plugin trees from a package's publish graph.

mod inspect;
mod metadata;
mod model;
pub mod output;
mod references;
mod root_files;
mod support;
mod tree;

pub use inspect::{
    PluginDirectoryState, PluginInputsState, PluginOutputState, PluginRootFileState,
    PluginVersionState, inspect,
};

use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde::Serialize;

use crate::config::{
    Config, EffectiveConfig, EffectiveDependency, FilterMode, LocalConfig, PluginHarness, Settings,
    SourceSpec,
};
use crate::diagnostic::{Diagnostic, DiagnosticCollector, DiagnosticLevel, LossinessMode};
use crate::error::{ConfigError, MarsError};
use crate::resolve::{PublishLens, ResolveOptions};
use crate::types::{RenameMap, SourceId, SourceName};

use metadata::PackageMetadata;

#[derive(Debug)]
pub enum PluginBuildDestination {
    Configured,
    Out(PathBuf),
}

impl PluginBuildDestination {
    fn resolve(&self, package_root: &Path, manifest: &Config) -> Result<PathBuf, MarsError> {
        let configured = manifest
            .plugin
            .as_ref()
            .map(|plugin| configured_output_path(package_root, plugin))
            .transpose()?;
        match self {
            Self::Configured => configured.ok_or_else(|| MarsError::InvalidRequest {
                message: "a configured plugin build needs a [plugin] table in mars.toml"
                    .to_string(),
            }),
            Self::Out(path) => {
                let out = std::path::absolute(path)?;
                if configured
                    .as_ref()
                    .map(|configured| paths_resolve_equal(&out, configured))
                    .transpose()?
                    .unwrap_or(false)
                {
                    return Err(MarsError::InvalidRequest {
                        message: format!(
                            "--out {} resolves to [plugin].dir; omit --out for a configured build",
                            path.display()
                        ),
                    });
                }
                Ok(out)
            }
        }
    }
}

fn configured_output_path(
    package_root: &Path,
    plugin: &crate::config::PluginConfig,
) -> Result<PathBuf, MarsError> {
    crate::config::validate_plugin_dir(&plugin.dir)?;
    let package_root = resolved_path_identity(package_root)?;
    let output = std::path::absolute(package_root.join(&plugin.dir))?;
    let identity = resolved_path_identity(&output)?;
    if identity == package_root || !identity.starts_with(&package_root) {
        return Err(MarsError::InvalidRequest {
            message: format!(
                "[plugin].dir `{}` must resolve inside the package root",
                plugin.dir.display()
            ),
        });
    }
    Ok(output)
}

fn paths_resolve_equal(left: &Path, right: &Path) -> Result<bool, MarsError> {
    Ok(resolved_path_identity(left)? == resolved_path_identity(right)?)
}

fn resolved_path_identity(path: &Path) -> Result<PathBuf, MarsError> {
    let absolute = std::path::absolute(path)?;
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    let absolute = normalized;
    let mut existing = absolute.as_path();
    let mut missing = Vec::new();
    loop {
        match dunce::canonicalize(existing) {
            Ok(mut canonical) => {
                for component in missing.iter().rev() {
                    canonical.push(component);
                }
                return Ok(canonical);
            }
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                let component = existing.file_name().ok_or_else(|| MarsError::Io {
                    operation: "resolve plugin output path".to_string(),
                    path: absolute.clone(),
                    source,
                })?;
                missing.push(component.to_os_string());
                existing = existing.parent().ok_or_else(|| MarsError::InvalidRequest {
                    message: format!("cannot resolve plugin output path {}", absolute.display()),
                })?;
            }
            Err(source) => {
                return Err(MarsError::Io {
                    operation: "resolve plugin output path".to_string(),
                    path: existing.to_path_buf(),
                    source,
                });
            }
        }
    }
}

#[derive(Debug)]
pub struct PluginBuildRequest {
    pub package_root: PathBuf,
    pub destination: PluginBuildDestination,
    pub harnesses: Vec<PluginHarness>,
    pub version: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PluginBuildReport {
    pub plugin_name: String,
    pub version: String,
    pub trees: Vec<PluginTreeReport>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Serialize)]
pub struct PluginTreeReport {
    pub tree: PluginTree,
    pub output: PathBuf,
    #[serde(flatten)]
    pub components: PluginComponents,
}

pub struct StagedPluginBuild {
    output: output::StagedPlugin,
    root_files: root_files::RootFilePlan,
    report: PluginBuildReport,
}

impl StagedPluginBuild {
    /// Package-relative root files publication will write or delete.
    pub fn touched_root_files(&self) -> Vec<PathBuf> {
        self.root_files.touched_paths()
    }

    pub fn publish(self) -> Result<PluginBuildReport, MarsError> {
        root_files::apply(&self.root_files)?;
        self.output.publish()?;
        Ok(self.report)
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct PluginComponents {
    pub agents: usize,
    pub skills: usize,
    pub hooks: usize,
    pub mcp_servers: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginTree {
    Claude,
    Cursor,
    OpenCode,
}

impl PluginTree {
    const ALL: [Self; 3] = [Self::Claude, Self::Cursor, Self::OpenCode];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Cursor => "cursor",
            Self::OpenCode => "opencode",
        }
    }
}

impl PluginHarness {
    fn tree(self) -> PluginTree {
        match self {
            Self::Claude | Self::Codex | Self::Grok => PluginTree::Claude,
            Self::Cursor => PluginTree::Cursor,
            Self::OpenCode => PluginTree::OpenCode,
        }
    }
}

pub fn build_plugin(request: &PluginBuildRequest) -> Result<PluginBuildReport, MarsError> {
    stage_plugin(request)?.publish()
}

pub fn stage_plugin(request: &PluginBuildRequest) -> Result<StagedPluginBuild, MarsError> {
    let package_root =
        dunce::canonicalize(&request.package_root).map_err(|source| MarsError::Io {
            operation: "resolve plugin package root".to_string(),
            path: request.package_root.clone(),
            source,
        })?;
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
    let mut metadata = PackageMetadata::from(manifest)?;
    if let Some(version) = &request.version {
        metadata.version.clone_from(version);
    }
    let package = manifest
        .package
        .as_ref()
        .expect("PackageMetadata validates the package table");
    let out_dir = request.destination.resolve(&package_root, manifest)?;
    let plugin_name = metadata.name.clone();
    let root_source = SourceName::from(package.name.clone());
    if request.harnesses.is_empty() {
        return Err(MarsError::InvalidRequest {
            message: "a plugin build needs at least one harness".to_string(),
        });
    }
    let profiles = requested_tree_profiles(&request.harnesses);

    let graph_staging = tempfile::TempDir::new()?;
    let effective = implicit_consumer_config(&lens, &root_source, &profiles)?;
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
    emit_missing_skill_warnings(&prepared_target.warnings, &mut diag);

    let aliases = crate::models::merged_model_aliases(
        &graph,
        &effective,
        &Config::default(),
        &LocalConfig::default(),
        &mut diag,
    );
    let terms = tree::TermSet::new(package.requires_meridian.is_some());
    let tree_input = tree::TreeInput {
        metadata: &metadata,
        root_source: &root_source,
        agent_overlays: &manifest.agents,
        aliases: &aliases,
        graph: &graph,
        target: &prepared_target.target,
        rename_index: &prepared_target.rename_index,
        collision_renames: &prepared_target.collision_renames,
        dep_precedence: &prepared_target.dep_precedence,
        terms: &terms,
    };
    tree::check_sources(&tree_input, &profiles, &mut diag)?;
    let (staged_output, (built_trees, diagnostics, root_file_plan)) =
        output::stage(&out_dir, |staging| {
            let inputs_hash = plugin_inputs_hash(&package_root)?;
            let mut built_trees = Vec::with_capacity(profiles.len());
            for profile in &profiles {
                let tree_root = staging.join(profile.tree().as_str());
                built_trees.push(tree::write_tree(
                    &tree_root,
                    &tree_input,
                    profile,
                    &mut diag,
                )?);
            }
            support::emit(
                &request.harnesses,
                &prepared_target.target,
                &built_trees,
                &mut diag,
            );
            let root_file_plan = root_files::plan(
                &package_root,
                &out_dir,
                staging,
                &request.destination,
                &metadata,
                &request.harnesses,
                &mut diag,
            )?;
            let collected = diag.drain();
            fail_on_diagnostic_errors(&collected)?;
            Ok((
                crate::plugin_metadata::PluginSentinel {
                    schema_version: crate::plugin_metadata::SENTINEL_SCHEMA_VERSION,
                    mars_version: env!("CARGO_PKG_VERSION").to_string(),
                    plugin_name: plugin_name.clone(),
                    version: metadata.version.clone(),
                    inputs_hash,
                    dependencies: resolved_dependency_identities(&graph, &root_source),
                    files: std::collections::BTreeMap::new(),
                    root_files: root_file_plan.root_files.clone(),
                },
                (built_trees, collected, root_file_plan),
            ))
        })?;

    let trees = built_trees
        .into_iter()
        .map(|built| {
            let tree = built.tree();
            PluginTreeReport {
                tree,
                output: out_dir.join(tree.as_str()),
                components: built.components,
            }
        })
        .collect();

    Ok(StagedPluginBuild {
        output: staged_output,
        root_files: root_file_plan,
        report: PluginBuildReport {
            plugin_name,
            version: metadata.version,
            trees,
            diagnostics,
        },
    })
}

fn requested_tree_profiles(harnesses: &[PluginHarness]) -> Vec<&'static tree::TreeProfile> {
    PluginTree::ALL
        .into_iter()
        .filter(|tree| harnesses.iter().any(|harness| harness.tree() == *tree))
        .map(tree::profile)
        .collect()
}

fn implicit_consumer_config(
    lens: &PublishLens,
    root_source: &SourceName,
    profiles: &[&tree::TreeProfile],
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
            targets: Some(
                profiles
                    .iter()
                    .map(|profile| profile.descriptor().target_dir.to_string())
                    .collect(),
            ),
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
        let staged = crate::resolve::stage_rooted_package(
            &source_name,
            node.rooted_ref.clone(),
            effective,
            Some(staging_root),
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
    let mut entries = std::collections::BTreeMap::new();
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
            entries.insert(
                format!("@config/{table_name}"),
                crate::hash::hash_bytes(&bytes),
            );
        }
    }

    let mut diag = DiagnosticCollector::new();
    for item in crate::discover::discover_resolved_source(package_root, None, &mut diag)? {
        let source = package_root.join(&item.source_path);
        match item.id.kind {
            crate::lock::ItemKind::Agent | crate::lock::ItemKind::McpServer => {
                fingerprint_authored_file(package_root, &source, &mut entries)?;
            }
            crate::lock::ItemKind::Skill
            | crate::lock::ItemKind::Hook
            | crate::lock::ItemKind::BootstrapDoc => {
                let excluded = if item.id.kind == crate::lock::ItemKind::Skill
                    && item.source_path == Path::new(".")
                {
                    crate::fs::FLAT_SKILL_EXCLUDED_TOP_LEVEL
                        .iter()
                        .map(PathBuf::from)
                        .collect::<Vec<_>>()
                } else {
                    Vec::new()
                };
                fingerprint_authored_tree(
                    package_root,
                    &source,
                    &excluded,
                    &mut diag,
                    &mut entries,
                )?;
            }
        }
    }
    for path in crate::compiler::mcp::discover_mcp_definition_paths(package_root, &mut diag)? {
        fingerprint_authored_file(package_root, &path, &mut entries)?;
    }

    Ok(crate::hash::hash_file_manifest(
        entries.into_iter().collect(),
    ))
}

fn fingerprint_authored_tree(
    package_root: &Path,
    source_root: &Path,
    excluded_paths: &[PathBuf],
    diag: &mut DiagnosticCollector,
    entries: &mut std::collections::BTreeMap<String, String>,
) -> Result<(), MarsError> {
    for entry in crate::fs::authored_tree_entries(source_root, excluded_paths, diag)? {
        if entry.kind == crate::fs::AuthoredEntryKind::File {
            fingerprint_authored_file(package_root, &entry.source, entries)?;
        }
    }
    Ok(())
}

fn fingerprint_authored_file(
    package_root: &Path,
    path: &Path,
    entries: &mut std::collections::BTreeMap<String, String>,
) -> Result<(), MarsError> {
    let relative = crate::hash::portable_relative(package_root, path).ok_or_else(|| {
        MarsError::InvalidRequest {
            message: format!(
                "plugin input path must be valid Unicode and remain under the package root: {}",
                path.display()
            ),
        }
    })?;
    let bytes = std::fs::read(path)?;
    let normalized = std::str::from_utf8(&bytes).map_or_else(
        |_| std::borrow::Cow::Borrowed(bytes.as_slice()),
        |content| {
            std::borrow::Cow::Owned(
                content
                    .replace("\r\n", "\n")
                    .replace('\r', "\n")
                    .into_bytes(),
            )
        },
    );
    entries.insert(
        format!("@source/{relative}"),
        crate::hash::hash_bytes(&normalized),
    );
    Ok(())
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
        let node = &graph.nodes[source_name];
        let resolved = &node.resolved_ref;
        let version = resolved
            .version
            .as_ref()
            .map(ToString::to_string)
            .or_else(|| resolved.version_tag.clone())
            .or_else(|| {
                node.manifest
                    .as_ref()
                    .map(|manifest| manifest.package.version.clone())
            });
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
