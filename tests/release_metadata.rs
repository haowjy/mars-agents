// qa-validated: mars-release-workflow-audit
use std::path::Path;

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &str) -> String {
    std::fs::read_to_string(repo_root().join(path)).expect(path)
}

#[test]
fn npm_stub_declares_windows_optional_package() {
    let stub: serde_json::Value =
        serde_json::from_str(&read("npm/@meridian-flow/mars-agents/package.json"))
            .expect("stub package json");
    let optional = stub
        .get("optionalDependencies")
        .and_then(serde_json::Value::as_object)
        .expect("optionalDependencies");

    assert!(
        optional.contains_key("@meridian-flow/mars-agents-win32-x64"),
        "stub package must install the Windows x64 binary package"
    );

    // The package it points at publishes the exe for win32 x64 only.
    let pkg: serde_json::Value = serde_json::from_str(&read(
        "npm/@meridian-flow/mars-agents-win32-x64/package.json",
    ))
    .expect("windows package json");
    let strings = |values: &[&str]| {
        Some(
            values
                .iter()
                .map(|v| serde_json::Value::String(v.to_string()))
                .collect::<Vec<_>>(),
        )
    };
    assert_eq!(
        pkg.get("name").and_then(serde_json::Value::as_str),
        Some("@meridian-flow/mars-agents-win32-x64")
    );
    for (key, expected) in [("os", "win32"), ("cpu", "x64"), ("files", "mars.exe")] {
        assert_eq!(
            pkg.get(key).and_then(serde_json::Value::as_array).cloned(),
            strings(&[expected]),
            "{key}"
        );
    }
}

#[test]
fn release_publish_uses_single_tag_workflow_identity() {
    let release_on_main = read(".github/workflows/release-on-main.yml");
    let release = read(".github/workflows/release.yml");

    assert!(
        !release_on_main.contains("uses: ./.github/workflows/release.yml"),
        "auto-release should create the release tag only; publishing must be triggered by release.yml on tag push"
    );
    assert!(
        release.contains("tags: [\"v*\"]"),
        "release.yml must remain the single tag-triggered publish workflow"
    );
    assert!(
        !release.contains("workflow_call:"),
        "release.yml should not also be reusable; npm trusted publishing allows one workflow identity per package"
    );
    assert!(
        !release.contains("inputs.tag"),
        "release.yml should derive release identity only from the pushed tag"
    );
    assert!(
        release_on_main.contains("TAG_PUSH_TOKEN: ${{ secrets.RELEASE_TOKEN || github.token }}"),
        "auto-release tag push should prefer RELEASE_TOKEN so the tag workflow can run"
    );
}
