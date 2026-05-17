use assert_fs::TempDir;
use assert_fs::prelude::*;
use httpmock::prelude::*;
use serde_json::Value;

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
