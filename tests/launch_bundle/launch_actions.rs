use super::common::{install_fake_harnesses, replace_path_with, setup_bundle_project};
use crate::test_common::{API_PATH, mars_cmd};
use assert_fs::TempDir;
use serde_json::Value;

pub(crate) fn build_launch_bundle_omits_launch_actions_without_context() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["cursor"]);
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
harness: cursor
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["version"].as_u64(), Some(4));
    assert!(bundle.get("launch_actions").is_none());
}

pub(crate) fn build_launch_bundle_projects_cursor_launch_actions() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["cursor"]);
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
harness: cursor
approval: auto
sandbox: read-only
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let context = serde_json::json!({
        "cwd": "/work/project",
        "temp_dir": "/tmp/mars-spawn",
        "streaming": null,
        "session_id": null,
        "fork": false,
        "workspace_roots": ["/ignored-extra-root"],
        "interactive": false,
        "extra_args": ["--foo"],
        "opencode_config_content": null,
        "pi_extension_entrypoints": [],
        "prompt": "Review this change"
    });

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--context",
        &context.to_string(),
    ]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(
        bundle["launch_actions"]["argv"],
        serde_json::json!([
            "cursor",
            "agent",
            "--print",
            "--output-format",
            "stream-json",
            "--trust",
            "--model",
            "claude-opus-4-6",
            "--force",
            "--sandbox",
            "enabled",
            "--workspace",
            "/work/project",
            "--foo",
            "Review this change"
        ])
    );
    assert_eq!(bundle["launch_actions"]["env"], serde_json::json!({}));
    assert_eq!(bundle["launch_actions"]["files"], serde_json::json!([]));
    assert!(bundle["launch_actions"]["protocol_payload"].is_null());
}

pub(crate) fn build_launch_bundle_projects_claude_launch_actions() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["claude"]);
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
harness: claude
effort: xhigh
approval: auto
tools: [Bash, Write, Agent]
disallowed-tools: [Write]
mcp-tools: [plugin:context7:context7]
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let context = serde_json::json!({
        "cwd": "/work/project",
        "temp_dir": "/tmp/mars-spawn",
        "streaming": null,
        "session_id": "session-123",
        "fork": true,
        "workspace_roots": ["/extra/root"],
        "interactive": false,
        "extra_args": ["--meridian-parent-allowed-tools", "Read,Write", "--allowedTools", "Grep", "--passthrough"],
        "opencode_config_content": null,
        "pi_extension_entrypoints": [],
        "prompt": "ignored by claude argv"
    });

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--context",
        &context.to_string(),
    ]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(
        bundle["launch_actions"]["argv"],
        serde_json::json!([
            "claude",
            "-p",
            "--output-format",
            "stream-json",
            "--verbose",
            "-",
            "--model",
            "claude-opus-4-6",
            "--effort",
            "max",
            "--agent",
            "reviewer",
            "--permission-mode",
            "acceptEdits",
            "--allowedTools",
            "Bash,Read,Grep",
            "--disallowedTools",
            "Write,Agent(Explore),Agent(Plan),Agent(General-purpose),Agent(general-purpose)",
            "--mcp-config",
            "plugin:context7:context7",
            "--append-system-prompt-file",
            "/tmp/mars-spawn/prompt.md",
            "--resume",
            "session-123",
            "--fork-session",
            "--add-dir",
            "/extra/root",
            "--passthrough"
        ])
    );
    assert_eq!(
        bundle["launch_actions"]["files"][0]["path"].as_str(),
        Some("/tmp/mars-spawn/prompt.md")
    );
    assert!(
        bundle["launch_actions"]["files"][0]["content"]
            .as_str()
            .unwrap()
            .contains("Review code changes.")
    );
}

pub(crate) fn build_launch_bundle_projects_codex_subprocess_launch_actions() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["codex"]);
    let agent_content = r#"---
