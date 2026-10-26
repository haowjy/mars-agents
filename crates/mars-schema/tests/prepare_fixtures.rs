use std::{fs, path::Path};

use mars_schema::{
    dialect::Dialect,
    prepare::{ChangeRecord, PrepareContext, Prepared, prepare},
    tree::FileTree,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct Request {
    context: PrepareContext,
    #[serde(default)]
    declared_dialect: Option<Dialect>,
}

#[test]
fn prepare_fixtures_match() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/prepare");
    let mut cases = child_directories(&root);
    cases.sort();

    for case in cases {
        let request: Request =
            serde_json::from_slice(&fs::read(case.join("request.json")).unwrap()).unwrap();
        let tree = load_tree(&case.join("input"));
        let prepared = prepare(&tree, request.context, request.declared_dialect);
        let actual = serde_json::to_string_pretty(&prepared).unwrap() + "\n";

        let expected = fs::read_to_string(case.join("expected.json")).unwrap();
        assert_eq!(actual, expected, "fixture {}", case.display());
        assert_changed_bytes(&case, &prepared);
    }
}

fn load_tree(root: &Path) -> FileTree {
    let mut files = Vec::new();
    collect_files(root, root, &mut files);
    FileTree::from_files(files).unwrap()
}

fn collect_files(root: &Path, dir: &Path, files: &mut Vec<(String, Vec<u8>)>) {
    let mut entries = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_files(root, &path, files);
        } else {
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .iter()
                .map(|component| component.to_str().unwrap())
                .collect::<Vec<_>>()
                .join("/");
            files.push((relative, fs::read(path).unwrap()));
        }
    }
}

fn child_directories(root: &Path) -> Vec<std::path::PathBuf> {
    fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .collect()
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
        collect_files(&expected_root, &expected_root, &mut files);
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
