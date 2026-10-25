//! Published schema-v1 reader contract fixtures.

use std::{fs, path::Path};

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
