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
fn grounding_uses_registering_bootstrap_container_layer_not_first_bootstrap_segment() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("outer/bootstrap/foo/bootstrap/bar")).unwrap();
    fs::write(dir.path().join("outer/bootstrap/foo/BOOTSTRAP.md"), "# foo").unwrap();
    fs::write(
        dir.path()
            .join("outer/bootstrap/foo/bootstrap/bar/BOOTSTRAP.md"),
        "# bar",
    )
    .unwrap();

    let items = discover_source(dir.path(), Some("demo"), &mut DiagnosticCollector::new()).unwrap();

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id.kind, ItemKind::BootstrapDoc);
    assert_eq!(items[0].id.name.as_str(), "foo");
    assert_eq!(items[0].source_path, PathBuf::from("outer/bootstrap/foo"));
}

#[test]
fn conventional_discovery_finds_package_bootstrap_docs() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("bootstrap/global-auth")).unwrap();
    fs::create_dir_all(dir.path().join("bootstrap/.hidden")).unwrap();
    fs::write(
        dir.path().join("bootstrap/global-auth/BOOTSTRAP.md"),
        "# auth",
    )
    .unwrap();
    fs::write(dir.path().join("bootstrap/.hidden/BOOTSTRAP.md"), "# hide").unwrap();

    let items = discover_source(dir.path(), None, &mut DiagnosticCollector::new()).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id.kind, ItemKind::BootstrapDoc);
    assert_eq!(items[0].id.name.as_str(), "global-auth");
    assert_eq!(items[0].source_path, PathBuf::from("bootstrap/global-auth"));
}

#[test]
fn conventional_bootstrap_discovery_ignores_missing_bootstrap_file() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("bootstrap/incomplete")).unwrap();
    fs::write(
        dir.path().join("bootstrap/incomplete/README.md"),
        "# readme",
    )
    .unwrap();

    let items = discover_source(dir.path(), None, &mut DiagnosticCollector::new()).unwrap();
    assert!(items.is_empty());
}

#[test]
fn fallback_manifest_declares_bootstrap_docs() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("docs/global-auth")).unwrap();
    fs::write(dir.path().join("docs/global-auth/BOOTSTRAP.md"), "# auth").unwrap();
    fs::create_dir_all(dir.path().join(".claude-plugin")).unwrap();
    fs::write(
        dir.path().join(".claude-plugin/plugin.json"),
        r#"{"bootstrapDocs":[{"path":"./docs/global-auth"}]}"#,
    )
    .unwrap();

    let items = discover_source(dir.path(), Some("demo"), &mut DiagnosticCollector::new()).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id.kind, ItemKind::BootstrapDoc);
    assert_eq!(items[0].id.name.as_str(), "global-auth");
    assert_eq!(items[0].source_path, PathBuf::from("docs/global-auth"));
}

#[test]
fn fallback_manifest_declares_bootstrap_container() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("bootstrap/setup")).unwrap();
    fs::write(dir.path().join("bootstrap/setup/BOOTSTRAP.md"), "# setup").unwrap();
    fs::create_dir_all(dir.path().join(".claude-plugin")).unwrap();
    fs::write(
        dir.path().join(".claude-plugin/plugin.json"),
        r#"{"bootstrap_docs":["./bootstrap"]}"#,
    )
    .unwrap();

    let items = discover_source(dir.path(), Some("demo"), &mut DiagnosticCollector::new()).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id.kind, ItemKind::BootstrapDoc);
    assert_eq!(items[0].id.name.as_str(), "setup");
    assert_eq!(items[0].source_path, PathBuf::from("bootstrap/setup"));
}

#[test]
fn nested_bootstrap_dir_is_discovered() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("nested/bootstrap/setup")).unwrap();
    fs::create_dir_all(dir.path().join("nested/bootstrap/.hidden")).unwrap();
    fs::write(
        dir.path().join("nested/bootstrap/setup/BOOTSTRAP.md"),
        "# setup",
    )
    .unwrap();
    fs::write(
        dir.path().join("nested/bootstrap/.hidden/BOOTSTRAP.md"),
        "# hidden",
    )
    .unwrap();

    let items = discover_source(dir.path(), Some("demo"), &mut DiagnosticCollector::new()).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id.kind, ItemKind::BootstrapDoc);
    assert_eq!(items[0].id.name.as_str(), "setup");
    assert_eq!(
        items[0].source_path,
        PathBuf::from("nested/bootstrap/setup")
    );
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
