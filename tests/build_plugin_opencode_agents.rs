mod common;

use assert_fs::TempDir;
use std::fs;

use common::*;

/// The OpenCode tree carries a published agent as plugin data, with OpenCode's v1 permission
/// keys translated to the v2 actions the plugin API matches on.
#[test]
fn opencode_plugin_registers_agent_with_v2_permissions() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("pkg");
    fs::create_dir_all(root.join("agents")).unwrap();
    fs::write(
        root.join("mars.toml"),
        "[package]\nname = \"pkg\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(
        root.join("agents/reader.md"),
        "---\nname: reader\ndescription: Reads\nmode: subagent\ndisallowed-tools: [bash, agent, write]\n---\nBody\n",
    )
    .unwrap();
    let out = dir.path().join("out");

    let output = offline_mars(dir.path())
        .args(["build", "plugin", "--root"])
        .arg(&root)
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");

    let index = fs::read_to_string(out.join("opencode/index.mjs")).unwrap();
    let data = index
        .split_once("const DATA = ")
        .and_then(|(_, rest)| rest.split_once("\nconst ROOT"))
        .map(|(data, _)| data.trim())
        .expect("index.mjs embeds DATA");
    let data: serde_json::Value = serde_json::from_str(data).unwrap();
    let agent = &data["agents"][0];
    assert_eq!(agent["id"], "pkg:reader");
    assert_eq!(agent["mode"], "subagent");
    let denied = agent["permissions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|rule| {
            (
                rule["action"].as_str().unwrap(),
                rule["effect"].as_str().unwrap(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        denied,
        [("shell", "deny"), ("subagent", "deny"), ("edit", "deny")]
    );
}

/// Build a package with the given hooks (`(name, hook.toml, extra files)`) and return the
/// OpenCode `DATA`, the output directory and the diagnostic codes.
type HookFixture<'a> = (&'a str, &'a str, &'a [(&'a str, &'a str)]);

fn build_hooks(hooks: &[HookFixture]) -> (TempDir, serde_json::Value, String) {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("pkg");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("mars.toml"),
        "[package]\nname = \"pkg\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    for (name, toml, files) in hooks {
        let hook = root.join("hooks").join(name);
        fs::create_dir_all(&hook).unwrap();
        fs::write(
            hook.join("hook.toml"),
            format!("visibility = \"exported\"\n{toml}"),
        )
        .unwrap();
        for (file, content) in *files {
            fs::write(hook.join(file), content).unwrap();
        }
    }
    let out = dir.path().join("out");
    let output = offline_mars(dir.path())
        .args(["build", "plugin", "--root"])
        .arg(&root)
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let index = fs::read_to_string(out.join("opencode/index.mjs")).unwrap();
    let data = index
        .split_once("const DATA = ")
        .and_then(|(_, rest)| rest.split_once("\nconst ROOT"))
        .map(|(data, _)| data.trim())
        .expect("index.mjs embeds DATA");
    let data = serde_json::from_str(data).unwrap();
    let diagnostics = String::from_utf8_lossy(&output.stderr).into_owned()
        + &String::from_utf8_lossy(&output.stdout);
    (dir, data, diagnostics)
}

const PORTABLE: &str = "[[hooks.PreToolUse]]\nmatcher = \"Bash\"\n[[hooks.PreToolUse.hooks]]\ntype = \"command\"\ncommand = \"bash ${MARS_HOOK_DIR}/run.sh ${MARS_HOOK_TARGET}\"\n";
const NATIVE: &str = "export default { id: \"native\", setup() {} }\n";

/// `[hooks]` alone ships the generated adapter, `index.mjs` loads it, and nothing is dropped.
#[test]
fn opencode_plugin_ships_adapter_generated_from_hooks() {
    let (dir, data, diagnostics) = build_hooks(&[("guard", PORTABLE, &[("run.sh", "")])]);
    assert_eq!(
        data["hooks"],
        serde_json::json!([{"name": "guard", "file": "mars-guard.hooks.ts"}])
    );
    let adapter = fs::read_to_string(
        dir.path()
            .join("out/opencode/hooks/guard/mars-guard.hooks.ts"),
    )
    .unwrap();
    assert!(
        adapter.contains("${MARS_HOOK_DIR}/run.sh .opencode"),
        "{adapter}"
    );
    assert!(
        !diagnostics.contains("OpenCode won't get it"),
        "{diagnostics}"
    );
}

/// A native hook alone loads as before; with `[hooks]` beside it, both load.
#[test]
fn opencode_plugin_loads_native_hook_beside_generated() {
    let native = "[targets.\".opencode\"]\nnative-hook = \"opencode.ts\"\n";
    let (_dir, data, _) = build_hooks(&[
        ("only", native, &[("opencode.ts", NATIVE)]),
        (
            "both",
            &format!("{PORTABLE}{native}"),
            &[("opencode.ts", NATIVE), ("run.sh", "")],
        ),
    ]);
    assert_eq!(
        data["hooks"],
        serde_json::json!([
            {"name": "both", "file": "opencode.ts"},
            {"name": "both", "file": "mars-both.hooks.ts"},
            {"name": "only", "file": "opencode.ts"},
        ])
    );
}

/// `[hooks]` that render nothing for OpenCode, with no native hook, still warn.
#[test]
fn opencode_plugin_warns_when_hooks_render_nothing() {
    let toml = "[[hooks.PermissionDenied]]\n[[hooks.PermissionDenied.hooks]]\ntype = \"command\"\ncommand = \"true\"\n";
    let (_dir, data, diagnostics) = build_hooks(&[("none", toml, &[])]);
    assert_eq!(data["hooks"], serde_json::json!([]));
    assert!(
        diagnostics.contains("OpenCode won't get it"),
        "{diagnostics}"
    );
}
