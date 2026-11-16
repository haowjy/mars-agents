//! Effort resolution and alias selection, the pure parts of `mars models`:
//! `fixtures/effort/<case>/{request.json, expected.json}` pins `resolveEffort`,
//! `fixtures/alias/<case>/{request.json, expected.json}` pins `resolveAlias`,
//! `fixtures/alias-candidates/` `aliasCandidates`, `fixtures/alias-narrow/` `narrowRows`, `fixtures/alias-resolve-all/` `resolveAliases`, and
//! `fixtures/effort-levels/expected.json` the ladder `effortLevels` returns.

use std::fs;

use mars_schema::Host;
use mars_schema::agent::{EffortLevel, EffortRequest};
use mars_schema::alias::ModelAlias;
use mars_schema::effort::{ModelReasoning, harness_levels, resolve_effort};
use mars_schema::models::{CachedModel, ModelReasoningRow};
use serde::Deserialize;

mod common;

#[derive(Deserialize)]
struct EffortRequestFile {
    requested: Option<String>,
    model: Option<serde_json::Value>,
    host: String,
}

#[derive(Deserialize)]
struct AliasRow {
    id: String,
    provider: String,
    release_date: Option<String>,
}

fn models(rows: Vec<AliasRow>) -> Vec<CachedModel> {
    rows.into_iter()
        .map(|row| CachedModel {
            provider: row.provider.clone(),
            provider_key: row.provider,
            id: row.id,
            release_date: row.release_date,
            ..Default::default()
        })
        .collect()
}

fn request<T: serde::de::DeserializeOwned>(case_dir: &std::path::Path) -> T {
    serde_json::from_slice(&fs::read(case_dir.join("request.json")).unwrap()).unwrap()
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
    #[derive(Deserialize)]
    struct Request {
        rows: Vec<AliasRow>,
        options: ModelAlias,
    }
    for case_dir in common::cases("alias") {
        let Request { rows, options } = request(&case_dir);
        common::assert_file(
            &case_dir.join("expected.json"),
            &common::pretty(&options.resolve(&models(rows))),
        );
    }
}

#[test]
fn alias_candidates_fixtures_match() {
    #[derive(Deserialize)]
    struct Request {
        rows: Vec<AliasRow>,
        alias: ModelAlias,
    }
    for case_dir in common::cases("alias-candidates") {
        let Request { rows, alias } = request(&case_dir);
        common::assert_file(
            &case_dir.join("expected.json"),
            &common::pretty(&alias.candidates(&models(rows))),
        );
    }
}

#[test]
fn alias_narrow_fixtures_match() {
    #[derive(Deserialize)]
    struct Request {
        rows: Vec<serde_json::Value>,
        providers: Option<Vec<String>>,
    }
    for case_dir in common::cases("alias-narrow") {
        let Request { rows, providers } = request(&case_dir);
        let providers = providers.unwrap_or_else(|| {
            mars_schema::models::DEFAULT_CATALOG_PROVIDERS
                .iter()
                .map(|provider| (*provider).to_owned())
                .collect()
        });
        let allowed = mars_schema::models::provider_filter(&providers);
        let kept: Vec<_> = rows
            .iter()
            .filter(|row| {
                let key = row.get("provider_key").or_else(|| row.get("provider"));
                allowed(key.and_then(|key| key.as_str()).unwrap())
            })
            .collect();
        common::assert_file(&case_dir.join("expected.json"), &common::pretty(&kept));
    }
}

#[test]
fn alias_resolve_all_fixtures_match() {
    #[derive(Deserialize)]
    struct Request {
        rows: Vec<AliasRow>,
        manifest: Manifest,
    }
    #[derive(Deserialize)]
    struct Manifest {
        models: indexmap::IndexMap<String, ModelAlias>,
    }
    for case_dir in common::cases("alias-resolve-all") {
        let Request { rows, manifest } = request(&case_dir);
        let rows = models(rows);
        let resolved: indexmap::IndexMap<_, _> = manifest
            .models
            .iter()
            .map(|(name, alias)| (name, alias.resolve(&rows)))
            .collect();
        common::assert_file(&case_dir.join("expected.json"), &common::pretty(&resolved));
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
