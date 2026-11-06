//! Tests for the binary-owned discovery of bootstrap documents (and hooks): bootstrap grounding
//! and plugin-manifest declarations. Agents and skills are selected by `mars-schema`.

use super::*;
use std::fs;
use tempfile::TempDir;

fn discover_source(
    root: &Path,
    name: Option<&str>,
    diag: &mut DiagnosticCollector,
) -> Result<Vec<DiscoveredItem>, MarsError> {
    discover_binary_owned_items(root, name, None, diag)
}

#[test]
fn manifest_declared_bootstrap_escape_is_rejected() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join(".claude-plugin")).unwrap();
    fs::write(
        dir.path().join(".claude-plugin/plugin.json"),
        r#"{"bootstrapDocs":[{"path":"./../escape"}]}"#,
    )
    .unwrap();

    let err =
        discover_source(dir.path(), Some("demo"), &mut DiagnosticCollector::new()).unwrap_err();
    assert!(matches!(err, MarsError::ManifestDeclaredPathEscape { .. }));
}
