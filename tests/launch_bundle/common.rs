use assert_fs::TempDir;
use assert_fs::prelude::*;
use httpmock::prelude::*;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

use crate::test_common::{API_PATH, create_source, mars_cmd, sample_catalog_json};

pub fn setup_bundle_project(
    temp: &TempDir,
    source_name: &str,
    agent_content: &str,
    skills: &[(&str, &str)],
    extra_project_toml: &str,
) -> (MockServer, std::path::PathBuf) {
    setup_bundle_project_with_agents(
        temp,
        source_name,
        &[("reviewer", agent_content)],
        skills,
        extra_project_toml,
    )
}

pub fn setup_bundle_project_with_agents(
    temp: &TempDir,
    source_name: &str,
    agents: &[(&str, &str)],
    skills: &[(&str, &str)],
    extra_project_toml: &str,
) -> (MockServer, std::path::PathBuf) {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path(API_PATH);
        then.status(200).json_body(sample_catalog_json());
    });

    let source = create_source(temp, source_name, agents, skills);
    let project = temp.child("project");
    project.create_dir_all().unwrap();

    let mut toml = format!(
        "[dependencies]\n{source_name} = {{ path = \"{}\" }}\n",
        source.display().to_string().replace('\\', "/")
    );
    if !extra_project_toml.trim().is_empty() {
        toml.push('\n');
        toml.push_str(extra_project_toml);
        toml.push('\n');
    }
    project.child("mars.toml").write_str(&toml).unwrap();

    let mut sync_cmd = mars_cmd(project.path(), temp.path(), &server.url(API_PATH));
    sync_cmd.arg("sync");
    sync_cmd.assert().success();

    (server, project.to_path_buf())
}

pub fn assert_prompt_surface_excludes(bundle: &Value, needles: &[&str]) {
    let prompt_surface = &bundle["prompt_surface"];
    let mut surfaces = vec![
        (
            "system_instruction",
            prompt_surface["system_instruction"]
                .as_str()
                .unwrap_or_default(),
        ),
        (
            "inventory_prompt",
            prompt_surface["inventory_prompt"]
                .as_str()
                .unwrap_or_default(),
        ),
    ];

    let empty_docs = Vec::new();
    let docs = prompt_surface["supplemental_documents"]
        .as_array()
        .unwrap_or(&empty_docs);
    for doc in docs {
        surfaces.push((
            "supplemental_documents.content",
            doc["content"].as_str().unwrap_or_default(),
        ));
    }

    for needle in needles {
        for (surface_name, content) in &surfaces {
            assert!(
                !content.contains(needle),
                "`{needle}` leaked into prompt surface `{surface_name}`: {content}"
            );
        }
    }
}

pub fn install_fake_harnesses(temp_root: &Path, harnesses: &[&str]) -> PathBuf {
    let bin_dir = temp_root.join("harness-bin-common");
    fs::create_dir_all(&bin_dir).unwrap();

    for harness in harnesses {
        #[cfg(windows)]
        {
            let script = if *harness == "pi" {
                "@echo off\r\nif \"%~1\"==\"--version\" (\r\n  echo pi 0.0.0-test\r\n  exit /b 0\r\n)\r\nif \"%~1\"==\"--help\" (\r\n  echo --mode rpc --model --append-system-prompt --session --fork --session-dir PI_CODING_AGENT_SESSION_DIR --no-extensions --no-skills --no-context-files --no-prompt-templates -e\r\n  exit /b 0\r\n)\r\nif \"%~1\"==\"--list-models\" (\r\n  echo openai gpt-5\r\n  echo openai gpt-5.4-mini\r\n  echo openai gpt-5.5\r\n  echo anthropic claude-opus-4-6\r\n  echo anthropic claude-opus-4-7\r\n  echo google gemini-2.5-pro\r\n  exit /b 0\r\n)\r\nexit /b 0\r\n"
            } else if *harness == "opencode" {
                "@echo off\r\nif \"%~1\"==\"models\" (\r\n  echo openai/gpt-5\r\n  echo openai/gpt-5.4-mini\r\n  echo openai/gpt-5.5\r\n  echo anthropic/claude-opus-4-6\r\n  echo anthropic/claude-opus-4-7\r\n  echo google/gemini-2.5-pro\r\n  exit /b 0\r\n)\r\nexit /b 0\r\n"
            } else {
                "@echo off\r\nexit /b 0\r\n"
            };
            fs::write(bin_dir.join(format!("{harness}.bat")), script).unwrap();
        }
        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = bin_dir.join(harness);
            let script = if *harness == "pi" {
                "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  echo \"pi 0.0.0-test\"\n  exit 0\nfi\nif [ \"$1\" = \"--help\" ]; then\n  echo \"--mode rpc --model --append-system-prompt --session --fork --session-dir PI_CODING_AGENT_SESSION_DIR --no-extensions --no-skills --no-context-files --no-prompt-templates -e\"\n  exit 0\nfi\nif [ \"$1\" = \"--list-models\" ]; then\n  printf '%s\\n' \\\n    'openai-codex gpt-5.4-mini' \\\n    'openai-codex gpt-5.5' \\\n    'openai gpt-5' \\\n    'openai gpt-5.4-mini' \\\n    'openai gpt-5.5' \\\n    'anthropic claude-opus-4-6' \\\n    'anthropic claude-opus-4-7' \\\n    'google gemini-2.5-pro'\n  exit 0\nfi\nexit 0\n"
            } else if *harness == "opencode" {
                "#!/bin/sh\nif [ \"$1\" = \"models\" ]; then\n  printf '%s\\n' \\\n    'openai/gpt-5' \\\n    'openai/gpt-5.4-mini' \\\n    'openai/gpt-5.5' \\\n    'anthropic/claude-opus-4-6' \\\n    'anthropic/claude-opus-4-7' \\\n    'google/gemini-2.5-pro'\n  exit 0\nfi\nexit 0\n"
            } else {
                "#!/bin/sh\nexit 0\n"
            };
            fs::write(&path, script).unwrap();
            let mut perms = fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(path, perms).unwrap();
        }
    }

    bin_dir
}

pub fn replace_path_with(bin_dir: &Path) -> String {
    bin_dir.to_string_lossy().into_owned()
}
