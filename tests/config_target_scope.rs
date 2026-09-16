//! Target scope and winning-file provenance at the real configuration boundary.
use mars_agents::config::{
    load_effective_project_config,
    targets::{HarnessScope, LinkSource, TargetOrigin},
};
use tempfile::tempdir;

#[test]
fn target_scope_retains_the_winning_field_and_file() {
    for (project, local, field, origin, permitted) in [
        ("", "", LinkSource::None, TargetOrigin::Unset, None),
        (
            "targets = []",
            "",
            LinkSource::Targets,
            TargetOrigin::Project,
            Some(vec![]),
        ),
        (
            "targets = [\".codex\"]",
            "targets = [\".codex\"]",
            LinkSource::Targets,
            TargetOrigin::Local,
            Some(vec!["codex"]),
        ),
        (
            "targets = [\".claude\"]",
            "targets = []",
            LinkSource::Targets,
            TargetOrigin::Local,
            Some(vec![]),
        ),
        (
            "targets = [\".codex\"]",
            "managed_root = \".claude\"",
            LinkSource::Targets,
            TargetOrigin::Project,
            Some(vec!["codex"]),
        ),
        (
            "managed_root = \".claude\"",
            "managed_root = \".agents\"",
            LinkSource::ManagedRoot,
            TargetOrigin::Local,
            Some(vec![]),
        ),
        (
            "managed_root = \".codex\"",
            "",
            LinkSource::ManagedRoot,
            TargetOrigin::Project,
            Some(vec!["codex"]),
        ),
        (
            "managed_root = \".claude\"",
            "targets = [\".codex\", \".agents\", \"path/agents\"]",
            LinkSource::Targets,
            TargetOrigin::Local,
            Some(vec!["codex"]),
        ),
    ] {
        let root = tempdir().unwrap();
        std::fs::write(
            root.path().join("mars.toml"),
            format!("[settings]\n{project}\n"),
        )
        .unwrap();
        std::fs::write(
            root.path().join("mars.local.toml"),
            format!("[settings]\n{local}\n"),
        )
        .unwrap();
        let effective = load_effective_project_config(root.path()).unwrap();
        let scope = effective.settings.effective_links().harness_scope();
        assert_eq!(
            scope.harness_names(),
            permitted.map(|names| names.into_iter().map(str::to_string).collect()),
            "{project} / {local}"
        );
        assert_eq!(effective.target_source.field, field);
        assert_eq!(effective.target_source.origin, origin);
        assert_eq!(
            effective.target_source.path,
            match origin {
                TargetOrigin::Project => Some(root.path().join("mars.toml")),
                TargetOrigin::Local => Some(root.path().join("mars.local.toml")),
                TargetOrigin::Unset => None,
            }
        );
        assert_eq!(
            scope == HarnessScope::Unrestricted,
            origin == TargetOrigin::Unset
        );
    }
}
