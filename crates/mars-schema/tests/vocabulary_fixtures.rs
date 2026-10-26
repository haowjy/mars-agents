//! The published tool vocabulary (`fixtures/vocabulary/expected.json`).

use std::path::Path;

mod common;

#[test]
fn vocabulary_matches_fixture() {
    let actual = serde_json::to_value(mars_schema::tool_natives::vocabulary()).unwrap();
    common::assert_file(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/vocabulary/expected.json"),
        &common::pretty(&actual),
    );
}

#[test]
fn ls_and_work_are_known_capabilities() {
    let read = mars_schema::agent::read("---\ntools:\n  ls: deny\n  work: deny\n---\n");
    assert!(
        read.diagnostics
            .iter()
            .all(|d| !d.message.contains("unknown")),
        "{:?}",
        read.diagnostics
    );
}
