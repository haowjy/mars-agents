use std::fs;

use mars_schema::resolve::wire::{Input, run};
use serde_json::{Value, json};

mod common;

/// Fixtures hold a tree answer's file text, the WASM package takes bytes: both runners convert.
fn as_bytes(answer: &mut Value) {
    if let Some(files) = answer.get_mut("files").and_then(Value::as_object_mut) {
        for text in files.values_mut() {
            *text = json!(text.as_str().expect("file text").as_bytes());
        }
    }
}

/// Each case is a base input and the answers each call adds to the ones before it; `expected.json`
/// has the output of every call.
#[test]
fn resolve_fixtures_match() {
    for case in common::cases("resolve") {
        let steps: Value =
            serde_json::from_slice(&fs::read(case.join("steps.json")).unwrap()).unwrap();
        let mut answers: Vec<Value> = Vec::new();
        let mut outputs = Vec::new();
        for step in steps["steps"].as_array().unwrap() {
            for answer in step["answers"].as_array().unwrap() {
                let mut answer = answer.clone();
                as_bytes(&mut answer);
                answers.push(answer);
            }
            let mut input = steps["input"].clone();
            input["answers"] = Value::Array(answers.clone());
            let input: Input = serde_json::from_value(input).unwrap();
            outputs.push(run(input).unwrap());
        }
        common::assert_file(&case.join("expected.json"), &common::pretty(&outputs));
    }
}