name: coder
model: gpt-5
harness: codex
effort: high
approval: auto
sandbox: workspace-write
mcp-tools: ["fs=npx filesystem-server"]
---
Code."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");
    let context = serde_json::json!({
        "cwd": "/work/project",
        "temp_dir": "/tmp/mars-spawn",
        "streaming": null,
        "session_id": "codex-thread",
        "fork": false,
        "workspace_roots": ["/extra/root"],
        "interactive": false,
        "extra_args": ["--foo"],
        "opencode_config_content": null,
        "pi_extension_entrypoints": [],
        "prompt": "USER",
        "base_instructions": "BASE",
        "developer_instructions": "DEV",
        "report_output_path": "/tmp/report.md"
    });

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--context",
        &context.to_string(),
    ]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(
        bundle["launch_actions"]["argv"],
        serde_json::json!([
            "codex",
            "exec",
            "--json",
            "--model",
            "gpt-5",
            "-c",
            "model_reasoning_effort=\"high\"",
            "--sandbox",
            "workspace-write",
            "-c",
            "approval_policy=\"on-request\"",
            "-c",
            "mcp.servers.fs.command=\"npx filesystem-server\"",
            "resume",
            "codex-thread",
            "--add-dir",
            "/extra/root",
            "--foo",
            "-o",
            "/tmp/report.md",
            "BASE\n\nDEV\n\nUSER"
        ])
    );
}

pub(crate) fn build_launch_bundle_projects_opencode_subprocess_launch_actions() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["opencode"]);
    let agent_content = r#"---
name: coder
model: openai/gpt-5
harness: opencode
effort: high
---
Code."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");
    let context = serde_json::json!({
        "cwd": "/work/project",
        "temp_dir": "/tmp/mars-spawn",
        "streaming": null,
        "session_id": "opencode-session",
        "fork": true,
        "workspace_roots": ["/extra/root"],
        "interactive": false,
        "extra_args": ["--foo"],
        "opencode_config_content": "{\"permission\":{\"external_directory\":[\"/parent\"]}}",
        "pi_extension_entrypoints": [],
        "prompt": "USER"
    });

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--context",
        &context.to_string(),
    ]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(
        bundle["launch_actions"]["argv"],
        serde_json::json!([
            "opencode",
            "run",
            "--model",
            "openai/gpt-5",
            "--variant",
            "high",
            "--foo",
            "-",
            "--session",
            "opencode-session",
            "--fork"
        ])
    );
    assert_eq!(
        bundle["launch_actions"]["env"]["OPENCODE_CONFIG_CONTENT"].as_str(),
        Some(
            "{\"permission\":{\"external_directory\":{\"/extra/root/**\":\"allow\",\"/parent\":\"allow\"}}}"
        )
    );
}

pub(crate) fn build_launch_bundle_projects_pi_launch_actions() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["pi"]);
    let agent_content = r#"---
name: coder
model: openai-codex/gpt-5.4-mini
harness: pi
effort: xhigh
---
Code."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");
    let context = serde_json::json!({
        "cwd": "/work/project",
        "temp_dir": "/tmp/mars-spawn",
        "streaming": null,
        "session_id": "pi-session",
        "fork": false,
        "workspace_roots": [],
        "interactive": false,
        "extra_args": ["--foo"],
        "opencode_config_content": null,
        "pi_extension_entrypoints": ["/ext/managed-bash/index.js", "/ext/spawn-watch/index.js"],
        "prompt": "USER",
        "pi_session_dir": "/tmp/pi-sessions"
    });

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--context",
        &context.to_string(),
    ]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();
    let argv = bundle["launch_actions"]["argv"].as_array().unwrap();

    assert_eq!(argv[0].as_str(), Some("pi"));
    assert_eq!(argv[1].as_str(), Some("--mode"));
    assert_eq!(argv[2].as_str(), Some("rpc"));
    assert!(
        argv.iter()
            .any(|value| value.as_str() == Some("openai-codex/gpt-5.4-mini:xhigh"))
    );
    assert!(
        argv.iter()
            .any(|value| value.as_str() == Some("/ext/managed-bash/index.js"))
    );
    assert!(
        argv.iter()
            .any(|value| value.as_str() == Some("/tmp/pi-sessions"))
    );
}

