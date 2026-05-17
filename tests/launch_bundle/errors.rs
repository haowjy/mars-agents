use super::common::{setup_bundle_project, setup_bundle_project_with_agents};
use crate::test_common::{API_PATH, mars_cmd};
use assert_fs::TempDir;

pub(crate) fn build_launch_bundle_fails_on_unknown_agent_harness() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
harness: not-a-harness
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.assert()
        .failure()
        .code(2)
        .stderr(predicates::str::contains("unknown harness"));
}

pub(crate) fn build_launch_bundle_fails_on_invalid_top_level_agent_field_value() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
model-invocable: nope
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.assert()
        .failure()
        .code(2)
        .stderr(predicates::str::contains("model-invocable"));
}

pub(crate) fn build_launch_bundle_fails_on_non_overridable_model_invocable_override() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
harness-overrides:
  claude:
    model-invocable: false
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.assert()
        .failure()
        .code(2)
        .stderr(predicates::str::contains("not overridable"));
}

pub(crate) fn build_launch_bundle_fails_when_inventory_agent_has_fatal_frontmatter_diagnostic() {
    let temp = TempDir::new().unwrap();
    let reviewer_content = r#"---
name: reviewer
model: claude-opus-4-6
---
Review code changes."#;
    let malformed_inventory_content = r#"---
name: malformed
model: claude-opus-4-6
model-invocable: nope
---
Broken inventory entry."#;

    let (server, project_root) = setup_bundle_project_with_agents(
        &temp,
        "bundle-source",
        &[
            ("reviewer", reviewer_content),
            ("malformed", malformed_inventory_content),
        ],
        &[],
        "",
    );

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.assert()
        .failure()
        .code(2)
        .stderr(predicates::str::contains("inventory file"));
}

pub(crate) fn build_launch_bundle_fails_when_agent_file_missing() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "missing-agent"]);
    cmd.assert()
        .failure()
        .stderr(predicates::str::contains("missing-agent"))
        .stderr(predicates::str::contains("read launch bundle agent"));
}
