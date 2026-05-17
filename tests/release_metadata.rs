// qa-validated: mars-release-workflow-audit
use std::path::Path;

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &str) -> String {
    std::fs::read_to_string(repo_root().join(path)).expect(path)
}

fn assert_before(haystack: &str, first: &str, second: &str) {
    let first_idx = haystack
        .find(first)
        .unwrap_or_else(|| panic!("missing marker: {}", first));
    let second_idx = haystack
        .find(second)
        .unwrap_or_else(|| panic!("missing marker: {}", second));
    assert!(
        first_idx < second_idx,
        "expected marker order: `{}` before `{}`",
        first,
        second
    );
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

    assert!(!workflow.contains("2>/dev/null || printf '[]'"));
    assert!(workflow.contains("exact merge_commit_sha match"));
    assert!(workflow.contains(".merge_commit_sha == $trigger_sha"));
    assert!(workflow.contains("selection_reason=\"merged PR targeting main fallback\""));
    assert!(workflow.contains("if [[ \"${candidate_count}\" -gt 1 ]]; then"));
    assert!(workflow.contains("Ambiguous merged PR selection for trigger ${TRIGGER_SHA}."));
    assert!(
        workflow.contains("labels=\"$(jq -r '.labels[]?.name' <<<\"${selected_pr}\" | sort -u)\"")
    );
    assert!(
        !workflow.contains("labels=\"$(jq -r '.[].labels[].name' <<<\"${prs_json}\" | sort -u)\"")
    );
    assert!(workflow.contains("if [[ \"${candidate_count}\" -eq 0 ]]; then"));
    assert!(workflow.contains("echo \"should_release=false\" >> \"$GITHUB_OUTPUT\""));
    assert!(workflow.contains("release:skip"));
    assert!(workflow.contains("release:(skip|patch|stable|rc)"));
    assert!(workflow.contains("release_kind=\"rc\""));
    assert!(workflow.contains("if [[ -n \"${unknown_release_labels}\" ]]; then"));
    assert!(workflow.contains("elif [[ \"${has_rc_label}\" == \"true\" ]]; then"));
    assert!(workflow.contains("elif [[ \"${has_stable_label}\" == \"true\" ]]; then"));
    assert!(workflow.contains("release_kind=\"stable\""));
    assert!(workflow.contains("echo \"release_kind=${release_kind}\" >> \"$GITHUB_OUTPUT\""));

    assert_before(
        &workflow,
        "if grep -qx 'release:skip' <<<\"${labels}\"; then",
        "release_labels=\"$(grep '^release:' <<<\"${labels}\" || true)\"",
    );
    assert_before(
        &workflow,
        "if [[ -n \"${unknown_release_labels}\" ]]; then",
        "elif [[ \"${has_rc_label}\" == \"true\" ]]; then",
    );
    assert_before(
        &workflow,
        "elif [[ \"${has_rc_label}\" == \"true\" ]]; then",
        "elif [[ \"${has_stable_label}\" == \"true\" ]]; then",
    );
}

#[test]
fn release_on_main_uses_trigger_marker_and_rerun_tag_recovery() {
    let workflow = read(".github/workflows/release-on-main.yml");

    assert!(workflow.contains(
        "git commit -m \"release: v${NEXT_VERSION}\" -m \"Release-Trigger: ${TRIGGER_SHA}\""
    ));
    assert!(workflow.contains("grep -Fxq \"Release-Trigger: ${TRIGGER_SHA}\""));
    assert!(workflow.contains("echo \"tag_missing=true\" >> \"$GITHUB_OUTPUT\""));
    assert!(workflow.contains("echo \"missing_tag=${selected_tag}\" >> \"$GITHUB_OUTPUT\""));
    assert!(workflow.contains("if: steps.release_intent.outputs.should_release == 'true' && steps.release_guard.outputs.tag_missing == 'true'"));
    assert!(workflow.contains("id: push_missing_tag"));
    assert!(workflow.contains(
        "git tag -a \"${MISSING_TAG}\" \"${MISSING_TAG_COMMIT}\" -m \"Release ${MISSING_TAG#v}\""
    ));
    assert!(workflow.contains(
        "Tag ${MISSING_TAG} already exists on ${tag_commit}, expected ${MISSING_TAG_COMMIT}."
    ));
    assert!(workflow.contains("echo \"tag=${MISSING_TAG}\" >> \"$GITHUB_OUTPUT\""));
    assert!(workflow.contains("steps.push_release.outputs.tag || steps.push_missing_tag.outputs.tag || steps.release_guard.outputs.existing_release_tag"));
}

