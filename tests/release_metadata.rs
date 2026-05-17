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
fn release_on_main_job_guard_skips_release_and_skip_commits() {
    let workflow = read(".github/workflows/release-on-main.yml");

    assert!(workflow.contains("startsWith(github.event.head_commit.message, 'release: v')"));
    assert!(workflow.contains("contains(github.event.head_commit.message, 'release:skip')"));
}

#[test]
fn release_on_main_anchors_release_to_current_main_tip() {
    let workflow = read(".github/workflows/release-on-main.yml");

    assert!(workflow.contains("name: Anchor release on current main"));
    assert!(workflow.contains("git fetch origin main --force"));
    assert!(workflow.contains("git checkout -B main origin/main"));
    assert!(workflow.contains("git cat-file -e \"${TRIGGER_SHA}^{commit}\""));
    assert!(workflow.contains("git merge-base --is-ancestor \"${TRIGGER_SHA}\" HEAD"));

    assert_before(
        &workflow,
        "git fetch origin main --force",
        "git checkout -B main origin/main",
    );
    assert_before(
        &workflow,
        "git cat-file -e \"${TRIGGER_SHA}^{commit}\"",
        "git merge-base --is-ancestor \"${TRIGGER_SHA}\" HEAD",
    );
}

#[test]
fn release_on_main_release_intent_uses_label_contract_and_safe_defaults() {
    let workflow = read(".github/workflows/release-on-main.yml");

    assert!(workflow.contains("if [[ \"${pr_count}\" -eq 0 ]]; then"));
    assert!(workflow.contains("echo \"should_release=false\" >> \"$GITHUB_OUTPUT\""));

    assert!(workflow.contains("if grep -qx 'release:skip' <<<\"${labels}\"; then"));
    assert!(workflow.contains("release_labels=\"$(grep '^release:' <<<\"${labels}\" || true)\""));
    assert!(workflow.contains("if [[ -z \"${release_labels}\" ]]; then"));
    assert!(workflow.contains(
        "unknown_release_labels=\"$(grep '^release:' <<<\"${labels}\" | grep -vxE 'release:(skip|patch|stable|rc)' || true)\""
    ));
    assert!(workflow.contains("release_kind=\"rc\""));
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
fn release_on_main_duplicate_guard_is_release_kind_aware() {
    let workflow = read(".github/workflows/release-on-main.yml");

    assert!(workflow.contains("RELEASE_KIND: ${{ steps.release_intent.outputs.release_kind }}"));
    assert!(workflow.contains("is_stable_tag()"));
    assert!(workflow.contains("is_rc_tag()"));
    assert!(workflow.contains("release_kind=\"${RELEASE_KIND:-rc}\""));
    assert!(workflow.contains("[[ \"${commit_subject}\" == release:\\ v* ]] || continue"));
    assert!(
        workflow.contains("git merge-base --is-ancestor \"${TRIGGER_SHA}\" \"${parent_commit}\"")
    );
    assert!(workflow.contains("git tag --points-at \"${commit_hash}\" --list 'v*'"));
    assert!(workflow.contains("if is_stable_tag \"${commit_tag}\"; then"));
    assert!(workflow.contains("if is_rc_tag \"${commit_tag}\"; then"));
    assert!(workflow.contains("if [[ \"${release_kind}\" == \"stable\" ]]; then"));
    assert!(workflow.contains("continue"));
    assert!(workflow.contains(
        "continue\n                fi\n                existing_release_tag=\"${commit_tag}\""
    ));
    assert!(workflow.contains("echo \"already_released=true\" >> \"$GITHUB_OUTPUT\""));
    assert!(workflow.contains("echo \"already_released=false\" >> \"$GITHUB_OUTPUT\""));

    assert_before(
        &workflow,
        "if is_rc_tag \"${commit_tag}\"; then",
        "if [[ \"${release_kind}\" == \"stable\" ]]; then",
    );
}

#[test]
fn release_on_main_pushes_tag_with_github_token() {
    let workflow = read(".github/workflows/release-on-main.yml");

    assert!(workflow.contains("TAG_PUSH_TOKEN: ${{ github.token }}"));
    assert!(workflow.contains("git push \"https://x-access-token:${TAG_PUSH_TOKEN}@github.com/${REPOSITORY}.git\" \"refs/tags/v${NEXT_VERSION}\""));
}

#[test]
fn release_on_main_computes_release_versions_and_updates_files() {
    let workflow = read(".github/workflows/release-on-main.yml");

    assert!(workflow.contains("if [[ \"${RELEASE_KIND}\" == \"stable\" ]]; then"));
    assert!(
        workflow.contains(
            "while git rev-parse -q --verify \"refs/tags/v${next_version}\" >/dev/null; do"
        )
    );
    assert!(workflow.contains("python_version=\"${next_version}\""));
    assert!(workflow.contains("done < <(git tag --list \"v${next_patch}-rc.*\")"));
    assert!(workflow.contains("next_version=\"${next_patch}-rc.${next_rc}\""));
    assert!(workflow.contains("python_version=\"${next_patch}rc${next_rc}\""));

    assert!(workflow.contains(
        "sed -i -E \"0,/^version = \\\".*\\\"/s//version = \\\"${NEXT_VERSION}\\\"/\" Cargo.toml"
    ));
    assert!(workflow.contains("sed -i -E \"0,/^version = \\\".*\\\"/s//version = \\\"${PYTHON_VERSION}\\\"/\" pyproject.toml"));
    assert!(workflow.contains("pkg.version = '${NEXT_VERSION}';"));
    assert!(workflow.contains("pkg.optionalDependencies[dep] = '${NEXT_VERSION}';"));
}

#[test]
fn release_on_main_invokes_release_workflow_with_tag_and_permissions() {
    let workflow = read(".github/workflows/release-on-main.yml");

    assert!(workflow.contains("publish:"));
    assert!(workflow.contains("uses: ./.github/workflows/release.yml"));
    assert!(workflow.contains("tag: ${{ needs.release.outputs.tag }}"));
    assert!(workflow.contains("secrets: inherit"));
    assert!(workflow.contains("id-token: write"));
}

#[test]
fn release_workflow_provenance_contract_covers_stable_and_rc_tags() {
    let workflow = read(".github/workflows/release.yml");

    assert!(
        workflow.contains(
            "if [[ \"$TAG_VERSION\" =~ ^([0-9]+\\.[0-9]+\\.[0-9]+)-rc\\.([0-9]+)$ ]]; then"
        )
    );
    assert!(workflow.contains("PYPI_VERSION=\"${BASH_REMATCH[1]}rc${BASH_REMATCH[2]}\""));
    assert!(workflow.contains("elif [[ \"$TAG_VERSION\" =~ ^[0-9]+\\.[0-9]+\\.[0-9]+$ ]]; then"));
    assert!(workflow.contains("PYPI_VERSION=\"$TAG_VERSION\""));
    assert!(workflow.contains("Tag version ($TAG_VERSION) must be stable X.Y.Z or RC X.Y.Z-rc.N"));
    assert!(workflow.contains("Expected pyproject.toml version $PYPI_VERSION"));

    assert!(workflow.contains("npm_packages=(npm/@meridian-flow/mars-agents*/package.json)"));
    assert!(workflow.contains("jq -r '.version // empty'"));
    assert!(workflow.contains("optionalDependencies // {} | to_entries[]"));

    assert!(workflow.contains("escaped_tag_version="));
    assert!(workflow.contains(r#"grep -qE "^## \\[${escaped_tag_version}\\] - " CHANGELOG.md"#,));
    assert!(workflow.contains("CHANGELOG.md missing release section for ${TAG_VERSION}"));

    assert!(
        workflow
            .contains("git merge-base --is-ancestor \"$RELEASE_SHA\" \"origin/$DEFAULT_BRANCH\"")
    );
    assert!(workflow.contains("COMMIT_MSG=$(git log -1 --format=%s \"$RELEASE_SHA\")"));
    assert!(workflow.contains("if [[ \"$COMMIT_MSG\" != \"release: v$TAG_VERSION\" ]]; then"));
}

#[test]
fn release_workflow_publish_metadata_maps_prerelease_and_npm_dist_tags() {
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
fn ci_workflow_runs_windows_build_test_clippy_and_fmt() {
    let workflow = read(".github/workflows/ci.yml");

    assert!(workflow.contains("check-windows:"));
    assert!(workflow.contains("runs-on: windows-latest"));
    assert!(workflow.contains("cargo build"));
    assert!(workflow.contains("cargo test"));
    assert!(workflow.contains("cargo clippy -- -D warnings"));
    assert!(workflow.contains("cargo fmt --check"));
}
