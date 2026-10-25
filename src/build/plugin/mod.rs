//! Build installable harness plugin trees from a package's publish graph.

mod inspect;
mod metadata;
mod model;
pub mod output;
mod references;
mod root_files;
pub mod support;
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
pub use crate::plugin::PluginTree;
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

pub(crate) fn configured_output_path(
    package_root: &Path,
    plugin: &crate::config::PluginConfig,
) -> Result<PathBuf, MarsError> {
    crate::config::validate_plugin_dir(&plugin.dir)?;
    let package_root = crate::fs::canonicalize_existing_prefix(package_root)?;
    let output = std::path::absolute(package_root.join(&plugin.dir))?;
    let identity = crate::fs::canonicalize_existing_prefix(&output)?;
    if identity == package_root || !identity.starts_with(&package_root) {
        return Err(MarsError::InvalidRequest {
            message: format!(
                "[plugin].dir `{}` must resolve inside the package root",
                plugin.dir.display()
            ),
        });
    }
    let canonical_relative = identity
        .strip_prefix(&package_root)
        .expect("contained plugin output has a package-relative path");
    crate::config::validate_plugin_dir(canonical_relative)?;
    Ok(output)
}

fn paths_resolve_equal(left: &Path, right: &Path) -> Result<bool, MarsError> {
    Ok(crate::fs::canonicalize_existing_prefix(left)?
        == crate::fs::canonicalize_existing_prefix(right)?)
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

    pub(crate) fn deleted_root_files(&self) -> Vec<PathBuf> {
        self.root_files.deleted_paths()
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

pub(crate) fn validate_plugin_build(
    config: &Config,
    harnesses: &[PluginHarness],
) -> Result<(), MarsError> {
    let package = config
        .package
        .as_ref()
        .ok_or_else(|| ConfigError::Invalid {
            message: "mars.toml needs a [package] table to build a plugin".to_string(),
        })?;
    if harnesses.is_empty() {
        return Err(ConfigError::Invalid {
            message: "a plugin build needs at least one harness".to_string(),
        }
        .into());
    }
    if let Some(message) = crate::plugin::unknown_plugin_harness_messages(harnesses)
        .into_iter()
        .next()
    {
        return Err(ConfigError::Invalid { message }.into());
    }
    let specs = harnesses
        .iter()
        .map(|harness| harness.spec().expect("unknown harnesses were rejected"))
        .collect::<Vec<_>>();
    if let Some((index, _)) = package
        .authors
        .iter()
        .enumerate()
        .find(|(_, author)| author.name.trim().is_empty())
    {
        return Err(ConfigError::Invalid {
            message: format!("mars.toml [package].authors[{index}].name cannot be empty"),
        }
        .into());
    }

    let name = config
        .plugin
        .as_ref()
        .map_or(package.name.as_str(), |plugin| plugin.name(package));
    let name_field = if config
        .plugin
        .as_ref()
        .is_some_and(|plugin| plugin.name.is_some())
    {
        "[plugin].name"
    } else {
        "[package].name"
    };
    for spec in &specs {
        if !spec.name_rule.accepts(name) {
            return Err(ConfigError::Invalid {
                message: format!(
                    "mars.toml {name_field} `{name}` is invalid: {}",
                    spec.name_rule.message
                ),
            }
            .into());
        }
    }

    let marketplace = config
        .plugin
        .as_ref()
        .map_or(package.name.as_str(), |plugin| plugin.marketplace(package));
    let marketplace_field = if config
        .plugin
        .as_ref()
        .is_some_and(|plugin| plugin.marketplace.is_some())
    {
        "[plugin].marketplace"
    } else {
        "[package].name"
    };
    let mut checked = std::collections::BTreeSet::new();
    for spec in specs {
        if !checked.insert(spec.tree.as_str()) {
            continue;
        }
        let Some(rule) = spec.tree.spec().marketplace_name_rule else {
            continue;
        };
        if !rule.accepts(marketplace) {
            return Err(ConfigError::Invalid {
                message: format!(
                    "mars.toml {marketplace_field} `{marketplace}` is invalid as a marketplace: {}",
                    rule.message
                ),
            }
            .into());
        }
    }
    Ok(())
}

pub fn build_plugin(request: &PluginBuildRequest) -> Result<PluginBuildReport, MarsError> {
    stage_plugin(request)?.publish()
}

pub fn stage_plugin(request: &PluginBuildRequest) -> Result<StagedPluginBuild, MarsError> {
    let mut diag = DiagnosticCollector::with_lossiness_mode(LossinessMode::Verbose);
    match stage_plugin_inner(request, &mut diag) {
        Ok(staged) => Ok(staged),
        Err(source) => {
            let diagnostics = diag.drain();
            if diagnostics.is_empty() {
                Err(source)
            } else {
                Err(MarsError::WithDiagnostics {
                    diagnostics,
                    source: Box::new(source),
                })
            }
        }
    }
}

fn stage_plugin_inner(
    request: &PluginBuildRequest,
    diag: &mut DiagnosticCollector,
) -> Result<StagedPluginBuild, MarsError> {
    let package_root =
        dunce::canonicalize(&request.package_root).map_err(|source| MarsError::Io {
            operation: "resolve plugin package root".to_string(),
            path: request.package_root.clone(),
            source,
        })?;
    let lens = PublishLens::load(&package_root, diag)?;
    let manifest = lens.manifest.as_ref().ok_or_else(|| {
        MarsError::Config(ConfigError::Invalid {
            message: format!(
                "{} is not a publishable package: mars.toml needs a [package] table",
                package_root.display()
            ),
        })
    })?;
    validate_plugin_build(manifest, &request.harnesses)?;
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
    let profiles = requested_tree_profiles(&request.harnesses);

    let graph_staging = tempfile::TempDir::new()?;
    let effective = implicit_consumer_config(&lens, &root_source, &profiles)?;
    let mut graph = crate::resolve::resolve_publish_graph_from_config(
        &lens,
        &effective,
        &ResolveOptions::default(),
        diag,
    )?;
    stage_graph(
        &mut graph,
        &effective,
        &graph_staging.path().join("graph"),
        diag,
    )?;
    let mut prepared_target =
        crate::sync::prepare_target_state(&graph, &effective, diag, |_, _| Ok(()))?;
    emit_missing_skill_warnings(&prepared_target.warnings, diag);
    materialize_rewritten_skills(&mut prepared_target.target)?;

    let aliases = crate::models::merged_model_aliases(
        &graph,
        &effective,
        &Config::default(),
        &LocalConfig::default(),
        diag,
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
    tree::check_sources(&tree_input, &profiles, diag)?;
    let (staged_output, (built_trees, diagnostics, root_file_plan)) =
        output::stage(&out_dir, |staging| {
            let inputs_hash = plugin_inputs_hash(&package_root)?;
            let mut built_trees = Vec::with_capacity(profiles.len());
            for profile in &profiles {
                let tree_root = staging.join(profile.tree().as_str());
                built_trees.push(tree::write_tree(&tree_root, &tree_input, profile, diag)?);
            }
            support::emit(
                &request.harnesses,
                &prepared_target.target,
                &built_trees,
                diag,
            );
            let root_file_plan = root_files::plan(
                &package_root,
                &out_dir,
                staging,
                &request.destination,
                &metadata,
                &request.harnesses,
                diag,
            )?;
            let collected = diag.drain();
            if let Err(error) = fail_on_diagnostic_errors(&collected) {
                diag.extend(collected);
                return Err(error);
            }
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
                    root_directories: root_file_plan.root_directories.clone(),
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

fn materialize_rewritten_skills(
    target: &mut crate::sync::target::TargetState,
) -> Result<(), MarsError> {
    for item in target.items.values_mut() {
        if item.id.kind != crate::lock::ItemKind::Skill {
            continue;
        }
        if item.is_flat_skill {
            for excluded in crate::fs::FLAT_SKILL_EXCLUDED_TOP_LEVEL {
                crate::platform::fs::safe_remove(&item.source_path.join(excluded))?;
            }
        }
        let Some(content) = item.rewritten_content.take() else {
            continue;
        };
        crate::fs::atomic_write_preserving_permissions(
            &item.source_path.join("SKILL.md"),
            content.as_bytes(),
        )?;
    }
    Ok(())
}

fn requested_tree_profiles(harnesses: &[PluginHarness]) -> Vec<&'static tree::TreeProfile> {
    tree::profiles()
        .filter(|profile| {
            harnesses.iter().any(|harness| {
                harness
                    .spec()
                    .is_some_and(|spec| spec.tree == profile.tree())
            })
        })
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
        let rooted = graph
            .nodes
            .get(&source_name)
            .expect("resolved graph order names an existing node")
            .rooted_ref
            .clone();
        let discovered = crate::discover::discover_resolved_source(
            &rooted.package_root,
            Some(source_name.as_str()),
            diag,
        )?;
        let mut selected = crate::sync::target::apply_source_filters(
            graph,
            &source_name,
            effective.dependencies.get(&source_name),
            &discovered,
            &rooted.package_root,
        )?
        .unwrap_or_default();
        // Hook manifests are parsed strictly before source filters are applied.
        // Keep every hook root in the staged tree so removed schemas and other
        // compatibility errors cannot be hidden by an unrelated item filter.
        for item in discovered
            .iter()
            .filter(|item| item.id.kind == crate::lock::ItemKind::Hook)
        {
            if !selected.contains(item) {
                selected.push(item.clone());
            }
        }
        let include_mcp = crate::sync::target::source_may_emit_mcp(graph, &source_name);
        let inputs = crate::staging::publish_inputs_for_items(
            &rooted.package_root,
            &selected,
            include_mcp,
            diag,
        )?;
        let staged = crate::resolve::stage_rooted_package_for_publish_inputs(
            &source_name,
            rooted,
            effective,
            staging_root,
            &inputs,
            diag,
        )?;
        let node = graph
            .nodes
            .get_mut(&source_name)
            .expect("resolved graph order names an existing node");
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
    for input in crate::staging::discover_publish_inputs(package_root, None, &mut diag)? {
        let source = package_root.join(&input.relative);
        let relative = crate::hash::portable_relative(package_root, &source).ok_or_else(|| {
            MarsError::InvalidRequest {
                message: format!(
                    "plugin input path must be valid Unicode and remain under the package root: {}",
                    source.display()
                ),
            }
        })?;
        for kind in &input.item_kinds {
            entries.insert(
                format!("@inventory/{kind}/{relative}"),
                crate::hash::hash_bytes(kind.to_string().as_bytes()),
            );
        }
        if std::fs::metadata(&source)?.is_dir() {
            fingerprint_authored_tree(
                package_root,
                &source,
                &input.excluded_paths,
                &mut diag,
                &mut entries,
            )?;
        } else {
            fingerprint_authored_file(package_root, &source, &mut entries)?;
        }
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
    for entry in crate::fs::authored_tree_entries(
        source_root,
        excluded_paths,
        crate::fs::AuthoredSymlinkPolicy::ConfineTo(package_root),
        diag,
    )? {
        if entry.kind == crate::fs::AuthoredEntryKind::File {
            fingerprint_authored_file_from(
                package_root,
                &source_root.join(&entry.relative),
                &entry.source,
                entries,
            )?;
        }
    }
    Ok(())
}

fn fingerprint_authored_file(
    package_root: &Path,
    path: &Path,
    entries: &mut std::collections::BTreeMap<String, String>,
) -> Result<(), MarsError> {
    let resolved = crate::fs::strict_authored_file(package_root, path)?;
    fingerprint_authored_file_from(package_root, path, &resolved, entries)
}

fn fingerprint_authored_file_from(
    package_root: &Path,
    logical_path: &Path,
    source_path: &Path,
    entries: &mut std::collections::BTreeMap<String, String>,
) -> Result<(), MarsError> {
    let relative = crate::hash::portable_relative(package_root, logical_path).ok_or_else(|| {
        MarsError::InvalidRequest {
            message: format!(
                "plugin input path must be valid Unicode and remain under the package root: {}",
                logical_path.display()
            ),
        }
    })?;
    let bytes = std::fs::read(source_path)?;
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
