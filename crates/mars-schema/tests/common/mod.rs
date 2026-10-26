//! Shared fixture helpers. `MARS_UPDATE_FIXTURES=1 cargo test -p mars-schema` rewrites
//! expected outputs from the current readers; review the diff before committing.
#![allow(dead_code)] // each test binary uses a subset

use std::{
    fs,
    path::{Path, PathBuf},
};

/// The case directories of one fixture suite, sorted.
pub fn cases(suite: &str) -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(suite);
    let mut cases: Vec<_> = fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .collect();
    cases.sort();
    cases
}

/// Compare `actual` with the file at `expected`, or rewrite the file when updating.
pub fn assert_file(expected: &Path, actual: &str) {
    if std::env::var_os("MARS_UPDATE_FIXTURES").is_some() {
        fs::write(expected, actual).unwrap();
        return;
    }
    let want = fs::read_to_string(expected).unwrap();
    assert_eq!(actual, want, "fixture {}", expected.display());
}

/// Pretty JSON with a trailing newline, the on-disk form of `expected.json`.
pub fn pretty(value: &impl serde::Serialize) -> String {
    serde_json::to_string_pretty(value).unwrap() + "\n"
}

/// The in-memory package tree under `root`.
pub fn load_tree(root: &Path) -> mars_schema::tree::FileTree {
    let mut files = Vec::new();
    collect_files(root, root, &mut files);
    mars_schema::tree::FileTree::from_files(files).unwrap()
}

pub fn collect_files(root: &Path, dir: &Path, files: &mut Vec<(String, Vec<u8>)>) {
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

/// A serialized result with its `normalized_bytes` left out, which the fixtures check against
/// files; through the ordered type, so key order stays in the expected file.
pub fn without_bytes(value: &impl serde::Serialize) -> mars_schema::ordered::OrderedValue {
    use mars_schema::ordered::OrderedValue;
    fn strip(value: &mut OrderedValue) {
        match value {
            OrderedValue::Object(map) => {
                map.shift_remove("normalized_bytes");
                map.values_mut().for_each(strip);
            }
            OrderedValue::Array(values) => values.iter_mut().for_each(strip),
            _ => {}
        }
    }
    let mut value: OrderedValue =
        serde_json::from_str(&serde_json::to_string(value).unwrap()).unwrap();
    strip(&mut value);
    value
}
