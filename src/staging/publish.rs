use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use mars_schema::item::ItemKind;

#[derive(Debug, Clone)]
pub(crate) struct PublishInput {
    pub relative: PathBuf,
    pub excluded_paths: Vec<PathBuf>,
    pub item_kinds: std::collections::BTreeSet<ItemKind>,
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
