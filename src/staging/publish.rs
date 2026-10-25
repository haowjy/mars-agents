use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;

use crate::config::SkillOverlay;
use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::types::RenameMap;
use mars_schema::item::ItemKind;

use super::{
    DefinitionPreparation, StageOverlayContext, item_kind_for_markdown, process_markdown_file,
    should_lift_markdown,
};

#[derive(Debug, Clone)]
pub(crate) struct PublishInput {
    pub relative: PathBuf,
    pub excluded_paths: Vec<PathBuf>,
    pub item_kinds: std::collections::BTreeSet<ItemKind>,
}

pub(crate) fn discover_inputs(
    package_root: &Path,
    fallback_name: Option<&str>,
    diag: &mut DiagnosticCollector,
) -> Result<Vec<PublishInput>, MarsError> {
    let items = crate::discover::discover_resolved_source(package_root, fallback_name, diag)?;
    inputs_for_items(package_root, &items, true, diag)
}

pub(crate) fn inputs_for_items(
    package_root: &Path,
    items: &[crate::discover::DiscoveredItem],
    include_mcp: bool,
    diag: &mut DiagnosticCollector,
) -> Result<Vec<PublishInput>, MarsError> {
    let mut inputs =
        BTreeMap::<PathBuf, (Vec<PathBuf>, std::collections::BTreeSet<ItemKind>)>::new();
    for item in items {
        let excluded_paths =
            if item.id.kind == ItemKind::Skill && item.source_path == Path::new(".") {
                flat_skill_excluded_paths(package_root)?
            } else {
                Vec::new()
            };
        let entry = inputs
            .entry(item.source_path.clone())
            .or_insert_with(|| (excluded_paths, Default::default()));
        entry.1.insert(item.id.kind);
    }
    if include_mcp {
        for definition in crate::compiler::mcp::discover_mcp_definition_paths(package_root, diag)? {
            let relative = definition
                .strip_prefix(package_root)
                .expect("discovered MCP definition is inside its package")
                .to_path_buf();
            inputs
                .entry(relative)
                .or_default()
                .1
                .insert(ItemKind::McpServer);
        }
    }
    Ok(inputs
        .into_iter()
        .map(|(relative, (excluded_paths, item_kinds))| PublishInput {
            relative,
            excluded_paths,
            item_kinds,
        })
        .collect())
}

fn flat_skill_excluded_paths(package_root: &Path) -> Result<Vec<PathBuf>, MarsError> {
    const TOOLING_DIRECTORIES: &[&str] = &[
        ".direnv",
        ".venv",
        "__pycache__",
        "build",
        "dist",
        "node_modules",
        "target",
    ];
    let mut excluded = crate::fs::FLAT_SKILL_EXCLUDED_TOP_LEVEL
        .iter()
        .chain(TOOLING_DIRECTORIES)
        .map(PathBuf::from)
        .chain(crate::plugin_metadata::generated_root_directories().map(PathBuf::from))
        .chain(crate::plugin_metadata::generated_root_files().map(PathBuf::from))
        .collect::<Vec<_>>();
    for entry in fs::read_dir(package_root)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if !file_type.is_dir() && !file_type.is_file() && !file_type.is_symlink() {
            excluded.push(PathBuf::from(entry.file_name()));
        }
    }
    excluded.sort();
    excluded.dedup();
    Ok(excluded)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn copy_and_lift_inputs(
    source_root: &Path,
    dest_root: &Path,
    preparation: DefinitionPreparation,
    skill_overrides: &IndexMap<String, SkillOverlay>,
    renames: &RenameMap,
    fallback_skill_name: Option<&str>,
    inputs: &[PublishInput],
    diag: &mut DiagnosticCollector,
) -> Result<bool, MarsError> {
    // These files decide how the staged tree is rediscovered. Validate them
    // before discovery follows any symlink they may contain.
    let mars_manifest = source_root.join("mars.toml");
    let mut legacy_changed = false;
    if mars_manifest.is_file() {
        let source = crate::fs::strict_authored_file(source_root, &mars_manifest)?;
        let destination = dest_root.join("mars.toml");
        if preparation == DefinitionPreparation::Legacy {
            let original = fs::read_to_string(&source)?;
            let upgraded = mars_schema::upgrade::upgrade_manifest(&original).map_err(|error| {
                MarsError::Source {
                    source_name: "legacy package".to_string(),
                    message: format!("failed to upgrade {}: {error}", source.display()),
                }
            })?;
            legacy_changed |= upgraded.changed_from(&original);
            fs::write(destination, upgraded.text)?;
        } else {
            fs::copy(&source, destination)?;
        }
    } else {
        let selected = inputs
            .iter()
            .flat_map(|input| {
                input
                    .item_kinds
                    .iter()
                    .map(|kind| (*kind, input.relative.clone()))
            })
            .collect::<Vec<_>>();
        for relative in crate::discover::SOURCE_MANIFEST_PATHS {
            copy_filtered_source_manifest(source_root, dest_root, relative, &selected)?;
        }
    }

    let mut ctx = StageOverlayContext {
        preparation,
        skill_overrides,
        renames,
        package_root: source_root,
        fallback_skill_name,
        skill_overlay_key: None,
        diag,
    };
    for input in inputs {
        legacy_changed |= copy_and_lift_path(source_root, input, dest_root, &mut ctx)?;
    }
    Ok(legacy_changed)
}

