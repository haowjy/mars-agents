// qa-validated: mars-tools-abstraction

mod common;

use assert_fs::TempDir;
use assert_fs::prelude::*;
use std::fs;

use common::*;

#[test]
fn sync_http_mcp_wires_into_all_targets() {
    let dir = TempDir::new().unwrap();

    let source = dir.child("base");
    source.create_dir_all().unwrap();

    let mcp_dir = source.child("mcp").child("remote-api");
    mcp_dir.create_dir_all().unwrap();
    mcp_dir
        .child("mcp.toml")
        .write_str(
            r#"type = "http"
url = "https://example.com/mcp"
visibility = "exported"

[headers]
Authorization = { from = "env", var = "API_TOKEN" }
X-Tenant = "acme"
"#,
        )
        .unwrap();

    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(&format!(
            r#"[settings]
targets = [".claude", ".codex", ".opencode", ".cursor"]

[dependencies.base]
path = "{}"
"#,
            source.path().display().to_string().replace('\\', "/")
        ))
        .unwrap();

    mars()
        .args(["sync", "--root", project.path().to_str().unwrap()])
        .assert()
        .success();

    let claude_json: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(project.child(".claude/.mcp.json").path()).unwrap(),
    )
    .unwrap();
    let claude_server = &claude_json["mcpServers"]["remote-api"];
    assert_eq!(claude_server["type"].as_str(), Some("http"));
    assert_eq!(
        claude_server["url"].as_str(),
        Some("https://example.com/mcp")
    );
    assert!(
        claude_server["headers"]["Authorization"]
            .as_str()
            .map(|s| s.contains("API_TOKEN"))
            .unwrap_or(false)
    );
    assert_eq!(claude_server["headers"]["X-Tenant"].as_str(), Some("acme"));

    let codex_toml = fs::read_to_string(project.child(".codex/config.toml").path()).unwrap();
    let codex_config: toml::Value = toml::from_str(&codex_toml).unwrap();
    let codex_server = &codex_config["mcp"]["servers"]["remote-api"];
    assert_eq!(
        codex_server["url"].as_str(),
        Some("https://example.com/mcp")
    );
    assert_eq!(
        codex_server["bearer_token_env_var"].as_str(),
        Some("API_TOKEN")
    );
    assert_eq!(
        codex_server["http_headers"]["X-Tenant"].as_str(),
        Some("acme")
    );

    let opencode_json: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(project.child(".opencode/opencode.json").path()).unwrap(),
    )
    .unwrap();
    let opencode_server = &opencode_json["mcp"]["remote-api"];
    assert_eq!(opencode_server["type"].as_str(), Some("remote"));
    assert_eq!(
        opencode_server["url"].as_str(),
        Some("https://example.com/mcp")
    );
    assert_eq!(
        opencode_server["headers"]["X-Tenant"].as_str(),
        Some("acme")
    );

    let cursor_json: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(project.child(".cursor/mcp.json").path()).unwrap(),
    )
    .unwrap();
    let cursor_server = &cursor_json["mcpServers"]["remote-api"];
    assert!(
        cursor_server.is_object(),
        "remote-api should be present in cursor mcp.json"
    );
    assert_eq!(
        cursor_server["url"].as_str(),
        Some("https://example.com/mcp")
    );
}

#[test]
fn sync_cursor_hook_drops_emit_warning() {
    let dir = TempDir::new().unwrap();

    let source = dir.child("base");
    source.create_dir_all().unwrap();

    let hook_dir = source.child("hooks").child("audit-log");
    hook_dir.create_dir_all().unwrap();
    hook_dir
        .child("hook.toml")
        .write_str(
            r#"name = "audit-log"
event = "session.start"
visibility = "exported"

[action]
kind = "script"
path = "run.sh"
"#,
        )
        .unwrap();
    hook_dir.child("run.sh").write_str("#!/bin/sh\n").unwrap();

    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(&format!(
            r#"[settings]
targets = [".cursor"]

[dependencies.base]
path = "{}"
"#,
            source.path().display().to_string().replace('\\', "/")
        ))
        .unwrap();

    let output = mars()
        .args(["sync", "--root", project.path().to_str().unwrap()])
        .output()
        .unwrap();

    assert!(output.status.success(), "sync should succeed for cursor");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("hook-dropped")
            || ((stderr.contains("hook") || stderr.contains("dropped"))
                && stderr.contains("cursor")),
        "expected cursor hook-drop warning in stderr: {stderr}"
    );

    assert!(!project.child(".cursor/hooks.json").exists());
    assert!(!project.child(".cursor/settings.json").exists());
    assert!(!project.child(".cursor/codex_hooks.json").exists());
}
