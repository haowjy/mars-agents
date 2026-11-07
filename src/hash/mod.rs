use std::fs;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::error::MarsError;
use mars_schema::item::ItemKind;

/// A bootstrap doc is hashed as its directory (`bootstrap/<name>/`), whether the caller holds
/// that directory or its `BOOTSTRAP.md`, so an installed doc's path hashes the way the lock
/// recorded it.
pub fn compute_hash(path: &Path, kind: ItemKind) -> Result<String, MarsError> {
    match kind {
        ItemKind::Agent => hash_file(path),
        ItemKind::Skill | ItemKind::Hook | ItemKind::McpServer => compute_dir_hash(path),
        ItemKind::BootstrapDoc => {
            let is_doc_file = path.file_name().is_some_and(|name| name == "BOOTSTRAP.md");
            compute_dir_hash(path.parent().filter(|_| is_doc_file).unwrap_or(path))
        }
    }
}

/// SHA-256 of one file's content; what the lock records for an agent or a generated hook file.
pub(crate) fn hash_file(path: &Path) -> Result<String, MarsError> {
    let content = fs::read(path).map_err(|error| MarsError::io("read", path, error))?;
    Ok(hash_bytes(&content))
}

pub fn compute_skill_hash_filtered(
    dir: &Path,
    excluded_top_level: &[&str],
) -> Result<String, MarsError> {
    compute_dir_hash_filtered(dir, excluded_top_level)
}

pub(crate) fn compute_skill_hash_with_override(
    dir: &Path,
    relative_path: &str,
    content: &[u8],
    excluded_top_level: &[&str],
) -> Result<String, MarsError> {
    let mut entries = Vec::new();
    collect_file_hashes(dir, dir, &mut entries, excluded_top_level)?;
    let Some((_, hash)) = entries.iter_mut().find(|(path, _)| path == relative_path) else {
        return Err(MarsError::InvalidRequest {
            message: format!(
                "skill tree {} does not contain {relative_path}",
                dir.display()
            ),
        });
    };
    *hash = hash_bytes(content);
    Ok(hash_file_manifest(entries))
}

pub fn hash_bytes(content: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content);
    let digest = hasher.finalize();
    format!("sha256:{:064x}", digest)
}

pub(crate) fn compute_dir_hash(dir: &Path) -> Result<String, MarsError> {
    compute_dir_hash_filtered(dir, &[])
}

fn compute_dir_hash_filtered(dir: &Path, excluded_top_level: &[&str]) -> Result<String, MarsError> {
    let mut entries: Vec<(String, String)> = Vec::new();
    collect_file_hashes(dir, dir, &mut entries, excluded_top_level)?;
    Ok(hash_file_manifest(entries))
}

pub(crate) fn hash_file_manifest(mut entries: Vec<(String, String)>) -> String {
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut manifest = String::new();
    for (rel_path, hash) in &entries {
        manifest.push_str(rel_path);
        manifest.push(':');
        manifest.push_str(hash);
        manifest.push('\n');
    }

    hash_bytes(manifest.as_bytes())
}

pub(crate) fn strict_file_hashes(
    root: &Path,
    excluded_top_level: &[&str],
) -> Result<std::collections::BTreeMap<String, String>, MarsError> {
    let snapshot =
        match crate::platform::fs::directory_tree_snapshot_filtered(root, excluded_top_level)? {
            crate::platform::fs::DirectoryTreeSnapshotResult::Snapshot(snapshot)
                if snapshot.is_strict() =>
            {
                snapshot
            }
            crate::platform::fs::DirectoryTreeSnapshotResult::NonUnicode(path) => {
                return Err(MarsError::InvalidRequest {
                    message: format!("path must be valid Unicode: {}", path.display()),
                });
            }
            crate::platform::fs::DirectoryTreeSnapshotResult::Snapshot(_)
            | crate::platform::fs::DirectoryTreeSnapshotResult::Unavailable => {
                return Err(MarsError::InvalidRequest {
                    message: format!(
                        "directory tree must contain only directories and regular files: {}",
                        root.display()
                    ),
                });
            }
        };
    Ok(snapshot.file_hash_manifest_entries().into_iter().collect())
}

pub(crate) fn portable_relative(root: &Path, path: &Path) -> Option<String> {
    path.strip_prefix(root)
        .ok()?
        .components()
        .map(|component| component.as_os_str().to_str().map(str::to_owned))
        .collect::<Option<Vec<_>>>()
        .map(|components| components.join("/"))
}

fn collect_file_hashes(
    root: &Path,
    current: &Path,
    entries: &mut Vec<(String, String)>,
    excluded_top_level: &[&str],
) -> Result<(), MarsError> {
    for entry in
        fs::read_dir(current).map_err(|error| MarsError::io("read directory", current, error))?
    {
        let entry = entry.map_err(|error| MarsError::io("read directory", current, error))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| MarsError::io("inspect", &path, error))?;

        let rel_path = path.strip_prefix(root).expect("path is always under root");
        if is_excluded_top_level(rel_path, excluded_top_level) {
            continue;
        }
        if file_type.is_dir() {
            collect_file_hashes(root, &path, entries, excluded_top_level)?;
        } else {
            // Follow symlinks so a linked file keeps hashing like the file it
            // names, but never open what is not a file: reading a FIFO blocks.
            let target_type = fs::metadata(&path)
                .map_err(|error| MarsError::io("inspect", &path, error))?
                .file_type();
            if !target_type.is_file() {
                return Err(MarsError::InvalidRequest {
                    message: format!(
                        "`{}` is not a regular file; a skill, hook or MCP server tree may contain only directories and regular files",
                        path.display()
                    ),
                });
            }
            let rel_path = portable_relative(root, &path)
                .unwrap_or_else(|| rel_path.to_string_lossy().replace('\\', "/"));
            let content = fs::read(&path).map_err(|error| MarsError::io("read", &path, error))?;
            let hash = hash_bytes(&content);
            entries.push((rel_path, hash));
        }
    }
    Ok(())
}

fn is_excluded_top_level(path: &Path, excluded_top_level: &[&str]) -> bool {
    let Some(first) = path.components().next().map(|c| c.as_os_str()) else {
        return false;
    };
    excluded_top_level.iter().any(|excluded| first == *excluded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[cfg(unix)]
    #[test]
    fn dir_hash_rejects_a_fifo_without_opening_it() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join("skill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "content").unwrap();
        let fifo = skill_dir.join("events");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );

        // Opening a FIFO blocks until something writes to it, so a walk that
        // stops rejecting special files would hang the suite instead of failing
        // it. Hashing gets its own thread and a deadline.
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(compute_dir_hash(&skill_dir).map_err(|error| error.to_string()));
        });
        let error = receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("hashing a tree with a FIFO must finish instead of blocking")
            .expect_err("a FIFO is not a regular file");
        assert!(
            error.contains(&fifo.display().to_string()),
            "error must name the FIFO, got: {error}"
        );
    }
}
