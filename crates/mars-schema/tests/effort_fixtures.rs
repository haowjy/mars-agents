//! Effort resolution and alias selection, the pure parts of `mars models`:
//! `fixtures/effort/<case>/{request.json, expected.json}` pins `resolveEffort`,
//! `fixtures/alias/<case>/{request.json, expected.json}` pins `resolveAlias`, and
//! `fixtures/effort-levels/expected.json` the ladder `effortLevels` returns.

use std::fs;

use mars_schema::Host;
use mars_schema::agent::{EffortLevel, EffortRequest};
use mars_schema::effort::{ModelReasoning, harness_levels, resolve_effort};
use mars_schema::models::{CachedModel, ModelReasoningRow, auto_resolve_all};
use serde::Deserialize;

mod common;

#[derive(Deserialize)]
struct EffortRequestFile {
    requested: Option<String>,
    model: Option<serde_json::Value>,
    host: String,
}

#[derive(Deserialize)]
struct AliasRequestFile {
    rows: Vec<AliasRow>,
    options: AliasOptions,
}

#[derive(Deserialize)]
struct AliasRow {
    id: String,
    provider: String,
    release_date: Option<String>,
}

#[derive(Deserialize)]
struct AliasOptions {
    #[serde(rename = "match")]
    match_patterns: Vec<String>,
    #[serde(default)]
    exclude: Vec<String>,
    provider: Option<String>,
}

#[test]
fn effort_fixtures_match() {
    for case_dir in common::cases("effort") {
        let request: EffortRequestFile =
            serde_json::from_slice(&fs::read(case_dir.join("request.json")).unwrap()).unwrap();
        let requested = request
            .requested
            .as_deref()
            .map_or(Some(EffortRequest::Default), EffortRequest::parse)
            .unwrap();
        let row = request
            .model
            .as_ref()
            .and_then(ModelReasoningRow::from_models_dev);
        let host = Host::parse(&request.host).unwrap();
        let resolution = resolve_effort(
            requested,
            &ModelReasoning::from_row(row.as_ref()),
            &harness_levels(host),
        );
        common::assert_file(
            &case_dir.join("expected.json"),
            &common::pretty(&resolution),
        );
    }
}

#[test]
fn alias_fixtures_match() {
    for case_dir in common::cases("alias") {
        let request: AliasRequestFile =
            serde_json::from_slice(&fs::read(case_dir.join("request.json")).unwrap()).unwrap();
        let models: Vec<CachedModel> = request
            .rows
            .into_iter()
            .map(|row| CachedModel {
                provider: row.provider.clone(),
                provider_key: row.provider,
                id: row.id,
                release_date: row.release_date,
                ..Default::default()
            })
            .collect();
        let winner = auto_resolve_all(
            request.options.provider.as_deref(),
            &request.options.match_patterns,
            &request.options.exclude,
            &models,
        )
        .into_iter()
        .next()
        .map(|model| model.id.as_str());
        common::assert_file(&case_dir.join("expected.json"), &common::pretty(&winner));
    }
}

#[test]
fn effort_levels_fixture_matches() {
    let levels: Vec<&str> = EffortLevel::LADDER
        .iter()
        .map(|level| level.as_str())
        .collect();
    common::assert_file(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/effort-levels/expected.json"),
        &common::pretty(&levels),
    );
}
