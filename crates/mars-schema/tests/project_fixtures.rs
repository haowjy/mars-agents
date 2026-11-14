//! The published tool-policy projection: `fixtures/project/<case>/{request.json, expected.json}`
//! pins what `project(rules, host)` reports for one rule list on one host.

use std::fs;

use mars_schema::Host;
use mars_schema::tool_policy::{EffectiveToolPolicy, ToolAction, ToolRule};
use mars_schema::tool_project::project;
use serde::Deserialize;

mod common;

#[derive(Deserialize)]
struct Request {
    host: String,
    rules: Vec<RuleInput>,
}

#[derive(Deserialize)]
struct RuleInput {
    capability: String,
    pattern: Option<String>,
    action: ToolAction,
}

#[test]
fn project_fixtures_match() {
    for case_dir in common::cases("project") {
        let request: Request =
            serde_json::from_slice(&fs::read(case_dir.join("request.json")).unwrap()).unwrap();
        let policy = EffectiveToolPolicy {
            rules: request
                .rules
                .iter()
                .map(|rule| {
                    ToolRule::from_wire(&rule.capability, rule.pattern.as_deref(), rule.action)
                        .unwrap()
                })
                .collect(),
        };
        let projection = project(&policy, Host::parse(&request.host).unwrap());
        common::assert_file(
            &case_dir.join("expected.json"),
            &common::pretty(&projection),
        );
    }
}
