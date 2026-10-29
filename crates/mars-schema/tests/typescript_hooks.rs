//! Generated TypeScript for OpenCode and Pi: `fixtures/typescript-hooks/<case>/hook.toml` is
//! rendered for each target. `opencode.ts` and `pi.ts` pin the generated part of the file (its
//! header and the JSON literal of the surviving groups; the shared core and the glue are source
//! files, run live), and `report.txt` pins what was left out and which answers do not carry over.

use std::fmt::Write;

use mars_schema::hooks::{self, HookOutput, Origin, Tokens, targets};

mod common;

#[test]
fn rendered_typescript_matches_the_golden_files() {
    for case in common::cases("typescript-hooks") {
        let text = std::fs::read_to_string(case.join("hook.toml")).unwrap();
        let read = hooks::read(&text, "case", Origin::Local);
        let def = read
            .def
            .unwrap_or_else(|| panic!("{}: {:?}", case.display(), read.diagnostics));
        let mut report = String::new();
        for (root, file) in [(".opencode", "opencode.ts"), (".pi", "pi.ts")] {
            let target = targets::target(root).unwrap();
            let tokens = Tokens {
                hook_dir: "/p/hooks/case",
                hook_target: root,
                hook_name: "",
            };
            let Ok(HookOutput::File(output)) = def.output(target, None, Origin::Local, &tokens)
            else {
                panic!("{root} is a file target");
            };
            match output.files.as_slice() {
                [generated] => {
                    assert_eq!(generated.path, target.generated_path(&def.name).unwrap());
                    let (literal, code) = generated
                        .content
                        .split_once("\nimport ")
                        .expect("the core follows the literal");
                    assert!(code.contains("export default"), "{root} has glue");
                    common::assert_file(&case.join(file), &format!("{literal}\n"));
                }
                [] => assert!(!case.join(file).exists(), "{root} renders nothing"),
                _ => panic!("no fragment was given"),
            }
            writeln!(report, "{root}").unwrap();
            for skip in &output.skips {
                writeln!(
                    report,
                    "  skip {}: {}",
                    skip.event.as_deref().unwrap_or("-"),
                    skip.reason
                )
                .unwrap();
            }
            for limit in &output.answer_limits {
                writeln!(
                    report,
                    "  answers {}: {}",
                    limit.event.as_deref().unwrap_or("-"),
                    limit.reason
                )
                .unwrap();
            }
        }
        common::assert_file(&case.join("report.txt"), &report);
    }
}

const PRELUDE: &str = r#"const mod = await import(process.argv[2]);
const hooks = {};
const ctx = { location: { directory: process.cwd() } };
for (const g of ["tool", "session", "permission"]) ctx[g] = { hook: async (n, fn) => { hooks[`${g}.${n}`] = fn; } };
await mod.default.setup(ctx);
"#;

/// Loads the generated OpenCode file in node with a fake `ctx`, runs `script` (which sees the
/// registered `hooks` by name) and returns what it prints. `None` when node is not installed.
fn run_opencode(hook_toml: &str, script: &str) -> Option<String> {
    use std::process::Command;
    let read = hooks::read(hook_toml, "perm", Origin::Local);
    let def = read.def.unwrap_or_else(|| panic!("{:?}", read.diagnostics));
    let target = targets::target(".opencode").unwrap();
    let tokens = Tokens {
        hook_dir: "/p/hooks/perm",
        hook_target: ".opencode",
        hook_name: "",
    };
    let Ok(HookOutput::File(output)) = def.output(target, None, Origin::Local, &tokens) else {
        panic!("opencode is a file target");
    };
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("mars-ts-hooks-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("hook.ts"), &output.files[0].content).unwrap();
    std::fs::write(dir.join("drive.mjs"), format!("{PRELUDE}{script}\n")).unwrap();
    let run = Command::new("node")
        .arg(dir.join("drive.mjs"))
        .arg(dir.join("hook.ts"))
        .current_dir(&dir)
        .output()
        .ok()?;
    std::fs::remove_dir_all(&dir).ok();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    Some(String::from_utf8(run.stdout).unwrap().trim().to_owned())
}

#[test]
fn a_block_overrides_an_allow_in_opencode_permissions() {
    let allow = r#"echo '{"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"allow"}}}'"#;
    let toml = |second: &str| {
        format!(
            "[[hooks.PermissionRequest]]\nmatcher = \"Bash\"\n\
             [[hooks.PermissionRequest.hooks]]\ntype = \"command\"\ncommand = '''{allow}'''\n\
             [[hooks.PermissionRequest.hooks]]\ntype = \"command\"\ncommand = '''{second}'''\n"
        )
    };
    let Some(blocked) = opencode_permission_effect(&toml("echo no >&2; exit 2")) else {
        eprintln!("node is not installed; skipping");
        return;
    };
    assert_eq!(blocked, "deny");
    assert_eq!(
        opencode_permission_effect(&toml("exit 0")).unwrap(),
        "allow"
    );
}

fn opencode_permission_effect(hook_toml: &str) -> Option<String> {
    run_opencode(
        hook_toml,
        r#"const e = { effect: "ask", action: "bash", resources: ["ls"], sessionID: "s1" };
await hooks["permission.evaluate"](e);
console.log(e.effect);"#,
    )
}

#[test]
fn opencode_patch_is_an_edit_with_the_patch_as_command() {
    // The hook echoes its stdin, so the block reason shows `tool_name` and `tool_input`.
    let toml = "[[hooks.PreToolUse]]\nmatcher = \"Edit|Write\"\n\
                [[hooks.PreToolUse.hooks]]\ntype = \"command\"\ncommand = \"cat >&2; exit 2\"\n";
    let Some(out) = run_opencode(
        toml,
        r#"try {
  await hooks["tool.execute.before"]({ tool: "patch", input: { patchText: "*** Begin Patch" }, sessionID: "s1", id: "t1" });
  console.log("allowed");
} catch (err) { console.log(err.message); }"#,
    ) else {
        eprintln!("node is not installed; skipping");
        return;
    };
    let seen: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(seen["tool_name"], "Edit");
    assert_eq!(seen["tool_input"]["command"], "*** Begin Patch");
}
