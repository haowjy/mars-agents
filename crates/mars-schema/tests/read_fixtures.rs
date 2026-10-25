//! Published schema-v1 reader contract fixtures.

use std::{fs, path::Path};

use mars_schema::diagnostic::PathSegment;

#[test]
fn read_fixtures_match() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/read");
    for entry in fs::read_dir(root).unwrap() {
        let dir = entry.unwrap().path();
        if !dir.is_dir() {
            continue;
        }
        let actual = if dir.join("agent.md").exists() {
            serde_json::to_value(mars_schema::agent::read(
                &fs::read_to_string(dir.join("agent.md")).unwrap(),
            ))
            .unwrap()
        } else if dir.join("skill.md").exists() {
            serde_json::to_value(mars_schema::skill::read(
                &fs::read_to_string(dir.join("skill.md")).unwrap(),
            ))
            .unwrap()
        } else {
            serde_json::to_value(mars_schema::manifest::read(
                &fs::read_to_string(dir.join("mars.toml")).unwrap(),
            ))
            .unwrap()
        };
        let expected: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(dir.join("expected.json")).unwrap()).unwrap();
        assert_eq!(actual, expected, "fixture {}", dir.display());
    }
}

#[test]
fn diagnostic_paths_preserve_authored_keys() {
    let read = mars_schema::agent::read(
        "---\nfuture.key: value\ntools:\n  read(foo.bar): nope\nharness-overrides:\n  claude:\n    a[b]: null\n---\n",
    );
    let paths: Vec<_> = read
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.path.clone())
        .collect();
    assert!(paths.contains(&vec![PathSegment::Key("future.key".into())]));
    assert!(paths.contains(&vec![
        PathSegment::Key("tools".into()),
        PathSegment::Key("read(foo.bar)".into()),
    ]));
    assert!(paths.contains(&vec![
        PathSegment::Key("harness-overrides".into()),
        PathSegment::Key("claude".into()),
        PathSegment::Key("a[b]".into()),
    ]));
}

#[test]
fn structural_scan_handles_unicode_and_parser_disagreement() {
    let cjk = mars_schema::agent::read(
        "---\nname: a\ndescription: 汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉汉\n---\nbody",
    );
    assert!(cjk.diagnostics.is_empty(), "{:?}", cjk.diagnostics);

    let document_end = mars_schema::agent::read("---\nname: é\n...\n---\nbody");
    assert!(
        document_end
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "yaml.document-end")
    );

    let disagreement = mars_schema::agent::read(
        "---\nname: &x a\nmodel: *x\ndescription: \"foo\nbar\"\n---\nbody",
    );
    assert!(
        disagreement
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "yaml.anchor")
    );
    assert!(
        disagreement
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "yaml.unsupported-syntax")
    );
}
