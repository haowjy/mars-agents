use std::path::Path;

use tempfile::TempDir;

#[test]
fn plugin_config_applies_defaults_from_package() {
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("mars.toml"),
        r#"
[package]
name = "example-plugin"
version = "1.2.3"
description = "Package description"
authors = [{ name = "Example Author", email = "author@example.com" }]
homepage = "https://example.com/plugin"
license = "MIT"

[plugin]
"#,
    )
    .unwrap();

    let config = crate::config::load(dir.path()).unwrap();
    let package = config.package.as_ref().unwrap();
    let plugin = config.plugin.as_ref().unwrap();
    assert!(plugin.as_table().is_some_and(toml::Table::is_empty));
    assert_eq!(
        package.authors.as_ref().and_then(toml::Value::as_array),
        Some(&vec![toml::Value::Table(toml::Table::from_iter([
            (
                "name".to_string(),
                toml::Value::String("Example Author".to_string())
            ),
            (
                "email".to_string(),
                toml::Value::String("author@example.com".to_string())
            ),
        ]))])
    );
    assert_eq!(
        package.homepage.as_ref().and_then(toml::Value::as_str),
        Some("https://example.com/plugin")
    );
    assert_eq!(
        package.license.as_ref().and_then(toml::Value::as_str),
        Some("MIT")
    );

    let metadata = super::validate_plugin_build(&config).unwrap();
    assert_eq!(metadata.dir, Path::new("plugin"));
    assert_eq!(metadata.name, "example-plugin");
    assert_eq!(metadata.marketplace, "example-plugin");
    assert_eq!(metadata.description.as_deref(), Some("Package description"));
    assert_eq!(metadata.harnesses.len(), 5);
    assert_eq!(metadata.authors[0].name, "Example Author");
    assert_eq!(
        metadata.authors[0].email.as_deref(),
        Some("author@example.com")
    );
}

#[test]
fn plugin_config_parses_explicit_values() {
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("mars.toml"),
        r#"
[package]
name = "example-package"
version = "1.2.3"
description = "Package description"
authors = ["Example Author <author@example.com>"]

[plugin]
dir = "generated/plugin"
name = "custom-plugin"
marketplace = "custom-marketplace"
description = "Plugin description"
harnesses = ["claude", "cursor"]
"#,
    )
    .unwrap();

    let config = crate::config::load(dir.path()).unwrap();
    let plugin = config.plugin.as_ref().unwrap();
    assert_eq!(
        plugin
            .as_table()
            .and_then(|plugin| plugin.get("dir"))
            .and_then(toml::Value::as_str),
        Some("generated/plugin")
    );
    let metadata = super::validate_plugin_build(&config).unwrap();
    assert_eq!(metadata.dir, Path::new("generated/plugin"));
    assert_eq!(metadata.name, "custom-plugin");
    assert_eq!(metadata.marketplace, "custom-marketplace");
    assert_eq!(metadata.description.as_deref(), Some("Plugin description"));
    assert_eq!(metadata.authors[0].name, "Example Author");
    assert_eq!(
        metadata.authors[0].email.as_deref(),
        Some("author@example.com")
    );
    assert_eq!(
        metadata
            .harnesses
            .iter()
            .map(|harness| harness.key)
            .collect::<Vec<_>>(),
        vec!["claude", "cursor"]
    );
}

#[test]
fn plugin_build_rejects_unsafe_directories() {
    for (plugin_dir, expected) in [
        ("", "cannot be empty"),
        ("/tmp/plugin", "must be a relative path"),
        ("../plugin", "without `.` or `..`"),
        (".", "without `.` or `..`"),
        (".generated", "cannot use a dot directory"),
        ("generated/.private", "cannot use a dot directory"),
        ("skills/plugin", "source convention directory `skills`"),
    ] {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("mars.toml"),
            format!(
                "[package]\nname = \"example-plugin\"\nversion = \"1.0.0\"\n\n[plugin]\ndir = {plugin_dir:?}\n"
            ),
        )
        .unwrap();

        let config = crate::config::load(dir.path()).unwrap();
        let error = super::validate_plugin_build(&config)
            .err()
            .unwrap()
            .to_string();
        assert!(
            error.contains(expected),
            "dir {plugin_dir:?} should report {expected:?}, got: {error}"
        );
    }
}

#[test]
fn plugin_build_rejects_unknown_harnesses_and_pi() {
    for harness in ["unknown", "pi"] {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("mars.toml"),
            format!(
                "[package]\nname = \"example-plugin\"\nversion = \"1.0.0\"\n\n[plugin]\nharnesses = [{harness:?}]\n"
            ),
        )
        .unwrap();

        let config = crate::config::load(dir.path()).unwrap();
        let error = super::validate_plugin_build(&config)
            .err()
            .unwrap()
            .to_string();
        assert!(
            error.contains(&format!("unknown harness `{harness}`"))
                && error.contains("expected one of: claude, codex, grok, cursor, opencode"),
            "harness {harness:?} should be refused, got: {error}"
        );
    }
}

#[test]
fn plugin_build_validates_name_for_each_listed_harness() {
    for (harness, name, expected) in [
        ("claude", "bad:name", "Claude names"),
        ("codex", "bad--name", "Codex names"),
        ("grok", "bad/name", "Grok reads the Claude tree"),
        ("cursor", "Bad_name", "Cursor names"),
        ("opencode", "_bad-name", "OpenCode names"),
    ] {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("mars.toml"),
            format!(
                "[package]\nname = \"fallback-name\"\nversion = \"1.0.0\"\n\n[plugin]\nname = {name:?}\nharnesses = [{harness:?}]\n"
            ),
        )
        .unwrap();

        let config = crate::config::load(dir.path()).unwrap();
        let error = super::validate_plugin_build(&config)
            .err()
            .unwrap()
            .to_string();
        assert!(
            error.contains(expected),
            "name {name:?} for {harness} should report {expected:?}, got: {error}"
        );
    }
}
