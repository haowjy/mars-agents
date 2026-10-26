use std::{fs, path::Path};

use mars_schema::{
    dialect::Dialect,
    prepare::{ChangeRecord, PrepareContext, Prepared, prepare},
    tree::TreeSource,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct Request {
    context: PrepareContext,
    #[serde(default)]
    declared_dialect: Option<Dialect>,
}

mod common;

#[test]
fn prepare_fixtures_match() {
    for case in common::cases("prepare") {
        let request: Request =
            serde_json::from_slice(&fs::read(case.join("request.json")).unwrap()).unwrap();
        let tree = common::load_tree(&case.join("input"));
        let prepared = prepare(&tree, request.context, request.declared_dialect);
        // The bytes are checked against `expected/` below, not inlined in `expected.json`.
        let mut value = serde_json::to_value(&prepared).unwrap();
        strip_bytes(&mut value);
        common::assert_file(&case.join("expected.json"), &common::pretty(&value));
        assert_changed_bytes(&case, &prepared);
        for definition in prepared
            .definitions
            .iter()
            .filter(|definition| definition.change == ChangeRecord::None)
        {
            if let Some(original) = tree.read(&definition.source_path) {
                assert_eq!(
                    original,
                    definition.normalized_bytes,
                    "fixture {} unchanged {}",
                    case.display(),
                    definition.source_path
                );
            }
        }
    }
}

fn strip_bytes(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            map.remove("normalized_bytes");
            map.values_mut().for_each(strip_bytes);
        }
        serde_json::Value::Array(values) => values.iter_mut().for_each(strip_bytes),
        _ => {}
    }
}

fn changed_files(prepared: &Prepared) -> Vec<(&str, &[u8])> {
    let manifest = prepared
        .manifest
        .iter()
        .filter(|manifest| manifest.change != ChangeRecord::None)
        .map(|manifest| {
            (
                manifest.output_path.as_str(),
                manifest.normalized_bytes.as_slice(),
            )
        });
    let definitions = prepared
        .definitions
        .iter()
        .filter(|definition| definition.change != ChangeRecord::None)
        .map(|definition| {
            (
                definition.output_path.as_str(),
                definition.normalized_bytes.as_slice(),
            )
        });
    manifest.chain(definitions).collect()
}

fn assert_changed_bytes(case: &Path, prepared: &Prepared) {
    let expected_root = case.join("expected");
    let changed = changed_files(prepared);
    let expected_count = if expected_root.exists() {
        let mut files = Vec::new();
        common::collect_files(&expected_root, &expected_root, &mut files);
        files.len()
    } else {
        0
    };
    assert_eq!(changed.len(), expected_count, "fixture {}", case.display());
    for (path, bytes) in changed {
        assert_eq!(
            fs::read(expected_root.join(path)).unwrap(),
            bytes,
            "fixture {} normalized {path}",
            case.display()
        );
    }
}