pub(crate) fn build_launch_bundle_projects_codex_streaming_launch_actions() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["codex"]);
    let agent_content = r#"---
name: coder
model: gpt-5
effort: high
harness: codex
approval: never
sandbox: danger-full-access
mcp-tools: ["fs=npx filesystem-server"]
---
Code."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");
    let context = serde_json::json!({
        "cwd": "/work/project",
        "temp_dir": "/tmp/mars-spawn",
        "streaming": {"host": "127.0.0.1", "port": 9876},
        "session_id": "thread-1",
        "fork": true,
        "workspace_roots": ["/extra/root"],
        "interactive": false,
        "extra_args": ["--foo"],
        "opencode_config_content": null,
        "pi_extension_entrypoints": [],
        "prompt": "USER",
        "base_instructions": "BASE",
        "developer_instructions": "DEV"
    });

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--transport",
        "streaming",
        "--context",
        &context.to_string(),
    ]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(
        bundle["launch_actions"]["argv"],
        serde_json::json!([
            "codex",
            "app-server",
            "--listen",
            "ws://127.0.0.1:9876",
            "-c",
            "sandbox_mode=\"danger-full-access\"",
            "-c",
            "approval_policy=\"never\"",
            "-c",
            "mcp.servers.fs.command=\"npx filesystem-server\"",
            "-c",
            "sandbox_workspace_write.writable_roots=[\"/extra/root\"]",
            "--foo"
        ])
    );
    assert_eq!(
        bundle["launch_actions"]["protocol_payload"],
        serde_json::json!({
            "transport": "jsonrpc",
            "method": "thread/fork",
            "params": {
                "cwd": "/work/project",
                "baseInstructions": "BASE",
                "developerInstructions": "DEV",
                "model": "gpt-5",
                "config": {"model_reasoning_effort": "high"},
                "approvalPolicy": "never",
                "sandbox": "danger-full-access",
                "threadId": "thread-1",
                "ephemeral": false
            }
        })
    );
}

pub(crate) fn build_launch_bundle_projects_opencode_streaming_launch_actions() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["opencode"]);
    let agent_content = r#"---
name: coder
model: openai/gpt-5
harness: opencode
mcp-tools: [server-one]
---
Code."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");
    let context = serde_json::json!({
        "cwd": "/work/project",
        "temp_dir": "/tmp/mars-spawn",
        "streaming": {"host": "127.0.0.1", "port": 9877},
        "session_id": null,
        "fork": false,
        "workspace_roots": ["/extra/root"],
        "interactive": false,
        "extra_args": ["--foo"],
        "opencode_config_content": null,
        "pi_extension_entrypoints": [],
        "prompt": "USER"
    });

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--transport",
        "streaming",
        "--context",
        &context.to_string(),
    ]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(
        bundle["launch_actions"]["argv"],
        serde_json::json!([
            "opencode",
            "serve",
            "--hostname",
            "127.0.0.1",
            "--port",
            "9877",
            "--foo"
        ])
    );
    assert_eq!(
        bundle["launch_actions"]["env"]["OPENCODE_CONFIG_CONTENT"].as_str(),
        Some("{\"permission\":{\"external_directory\":{\"/extra/root/**\":\"allow\"}}}")
    );
    assert_eq!(
        bundle["launch_actions"]["protocol_payload"],
        serde_json::json!({
            "transport": "http",
            "method": "POST",
            "path": "/session",
            "body": {
                "model": "openai/gpt-5",
                "modelID": "openai/gpt-5",
                "mcp": {"servers": ["server-one"]}
            }
        })
    );
}
