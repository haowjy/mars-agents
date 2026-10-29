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
    // Only the OpenCode tree gets the adapter, and it is what sync renders for `.opencode`.
    for other in ["claude", "cursor"] {
        assert!(
            !dir.path()
                .join("out")
                .join(other)
                .join("hooks/guard/mars-guard.hooks.ts")
                .exists(),
            "{other} tree has the adapter"
        );
    }
    let pkg = dir.path().join("pkg/hooks/guard/hook.toml");
    let origin = mars_schema::hooks::Origin::Local;
    let def = mars_schema::hooks::read(&fs::read_to_string(pkg).unwrap(), "guard", origin)
        .def
        .unwrap();
    let tokens = mars_schema::hooks::Tokens {
        hook_dir: mars_schema::hooks::HOOK_DIR_TOKEN,
        hook_target: ".opencode",
        ..Default::default()
    };
    let target = mars_schema::hooks::targets::target(".opencode").unwrap();
    let rendered = match def.output(target, None, origin, &tokens).unwrap() {
        mars_schema::hooks::HookOutput::File(file) => file.files[0].content.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(adapter, rendered);
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

/// A native hook's `${MARS_HOOK_TARGET}` is filled with `.opencode`, as a sync install does.
#[test]
fn opencode_plugin_fills_target_token_in_native_hook() {
    let native = "[targets.\".opencode\"]\nnative-hook = \"opencode.ts\"\n";
    let source = "export default { id: \"${MARS_HOOK_TARGET}\", setup() {} }\n";
    let (dir, data, _) = build_hooks(&[("only", native, &[("opencode.ts", source)])]);
    assert_eq!(data["tokens"]["hookTarget"], "${MARS_HOOK_TARGET}");
    let index = fs::read_to_string(dir.path().join("out/opencode/index.mjs")).unwrap();
    assert!(
        index.contains("replaceAll(HOOK_TARGET_TOKEN, () => \".opencode\")"),
        "{index}"
    );
    // The tree keeps the token; `index.mjs` fills it when it loads the hook.
    let file = fs::read_to_string(dir.path().join("out/opencode/hooks/only/opencode.ts")).unwrap();
    assert!(file.contains("${MARS_HOOK_TARGET}"));
}

/// Events and answers the adapter cannot carry are reported for the OpenCode tree.
#[test]
fn opencode_plugin_reports_dropped_events_and_answers() {
    let toml = format!(
        "{PORTABLE}[[hooks.PermissionDenied]]\n[[hooks.PermissionDenied.hooks]]\ntype = \"command\"\ncommand = \"true\"\n"
    );
    let (_dir, _, diagnostics) = build_hooks(&[("two", &toml, &[("run.sh", "")])]);
    assert!(
        diagnostics.contains(
            "hook `two` from `pkg`: the `opencode` plugin tree does not run `PermissionDenied`"
        ),
        "{diagnostics}"
    );
}
