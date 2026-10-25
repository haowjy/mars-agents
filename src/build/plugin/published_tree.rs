use std::fs;
use std::path::{Path, PathBuf};

use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;

/// Copy authored source into a published plugin tree through the same strict
/// walk used by plugin fingerprints and publish staging.
pub(super) fn copy(
    package_root: &Path,
    source_root: &Path,
    destination: &Path,
    excluded_paths: &[PathBuf],
    diag: &mut DiagnosticCollector,
) -> Result<(), MarsError> {
    fs::create_dir_all(destination)?;
    for entry in
        crate::fs::strict_authored_tree_entries(package_root, source_root, excluded_paths, diag)?
    {
        let output = destination.join(&entry.relative);
        match entry.kind {
            crate::fs::AuthoredEntryKind::Directory => fs::create_dir_all(&output)?,
            crate::fs::AuthoredEntryKind::File => {
                if let Some(parent) = output.parent() {
                    fs::create_dir_all(parent)?;
                }
                let bytes = fs::read(&entry.source)?;
                fs::write(&output, bytes)?;
                preserve_executable_bits(&entry.source, &output)?;
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn preserve_executable_bits(source: &Path, destination: &Path) -> Result<(), MarsError> {
    use std::os::unix::fs::PermissionsExt;

    let source_mode = fs::metadata(source)?.permissions().mode();
    fs::set_permissions(
        destination,
        fs::Permissions::from_mode(0o644 | (source_mode & 0o111)),
    )?;
    Ok(())
}

#[cfg(not(unix))]
fn preserve_executable_bits(_source: &Path, _destination: &Path) -> Result<(), MarsError> {
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    #[test]
    fn strict_publish_copy_confines_symlinks_and_materializes_safe_targets() {
        let package = tempfile::TempDir::new().unwrap();
        let outside = tempfile::TempDir::new().unwrap();
        let source = package.path().join("skills/example");
        let destination = package.path().join("output");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("inside.txt"), "inside").unwrap();
        fs::write(outside.path().join("secret.txt"), "secret").unwrap();
        symlink("inside.txt", source.join("safe.txt")).unwrap();
        symlink(outside.path().join("secret.txt"), source.join("escape.txt")).unwrap();

        let error = copy(
            package.path(),
            &source,
            &destination,
            &[],
            &mut DiagnosticCollector::new(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("escape.txt"));
        assert!(error.to_string().contains("leaves package root"));

        fs::remove_file(source.join("escape.txt")).unwrap();
        copy(
            package.path(),
            &source,
            &destination,
            &[],
            &mut DiagnosticCollector::new(),
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(destination.join("safe.txt")).unwrap(),
            "inside"
        );
        assert!(
            !fs::symlink_metadata(destination.join("safe.txt"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
}
