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
}

#[test]
fn windows_npm_package_publishes_exe_only_for_win32_x64() {
    let pkg: serde_json::Value = serde_json::from_str(&read(
        "npm/@meridian-flow/mars-agents-win32-x64/package.json",
    ))
    .expect("windows package json");

    assert_eq!(
        pkg.get("name").and_then(serde_json::Value::as_str),
        Some("@meridian-flow/mars-agents-win32-x64")
    );
    assert_eq!(
        pkg.get("os").and_then(serde_json::Value::as_array),
        Some(&vec![serde_json::Value::String("win32".to_string())])
    );
    assert_eq!(
        pkg.get("cpu").and_then(serde_json::Value::as_array),
        Some(&vec![serde_json::Value::String("x64".to_string())])
    );
    assert_eq!(
        pkg.get("files").and_then(serde_json::Value::as_array),
        Some(&vec![serde_json::Value::String("mars.exe".to_string())])
    );
}

#[test]
fn npm_launcher_routes_windows_to_exe_package() {
    let launcher = read("npm/@meridian-flow/mars-agents/bin/mars");

    assert!(launcher.contains("\"win32 x64\": \"@meridian-flow/mars-agents-win32-x64\""));
    assert!(launcher.contains("process.platform === \"win32\" ? \"mars.exe\" : \"mars\""));
    assert!(launcher.contains("win32-x64"));
}

#[test]
fn ci_workflow_runs_windows_build_test_clippy_and_fmt() {
    let workflow = read(".github/workflows/ci.yml");

    assert!(workflow.contains("check-windows:"));
    assert!(workflow.contains("runs-on: windows-latest"));
    assert!(workflow.contains("cargo build"));
    assert!(workflow.contains("cargo test"));
    assert!(workflow.contains("cargo clippy -- -D warnings"));
    assert!(workflow.contains("cargo fmt --check"));
}

#[test]
fn release_workflow_windows_artifact_contract() {
    let workflow = read(".github/workflows/release.yml");

    assert!(workflow.contains("x86_64-pc-windows-msvc"));
    assert!(workflow.contains("artifact: mars-windows-x64.exe"));
    assert!(workflow.contains("Smoke test (Windows)"));
    assert!(workflow.contains("mars.exe --version"));
    assert!(workflow.contains("mars.exe init --root $tmp"));
    assert!(workflow.contains("mars.exe doctor --root $tmp"));
    assert!(workflow.contains("cp \"$GITHUB_WORKSPACE/artifacts/$binary\" mars.exe"));
    assert!(workflow.contains(
        "publish_platform npm/@meridian-flow/mars-agents-win32-x64 mars-windows-x64.exe"
    ));
}

#[test]
fn release_on_main_has_rc_default_label_contract() {
    let workflow = read(".github/workflows/release-on-main.yml");

    assert!(workflow.contains("release:skip"));
    assert!(workflow.contains("release:(skip|patch|stable|rc)"));
    assert!(workflow.contains("release_kind=\"rc\""));
    assert!(workflow.contains("release_kind=\"stable\""));
    assert!(workflow.contains("echo \"release_kind=${release_kind}\" >> \"$GITHUB_OUTPUT\""));
}

#[test]
fn release_on_main_computes_stable_and_rc_versions() {
    let workflow = read(".github/workflows/release-on-main.yml");

    assert!(workflow.contains("if [[ \"${RELEASE_KIND}\" == \"stable\" ]]; then"));
    assert!(workflow.contains("next_version=\"${next_patch}-rc.${next_rc}\""));
    assert!(workflow.contains("python_version=\"${next_patch}rc${next_rc}\""));
    assert!(workflow.contains("done < <(git tag --list \"v${next_patch}-rc.*\")"));
    assert!(workflow.contains("git tag --list 'v*' --sort=-version:refname"));
}

#[test]
fn release_workflow_accepts_stable_and_rc_provenance() {
    let workflow = read(".github/workflows/release.yml");

    assert!(workflow.contains("X.Y.Z or RC X.Y.Z-rc.N"));
    assert!(workflow.contains("PYPI_VERSION=\"${BASH_REMATCH[1]}rc${BASH_REMATCH[2]}\""));
    assert!(workflow.contains("PYPI_VERSION=\"$TAG_VERSION\""));
    assert!(workflow.contains("Expected pyproject.toml version $PYPI_VERSION"));
    assert!(workflow.contains("CHANGELOG.md missing release section for ${TAG_VERSION}"));
    assert!(workflow.contains("if [[ \"$COMMIT_MSG\" != \"release: v$TAG_VERSION\" ]]; then"));
}

#[test]
fn release_workflow_marks_rc_prerelease_and_uses_npm_dist_tags() {
    let workflow = read(".github/workflows/release.yml");

    assert!(workflow.contains("prerelease: ${{ steps.release_meta.outputs.prerelease }}"));
    assert!(workflow.contains("echo \"prerelease=true\" >> \"$GITHUB_OUTPUT\""));
    assert!(workflow.contains("echo \"npm_dist_tag=rc\" >> \"$GITHUB_OUTPUT\""));
    assert!(workflow.contains("echo \"npm_dist_tag=latest\" >> \"$GITHUB_OUTPUT\""));
    assert!(workflow.contains("npm publish --provenance --access public --tag \"$NPM_DIST_TAG\""));
}

#[test]
fn release_workflow_pypi_publish_uses_trusted_publisher_with_required_inputs() {
    let workflow = read(".github/workflows/release.yml");

    assert!(workflow.contains("needs: [pypi-wheels, pypi-sdist, verify-provenance]"));
    assert!(workflow.contains("uses: pypa/gh-action-pypi-publish@release/v1"));
    assert!(workflow.contains("packages-dir: dist"));
}