fn copy_filtered_source_manifest(
    source_root: &Path,
    dest_root: &Path,
    relative: &str,
    selected: &[(ItemKind, PathBuf)],
) -> Result<(), MarsError> {
    let logical_source = source_root.join(relative);
    if !logical_source.is_file() {
        return Ok(());
    }
    let source = crate::fs::strict_authored_file(source_root, &logical_source)?;
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(source)?).map_err(|error| MarsError::Source {
            source_name: "plugin publication staging".to_string(),
            message: format!(
                "failed to parse source plugin manifest `{}`: {error}",
                logical_source.display()
            ),
        })?;
    crate::discover::retain_selected_manifest_declarations(&mut manifest, selected);
    let destination = dest_root.join(relative);
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        destination,
        serde_json::to_vec_pretty(&manifest).expect("JSON values serialize"),
    )?;
    Ok(())
}

fn copy_and_lift_path(
    package_root: &Path,
    input: &PublishInput,
    dest_root: &Path,
    ctx: &mut StageOverlayContext<'_>,
) -> Result<bool, MarsError> {
    let relative_root = &input.relative;
    let logical_root = package_root.join(relative_root);
    if fs::metadata(&logical_root)?.is_dir() {
        fs::create_dir_all(dest_root.join(relative_root))?;
        let mut changed = false;
        for entry in crate::fs::authored_tree_entries(
            &logical_root,
            &input.excluded_paths,
            crate::fs::AuthoredSymlinkPolicy::ConfineTo(package_root),
            ctx.diag,
        )? {
            let relative = relative_root.join(&entry.relative);
            let destination = dest_root.join(&relative);
            if entry.kind == crate::fs::AuthoredEntryKind::Directory {
                fs::create_dir_all(destination)?;
            } else {
                changed |= copy_and_lift_file(&entry.source, &relative, dest_root, ctx)?;
            }
        }
        Ok(changed)
    } else {
        let source = crate::fs::strict_authored_file(package_root, &logical_root)?;
        copy_and_lift_file(&source, relative_root, dest_root, ctx)
    }
}

fn copy_and_lift_file(
    source: &Path,
    relative: &Path,
    dest_root: &Path,
    ctx: &mut StageOverlayContext<'_>,
) -> Result<bool, MarsError> {
    let destination = dest_root.join(relative);
    if should_lift_markdown(relative) {
        process_markdown_file(
            source,
            &ctx.package_root.join(relative),
            &destination,
            item_kind_for_markdown(relative),
            ctx,
        )
    } else {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(source, destination)?;
        Ok(false)
    }
}
