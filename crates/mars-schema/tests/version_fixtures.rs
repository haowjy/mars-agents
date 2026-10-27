//! Published version-rule contract (`fixtures/version/<case>/{input,expected}.json`): constraint
//! text, tag names and requirement text in, the choice or the refusal (`{error: {code, message}}`)
//! out.

use std::{collections::HashSet, fs};

use mars_schema::version::{
    Chosen, Constraint, SelectionPolicy, choose, parse_constraint, parse_tag, satisfies,
    satisfies_requirement,
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

fn error_json(code: &str, error: &impl std::fmt::Display) -> Value {
    json!({ "error": { "code": code, "message": error.to_string() } })
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
                Err(error) => error_json(error.code(), &error),
            }
        }
        "select-version" => {
            let tags = strings(input, "tags");
            let parsed: Vec<(String, Constraint)> = input["constraints"]
                .as_array()
                .unwrap()
                .iter()
                .map(|entry| {
                    (
                        entry[0].as_str().unwrap().to_owned(),
                        parse_constraint(entry[1].as_str()),
                    )
                })
                .collect();
            let constraints: Vec<(&str, &Constraint)> =
                parsed.iter().map(|(who, c)| (who.as_str(), c)).collect();
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
            match choose(&tags, &constraints, locked.as_ref(), policy, &excluded) {
                Ok(Chosen::Pin(reference)) => json!({ "pin": reference }),
                Ok(Chosen::Tag(tag)) => json!({ "tag": tag }),
                Ok(Chosen::Untagged) => json!({ "untagged": true }),
                Err(error) => error_json(error.code(), &error),
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
