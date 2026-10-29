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