#[test]
fn release_on_main_computes_stable_and_rc_versions() {
    let workflow = read(".github/workflows/release-on-main.yml");

    assert!(
        workflow
            .contains("if grep -Eq '^v[0-9]+\\.[0-9]+\\.[0-9]+$' <<<\"${candidate_tag}\"; then"),
        "stable base version lookup must ignore prerelease tags"
    );
    assert!(workflow.contains("if [[ \"${RELEASE_KIND}\" == \"stable\" ]]; then"));
    assert!(
        workflow.contains(
            "while git rev-parse -q --verify \"refs/tags/v${next_version}\" >/dev/null; do"
        ),
        "stable releases must advance past stable tag collisions"
    );
    assert!(workflow.contains("python_version=\"${next_version}\""));
    assert!(workflow.contains("max_rc=0"));
    assert!(workflow.contains("if (( rc_number > max_rc )); then"));
    assert!(workflow.contains("next_rc=$((max_rc + 1))"));
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
    assert!(workflow.contains("Tag version ($TAG_VERSION) != Cargo.toml ($CARGO_VERSION)"));
    assert!(workflow.contains("Expected pyproject.toml version $PYPI_VERSION"));
    assert!(workflow.contains("npm_packages=(npm/@meridian-flow/mars-agents*/package.json)"));
    assert!(workflow.contains("jq -r '.version // empty'"));
    assert!(workflow.contains("optionalDependencies // {} | to_entries[]"));
    assert!(workflow.contains("CHANGELOG.md missing release section for ${TAG_VERSION}"));
    assert!(
        workflow
            .contains("git merge-base --is-ancestor \"$RELEASE_SHA\" \"origin/$DEFAULT_BRANCH\"")
    );
    assert!(workflow.contains("if [[ \"$COMMIT_MSG\" != \"release: v$TAG_VERSION\" ]]; then"));
}

#[test]
fn release_workflow_marks_rc_prerelease_and_uses_npm_dist_tags() {
    let workflow = read(".github/workflows/release.yml");

    assert!(workflow.contains("prerelease: ${{ steps.release_meta.outputs.prerelease }}"));
    assert!(workflow.contains("echo \"prerelease=true\" >> \"$GITHUB_OUTPUT\""));
    assert!(workflow.contains("echo \"prerelease=false\" >> \"$GITHUB_OUTPUT\""));
    assert!(workflow.contains("echo \"npm_dist_tag=rc\" >> \"$GITHUB_OUTPUT\""));
    assert!(workflow.contains("echo \"npm_dist_tag=latest\" >> \"$GITHUB_OUTPUT\""));
    assert!(workflow.contains("npm publish --provenance --access public --tag \"$NPM_DIST_TAG\""));
    assert_eq!(
        workflow
            .matches("npm publish --provenance --access public --tag \"$NPM_DIST_TAG\"")
            .count(),
        2,
        "expected dist-tagged publish in platform and stub npm publish steps"
    );
}

#[test]
fn release_workflow_pypi_publish_uses_trusted_publisher_with_required_inputs() {
    let workflow = read(".github/workflows/release.yml");

    assert!(workflow.contains("needs: [pypi-wheels, pypi-sdist, verify-provenance]"));
    assert!(workflow.contains("uses: pypa/gh-action-pypi-publish@release/v1"));
    assert!(workflow.contains("packages-dir: dist"));
}

#[test]
fn release_workflow_cargo_publish_only_ignores_already_uploaded_errors() {
    let workflow = read(".github/workflows/release.yml");

    assert!(workflow.contains("publish_stderr=\"$(mktemp)\""));
    assert!(workflow.contains("if cargo publish 2> >(tee \"${publish_stderr}\" >&2); then"));
    assert!(workflow.contains("grep -Eiq 'already (uploaded|published)|already exists'"));
    assert!(workflow.contains("Crate version already published on crates.io; continuing."));
    assert!(workflow.contains("cargo publish failed with an unexpected error."));
    assert!(!workflow.contains("cargo publish || true"));
}
