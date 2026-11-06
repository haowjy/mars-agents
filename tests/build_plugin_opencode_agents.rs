mod common;

use assert_fs::TempDir;
use std::fs;

use common::*;

const PORTABLE: &str = "[[hooks.PreToolUse]]\nmatcher = \"Bash\"\n[[hooks.PreToolUse.hooks]]\ntype = \"command\"\ncommand = \"bash ${MARS_HOOK_DIR}/run.sh ${MARS_HOOK_TARGET}\"\n";
const NATIVE: &str = "export default { id: \"native\", setup() {} }\n";

/// An authored file named like the generated adapter fails the build instead of being replaced.
#[test]
fn opencode_plugin_rejects_authored_file_named_like_adapter() {
    let native = "[targets.\".opencode\"]\nnative-hook = \"mars-both.hooks.ts\"\n";
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("pkg");
    let hook = root.join("hooks/both");
    fs::create_dir_all(&hook).unwrap();
    fs::write(
        root.join("mars.toml"),
        "[package]\nname = \"pkg\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(
        hook.join("hook.toml"),
        format!("visibility = \"exported\"\n{PORTABLE}{native}"),
    )
    .unwrap();
    fs::write(hook.join("mars-both.hooks.ts"), NATIVE).unwrap();
    fs::write(hook.join("run.sh"), "").unwrap();
    let output = offline_mars(dir.path())
        .args(["build", "plugin", "--root"])
        .arg(&root)
        .arg("--out")
        .arg(dir.path().join("out"))
        .output()
        .unwrap();
    assert!(!output.status.success(), "{output:?}");
    let text = String::from_utf8_lossy(&output.stderr).into_owned()
        + &String::from_utf8_lossy(&output.stdout);
    assert!(
        text.contains("hook `both`")
            && text.contains("mars-both.hooks.ts")
            && text.contains("may not use it"),
        "{text}"
    );
}
