use std::{fs, path::Path};

use mars_schema::upgrade::{DefinitionKind, upgrade_definition, upgrade_manifest};

#[test]
fn legacy_upgrade_fixtures() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/upgrade");
    let mut cases = fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    cases.sort();

    for case in cases {
        let kind = fs::read_to_string(case.join("kind.txt")).unwrap();
        let extension = if kind.trim() == "manifest" {
            "toml"
        } else {
            "md"
        };
        let input = fs::read_to_string(case.join(format!("input.{extension}"))).unwrap();
        let expected = fs::read_to_string(case.join(format!("expected.{extension}"))).unwrap();
        let result = match kind.trim() {
            "agent" => upgrade_definition(DefinitionKind::Agent, &input).unwrap(),
            "skill" => upgrade_definition(DefinitionKind::Skill, &input).unwrap(),
            "manifest" => upgrade_manifest(&input).unwrap(),
            other => panic!("unknown fixture kind {other}"),
        };
        assert_eq!(result.text, expected, "fixture {}", case.display());
        assert_eq!(
            result.fully_rewritten,
            case.join("fully-rewritten.txt").exists(),
            "fixture {} rewrite mode",
            case.display()
        );

        let expected_keys: Vec<String> = serde_json::from_str(
            &fs::read_to_string(case.join("expected-diagnostics.json")).unwrap(),
        )
        .unwrap();
        let messages = result
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>();
        for key in &expected_keys {
            assert!(
                messages
                    .iter()
                    .any(|message| message.contains(&format!("`{key}`"))),
                "fixture {} has no diagnostic naming `{key}`: {messages:?}",
                case.display()
            );
        }
        assert_eq!(
            result.diagnostics.len(),
            expected_keys.len(),
            "fixture {} diagnostic count",
            case.display()
        );

        let rerun = match kind.trim() {
            "agent" => upgrade_definition(DefinitionKind::Agent, &result.text).unwrap(),
            "skill" => upgrade_definition(DefinitionKind::Skill, &result.text).unwrap(),
            "manifest" => upgrade_manifest(&result.text).unwrap(),
            _ => unreachable!(),
        };
        assert_eq!(
            rerun.text,
            result.text,
            "fixture {} idempotence",
            case.display()
        );
    }
}
