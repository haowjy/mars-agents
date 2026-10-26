//! Published emission contract: `fixtures/emit/<case>/{value.json, expected.txt}` pins the bytes
//! `emit_value` writes, and each case must read back as the input value once spliced into a
//! real frontmatter at its indent. Refusal cases carry `expected-error.txt` instead.

use std::fs;

use indexmap::IndexMap;
use mars_schema::ordered::OrderedValue as Value;

mod common;

#[derive(serde::Deserialize)]
struct Case {
    value: Value,
    indent: usize,
}

/// Splice `emitted` in as the value of a key at column `indent`, nested under parent keys at
/// columns 0, 2, ...; the value then reads back at `x-test` (and `k` for each level below).
fn splice(emitted: &str, indent: usize) -> String {
    let mut text = String::from("---\nname: probe\n");
    for level in 0..=indent / 2 {
        let key = if level == 0 { "x-test" } else { "k" };
        text.push_str(&format!("{}{key}:", " ".repeat(level * 2)));
        if level * 2 == indent {
            if !emitted.starts_with('\n') {
                text.push(' ');
            }
            text.push_str(emitted);
        }
        text.push('\n');
    }
    text + "---\nbody\n"
}

fn nest(value: &Value, indent: usize) -> Value {
    (0..indent / 2).fold(value.clone(), |inner, _| {
        Value::Object(IndexMap::from([("k".to_owned(), inner)]))
    })
}

#[test]
fn emit_fixtures_match_and_round_trip() {
    for case in common::cases("emit") {
        let input: Case =
            serde_json::from_slice(&fs::read(case.join("value.json")).unwrap()).unwrap();
        let (value, indent) = (&input.value, input.indent);
        let emitted = mars_schema::edit::emit_value(value, indent);
        if case.join("expected-error.txt").exists() {
            let message = emitted.unwrap_err().to_string();
            let want = fs::read_to_string(case.join("expected-error.txt")).unwrap();
            assert!(
                message.contains(want.trim()),
                "fixture {}: {message}",
                case.display()
            );
            continue;
        }
        let emitted = emitted.unwrap();
        common::assert_file(&case.join("expected.txt"), &emitted);
        assert!(!emitted.ends_with('\n'), "fixture {}", case.display());

        let reading = mars_schema::agent::read(&splice(&emitted, indent));
        let errors: Vec<_> = reading
            .diagnostics
            .iter()
            .filter(|d| d.code.starts_with("yaml."))
            .collect();
        assert!(errors.is_empty(), "fixture {}: {errors:?}", case.display());
        let read_back = reading
            .frontmatter
            .iter()
            .find(|(key, _)| key == "x-test")
            .map(|(_, value)| value);
        assert_eq!(
            read_back,
            Some(&nest(value, indent)),
            "fixture {}",
            case.display()
        );
    }
}

#[test]
fn rewrite_block_round_trips_a_frontmatter() {
    let source = "---\nname: a\ndescription: \"x\\ny\\n\"\ntools:\n  \"*\": deny\n  read: allow\nx-keys:\n  \"#k\": 1\n  \"- x\": [1, {a: b}]\n---\nbody\n";
    let first = mars_schema::agent::read(source);
    assert!(
        first
            .diagnostics
            .iter()
            .all(|d| !d.code.starts_with("yaml.")),
        "{:?}",
        first.diagnostics
    );
    let block = mars_schema::edit::rewrite_block(&first.frontmatter).unwrap();
    assert!(block.ends_with('\n') && !block.ends_with("\n\n"));
    let second = mars_schema::agent::read(&format!("---\n{block}---\nbody\n"));
    assert_eq!(first.frontmatter, second.frontmatter);
    assert_eq!(mars_schema::edit::rewrite_block(&[]).unwrap(), "");
}

#[test]
fn rewrite_block_refuses_duplicate_keys() {
    let pairs = [
        ("a".to_owned(), Value::Number(1.into())),
        ("a".to_owned(), Value::Number(2.into())),
    ];
    assert!(mars_schema::edit::rewrite_block(&pairs).is_err());
}

/// Nested mapping order survives read → rewrite_block → read, and emission.
#[test]
fn nested_key_order_survives() {
    let source = "---\nz: 1\ntools:\n  write: allow\n  \"*\": deny\n  bash: ask\nharness-overrides:\n  flow:\n    zeta: {b: 1, a: 2}\n    alpha: [{y: 1, x: 2}]\n---\nb\n";
    let first = mars_schema::agent::read(source);
    let block = mars_schema::edit::rewrite_block(&first.frontmatter).unwrap();
    assert_eq!(
        block,
        "z: 1\ntools:\n  write: allow\n  \"*\": deny\n  bash: ask\nharness-overrides:\n  flow:\n    zeta:\n      b: 1\n      a: 2\n    alpha:\n      - \"y\": 1\n        x: 2\n"
    );
    let serialized = serde_json::to_string(&first.frontmatter).unwrap();
    assert!(
        serialized.contains(r#"{"write":"allow","*":"deny","bash":"ask"}"#),
        "{serialized}"
    );
    let fields = serde_json::to_string(&first.fields).unwrap();
    assert!(
        fields.contains(
            r#""harness-overrides":{"flow":{"zeta":{"b":1,"a":2},"alpha":[{"y":1,"x":2}]}}"#
        ),
        "{fields}"
    );
}
