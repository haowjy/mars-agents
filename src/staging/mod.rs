//! Filesystem projection of crate-owned prepared packages.
//!
//! Schema classification, discovery, normalization, and reading happen in
//! `mars-schema`; this module only copies prepared bytes and binary-owned payload.

mod overlay;
mod publish;

pub(crate) use publish::inputs_for_items as publish_inputs_for_items;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use indexmap::IndexMap;

use crate::config::SkillOverlay;
use crate::diagnostic::DiagnosticCollector;
use crate::dialect::Dialect;
use crate::error::MarsError;
use crate::platform::cache::safe_component;
use crate::resolve::RootedSourceRef;
use crate::types::{RenameMap, SourceName};
use mars_schema::frontmatter::Frontmatter;
use mars_schema::item::ItemKind;

pub(crate) use overlay::{apply_skill_overlay, skill_overlay_lookup_name};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HookSurfaceState {
    Readable,
    Unreadable {
        hook_names: std::collections::BTreeSet<String>,
    },
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn materialize_prepared_rooted(
    source_name: &SourceName,
    rooted: RootedSourceRef,
    prepared: Arc<mars_schema::prepare::Prepared>,
    skill_overrides: &IndexMap<String, SkillOverlay>,
    renames: &RenameMap,
    staging_root: &Path,
    diag: &mut DiagnosticCollector,
) -> Result<(RootedSourceRef, HookSurfaceState), MarsError> {
    let destination = staging_dir_for(staging_root, source_name);
    materialize_prepared_tree(
        &rooted.authored_root,
        &destination,
        &prepared,
        skill_overrides,
        renames,
        Some(source_name.as_ref()),
        diag,
    )?;
    let hook_surface = detect_unreadable_hook_surface(&destination, diag)?;
    Ok((
        RootedSourceRef {
            checkout_root: rooted.checkout_root,
            authored_root: rooted.authored_root,
            package_root: destination,
        },
        hook_surface,
    ))
}

/// Materialize a completely prepared graph after its diagnostic gate.
pub(crate) fn materialize_prepared_graph(
    graph: &mut crate::resolve::ResolvedGraph,
    effective: &crate::config::EffectiveConfig,
    staging_root: &Path,
    diag: &mut DiagnosticCollector,
) -> Result<(), MarsError> {
    let empty_renames = RenameMap::new();
    for (source_name, node) in &mut graph.nodes {
        let prepared = node
            .prepared
            .as_ref()
            .expect("production graph nodes retain prepared packages");
        let renames = effective
            .dependencies
            .get(source_name)
            .map(|dependency| &dependency.rename)
            .unwrap_or(&empty_renames);
        let (rooted, _) = materialize_prepared_rooted(
            source_name,
            node.rooted_ref.clone(),
            Arc::clone(prepared),
            &effective.skills,
            renames,
            staging_root,
            diag,
        )?;
        node.rooted_ref = rooted;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn materialize_prepared_tree(
    authored_root: &Path,
    destination: &Path,
    prepared: &mars_schema::prepare::Prepared,
    skill_overrides: &IndexMap<String, SkillOverlay>,
    renames: &RenameMap,
    fallback_skill_name: Option<&str>,
    diag: &mut DiagnosticCollector,
) -> Result<(), MarsError> {
    if destination.symlink_metadata().is_ok() {
        crate::platform::fs::safe_remove(destination)?;
    }
    fs::create_dir_all(destination)?;
    if let Some(manifest) = &prepared.manifest {
        fs::write(
            destination.join(&manifest.output_path),
            &manifest.normalized_bytes,
        )?;
    }
    for definition in &prepared.definitions {
        let mut bytes = definition.normalized_bytes.clone();
        if definition.kind == mars_schema::upgrade::DefinitionKind::Skill {
            let logical = authored_root.join(&definition.source_path);
            let installed_name =
                skill_overlay_lookup_name(&logical, authored_root, renames, fallback_skill_name);
            if let Some(overlay) = installed_name
                .as_deref()
                .and_then(|name| skill_overrides.get(name))
                && let Ok(text) = std::str::from_utf8(&bytes)
                && let Ok(frontmatter) = Frontmatter::parse(text)
            {
                let (frontmatter, changed) = apply_skill_overlay(&frontmatter, overlay);
                if changed {
                    bytes = frontmatter.render().into_bytes();
                }
            }
        }
        let output = destination.join(&definition.output_path);
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(output, bytes)?;
    }
    for payload in &prepared.payload {
        let source = crate::fs::strict_authored_file(
            authored_root,
            &authored_root.join(&payload.source_path),
        )?;
        let output = destination.join(&payload.output_path);
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(source, output)?;
    }
    // Hooks, bootstrap documents, MCP definitions and foreign source manifests
    // remain binary-owned package surfaces.
    for relative in ["hooks", "mcp"] {
        copy_authored_subtree(authored_root, Path::new(relative), destination, diag)?;
    }
    for item in crate::discover::discover_bootstrap_surfaces(authored_root, diag)? {
        copy_authored_subtree(authored_root, &item.source_path, destination, diag)?;
    }
    for relative in crate::discover::SOURCE_MANIFEST_PATHS {
        let source = authored_root.join(relative);
        if source.is_file() {
            let source = crate::fs::strict_authored_file(authored_root, &source)?;
            let output = destination.join(relative);
            if let Some(parent) = output.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(source, output)?;
        }
    }
    Ok(())
}

fn copy_authored_subtree(
    authored_root: &Path,
    relative: &Path,
    destination: &Path,
    diag: &mut DiagnosticCollector,
) -> Result<(), MarsError> {
    let root = authored_root.join(relative);
    if !root.is_dir() {
        return Ok(());
    }
    fs::create_dir_all(destination.join(relative))?;
    for entry in crate::fs::authored_tree_entries(
        &root,
        &[],
        crate::fs::AuthoredSymlinkPolicy::Follow,
        diag,
    )? {
        let output = destination.join(relative).join(&entry.relative);
        match entry.kind {
            crate::fs::AuthoredEntryKind::Directory => fs::create_dir_all(output)?,
            crate::fs::AuthoredEntryKind::File => {
                if let Some(parent) = output.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(entry.source, output)?;
            }
        }
    }
    Ok(())
}

/// Compatibility entry used by lossiness preview and discovery fixtures. It
/// delegates semantic work to `mars-schema` and only materializes the result.
pub(crate) fn stage_canonical_source(
    source_root: &Path,
    dest_root: &Path,
    dialect: Dialect,
    skill_overrides: &IndexMap<String, SkillOverlay>,
    renames: &RenameMap,
    fallback_skill_name: Option<&str>,
    diag: &mut DiagnosticCollector,
) -> Result<(), MarsError> {
    let consumer_manifest = fs::read_to_string(source_root.join("mars.toml"))
        .ok()
        .and_then(|content| content.parse::<toml::Value>().ok())
        .is_some_and(|manifest| manifest.get("package").is_none());
    let prepared = if consumer_manifest {
        crate::package_prepare::prepare_authored_without_manifest(
            source_root,
            mars_schema::prepare::PrepareContext::AuthoredLocal,
            Some(dialect),
            diag,
        )?
    } else {
        crate::package_prepare::prepare_authored(
            source_root,
            mars_schema::prepare::PrepareContext::Package,
            Some(dialect),
            diag,
        )?
    };
    materialize_prepared_tree(
        source_root,
        dest_root,
        &prepared,
        skill_overrides,
        renames,
        fallback_skill_name,
        diag,
    )
}

pub(crate) fn materialize_local_prepared(
    item: &crate::local_source::LocalDiscoveredItem,
    skill_overrides: &IndexMap<String, SkillOverlay>,
    staging_root: &Path,
    item_key: &str,
    excluded_paths: &[PathBuf],
) -> Result<PathBuf, MarsError> {
    let destination = staging_root.join("_local").join(safe_component(item_key));
    if destination.symlink_metadata().is_ok() {
        crate::platform::fs::safe_remove(&destination)?;
    }
    fs::create_dir_all(&destination)?;

    let mut definition_bytes = item.definition.normalized_bytes.clone();
    if item.discovered.id.kind == ItemKind::Skill
        && let Some(overlay) = skill_overrides.get(item.discovered.id.name.as_str())
        && let Ok(text) = std::str::from_utf8(&definition_bytes)
        && let Ok(frontmatter) = Frontmatter::parse(text)
    {
        let (frontmatter, changed) = apply_skill_overlay(&frontmatter, overlay);
        if changed {
            definition_bytes = frontmatter.render().into_bytes();
        }
    }
    let result = match item.discovered.id.kind {
        ItemKind::Agent => {
            let name = Path::new(&item.definition.output_path)
                .file_name()
                .expect("agent output has a filename");
            let path = destination.join(name);
            fs::write(&path, definition_bytes)?;
            path
        }
        ItemKind::Skill => {
            fs::write(destination.join("SKILL.md"), definition_bytes)?;
            for payload in &item.payload {
                let relative = PathBuf::from(&payload.source_path);
                let relative = if item.discovered.source_path == Path::new(".") {
                    relative
                } else {
                    relative
                        .strip_prefix(&item.discovered.source_path)
                        .expect("skill payload belongs to selected skill")
                        .to_path_buf()
                };
                if excluded_paths
                    .iter()
                    .any(|excluded| relative.starts_with(excluded))
                {
                    continue;
                }
                let source = crate::fs::strict_authored_file(
                    &item.root,
                    &item.root.join(&payload.source_path),
                )?;
                let output = destination.join(relative);
                if let Some(parent) = output.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(source, output)?;
            }
            destination
        }
        _ => unreachable!("prepared local definitions are agents or skills"),
    };
    Ok(result)
}

fn staging_dir_for(staging_root: &Path, source_name: &SourceName) -> PathBuf {
    staging_root.join(safe_component(source_name.as_ref()))
}

pub(crate) fn detect_unreadable_hook_surface(
    package_root: &Path,
    diag: &mut DiagnosticCollector,
) -> Result<HookSurfaceState, MarsError> {
    let hook_dirs = crate::discover::discover_hook_directories(package_root, diag)?;
    let mut hook_names = std::collections::BTreeSet::new();
    for hook_dir in hook_dirs {
        let manifest = hook_dir.join("hook.toml");
        let raw = fs::read_to_string(&manifest)?;
        let Ok(value) = toml::from_str::<toml::Value>(&raw) else {
            continue;
        };
        if crate::compiler::hooks::uses_removed_schema(&value)
            && let Some(hook_name) = hook_dir.file_name().and_then(|name| name.to_str())
        {
            hook_names.insert(hook_name.to_owned());
        }
    }
    if hook_names.is_empty() {
        Ok(HookSurfaceState::Readable)
    } else {
        Ok(HookSurfaceState::Unreadable { hook_names })
    }
}
