//! Published schema-v1 reader contract fixtures.

use std::{
    fs,
    path::{Path, PathBuf},
};

use mars_schema::diagnostic::PathSegment;

mod common;

/// The package-relative path a read fixture's source file stands for.
fn fixture_path(dir: &Path) -> (&'static str, PathBuf) {
    if dir.join("agent.md").exists() {
        ("agents/agent.md", dir.join("agent.md"))
    } else if dir.join("skill.md").exists() {
        ("skills/skill/SKILL.md", dir.join("skill.md"))
    } else {
        ("mars.toml", dir.join("mars.toml"))
    }
}

#[test]
fn read_fixtures_match() {
    for dir in common::cases("read") {
        let (path, file) = fixture_path(&dir);
        let reading = mars_schema::read::read(path, &fs::read_to_string(file).unwrap()).unwrap();
        common::assert_file(
            &dir.join("expected.json"),
            &common::pretty(&serde_json::to_value(reading).unwrap()),
        );
    }
}

#[test]
fn read_refuses_paths_that_are_not_schema_files() {
    for path in [
        "README.md",
        "agents/dir/a.md",
        "agents/.md",
        "skills/a.md",
        "skills/a/b/SKILL.md",
        "skills//SKILL.md",
        "mars.lock",
        "",
    ] {
        assert!(mars_schema::read::read(path, "").is_err(), "{path}");
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
