use std::{fs, path::Path};

use mars_schema::{
    dialect::Dialect,
    prepare::{ChangeRecord, PrepareContext, Prepared, prepare, prepare_for_migration},
    tree::TreeSource,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct Request {
    context: PrepareContext,
    #[serde(default)]
    declared_dialect: Option<Dialect>,
    /// Preparation as `mars migrate` runs it: the written-back repair is part of the case.
    #[serde(default)]
    migrate: bool,
}

mod common;

#[test]
fn prepare_fixtures_match() {
    for case in common::cases("prepare") {
        let request: Request =
            serde_json::from_slice(&fs::read(case.join("request.json")).unwrap()).unwrap();
        let tree = common::load_tree(&case.join("input"));
        let prepared = if request.migrate {
            prepare_for_migration(&tree, request.context, request.declared_dialect)
        } else {
            prepare(&tree, request.context, request.declared_dialect)
        };
        // The bytes are checked against `expected/` below, not inlined in `expected.json`.
        common::assert_file(
            &case.join("expected.json"),
            &common::pretty(&common::without_bytes(&prepared)),
        );
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
    if std::env::var_os("MARS_UPDATE_FIXTURES").is_some() {
        let _ = fs::remove_dir_all(&expected_root);
        for (path, bytes) in &changed {
            let target = expected_root.join(path);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::write(target, bytes).unwrap();
        }
    }
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
