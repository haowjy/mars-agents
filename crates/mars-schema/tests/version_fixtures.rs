//! Published version-rule contract (`fixtures/version/<case>/{input,expected}.json`): constraint
//! text, tag names and requirement text in, the choice or the refusal out.

use std::{collections::HashSet, fs};

use mars_schema::version::{
    Constraint, SelectionPolicy, parse_constraint, parse_tag, satisfies, satisfies_requirement,
    select,
};
use semver::Version;
use serde_json::{Value, json};

mod common;

fn strings(input: &Value, key: &str) -> Vec<String> {
    input[key]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| item.as_str().unwrap().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

fn constraint_json(constraint: &Constraint) -> Value {
    match constraint {
        Constraint::Semver(req) => json!({ "semver": req.to_string() }),
        Constraint::Latest => json!({ "latest": true }),
        Constraint::RefPin(name) => json!({ "ref-pin": name }),
    }
}

fn run(input: &Value) -> Value {
    match input["op"].as_str().unwrap() {
        "parse-constraint" => constraint_json(&parse_constraint(input["input"].as_str())),
        "parse-tag" => parse_tag(input["input"].as_str().unwrap()).map_or(
            Value::Null,
            |version| json!({ "version": version.to_string() }),
        ),
        "satisfies" => {
            let constraint = parse_constraint(input["constraint"].as_str());
            let version = Version::parse(input["version"].as_str().unwrap()).unwrap();
            json!({ "satisfied": satisfies(&constraint, &version) })
        }
        "satisfies-requirement" => {
            let version = Version::parse(input["version"].as_str().unwrap()).unwrap();
            match satisfies_requirement(input["requirement"].as_str().unwrap(), &version) {
                Ok(satisfied) => json!({ "satisfied": satisfied }),
                Err(_) => json!({ "error": "not a requirement" }),
            }
        }
        "select" => {
            let tags = strings(input, "tags");
            let versions: Vec<Version> = tags.iter().map(|tag| parse_tag(tag).unwrap()).collect();
            let reqs: Vec<(String, semver::VersionReq)> = strings(input, "constraints")
                .iter()
                .enumerate()
                .map(|(i, text)| match parse_constraint(Some(text)) {
                    Constraint::Semver(req) => (format!("requester-{i}"), req),
                    other => panic!("select takes semver constraints, got {other}"),
                })
                .collect();
            let constraints: Vec<(&str, &semver::VersionReq)> =
                reqs.iter().map(|(who, req)| (who.as_str(), req)).collect();
            let locked = input["locked"].as_str().map(|tag| parse_tag(tag).unwrap());
            let policy = match input["policy"]
                .as_str()
                .unwrap_or("prefer-lock-then-latest")
            {
                "latest-only" => SelectionPolicy::LatestOnly,
                "lock-only" => SelectionPolicy::LockOnly,
                _ => SelectionPolicy::PreferLockThenLatest,
            };
            let excluded: HashSet<Version> = strings(input, "excluded")
                .iter()
                .map(|tag| parse_tag(tag).unwrap())
                .collect();
            match select(&versions, &constraints, locked.as_ref(), policy, &excluded) {
                Ok(index) => json!({ "tag": tags[index] }),
                Err(error) => json!({ "error": error.to_string() }),
            }
        }
        op => panic!("unknown op {op}"),
    }
}

#[test]
fn version_fixtures_match() {
    for dir in common::cases("version") {
        let input: Value =
            serde_json::from_str(&fs::read_to_string(dir.join("input.json")).unwrap()).unwrap();
        common::assert_file(&dir.join("expected.json"), &common::pretty(&run(&input)));
    }
}
