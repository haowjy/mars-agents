//! The `inspect` and `normalize` contract. Each case under `fixtures/normalize/` holds
//! `input/` (the repository), `request.json` (hint, source, selection, and `remote` trees to
//! hand `normalize`), `inspection.json`, `normalized.json` (without file bytes) and
//! `expected/<package id>/` (every manifest and definition file the packages hold).

use std::{collections::BTreeMap, fs};

use mars_schema::{
    inspect::{Hint, PluginHint, inspect},
    normalize::{NormalizeRequest, RemoteTree, Selection, SourceInfo, normalize},
    tree::{FileTree, TreeSource},
};
use serde::Deserialize;

mod common;

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Request {
    #[serde(default)]
    source: SourceInfo,
    #[serde(default)]
    hint: Hint,
    #[serde(default)]
    selection: Selection,
    /// Plugin name to a fetched tree under the case directory.
    #[serde(default)]
    remote: BTreeMap<String, Remote>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Remote {
    dir: String,
    #[serde(default)]
    source: SourceInfo,
}

#[test]
fn normalize_fixtures_match() {
    for case in common::cases("normalize") {
        let request: Request = match fs::read(case.join("request.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap(),
            Err(_) => Request::default(),
        };
        let tree = common::load_tree(&case.join("input"));
        let inspection = inspect(&tree, &request.hint);
        common::assert_file(&case.join("inspection.json"), &common::pretty(&inspection));

        let remote_trees: BTreeMap<String, FileTree> = request
            .remote
            .iter()
            .map(|(plugin, remote)| (plugin.clone(), common::load_tree(&case.join(&remote.dir))))
            .collect();
        if !remote_trees.is_empty() {
            // The staged flow: inspect each fetched tree as the plugin its entry names.
            let remote_inspections: BTreeMap<_, _> = remote_trees
                .iter()
                .map(|(plugin, remote)| {
                    let entry = inspection
                        .plugins
                        .iter()
                        .find(|candidate| &candidate.entry.name == plugin)
                        .unwrap_or_else(|| panic!("{}: no plugin {plugin}", case.display()))
                        .entry
                        .clone();
                    let hint = Hint {
                        plugin: Some(PluginHint {
                            entry,
                            at_marketplace_root: false,
                        }),
                        ..Hint::default()
                    };
                    (plugin.clone(), inspect(remote, &hint))
                })
                .collect();
            common::assert_file(
                &case.join("remote-inspections.json"),
                &common::pretty(&remote_inspections),
            );
        }

        let normalized = normalize(&NormalizeRequest {
            tree: &tree,
            source: request.source.clone(),
            hint: request.hint.clone(),
            selection: request.selection.clone(),
            remote_trees: remote_trees
                .iter()
                .map(|(plugin, remote)| {
                    (
                        plugin.clone(),
                        RemoteTree {
                            tree: remote,
                            source: request.remote[plugin].source.clone(),
                        },
                    )
                })
                .collect(),
        });
        common::assert_file(
            &case.join("normalized.json"),
            &common::pretty(&common::without_bytes(&normalized)),
        );

        let expected = case.join("expected");
        if std::env::var_os("MARS_UPDATE_FIXTURES").is_some() {
            let _ = fs::remove_dir_all(&expected);
        }
        let mut produced = BTreeMap::new();
        for package in &normalized.packages {
            let prepared = &package.prepared;
            let files = prepared
                .manifest
                .iter()
                .map(|manifest| (&manifest.output_path, &manifest.normalized_bytes))
                .chain(
                    prepared
                        .definitions
                        .iter()
                        .map(|definition| (&definition.output_path, &definition.normalized_bytes)),
                );
            for (path, bytes) in files {
                produced.insert(format!("{}/{path}", package.id), bytes.clone());
            }
        }
        if std::env::var_os("MARS_UPDATE_FIXTURES").is_some() {
            for (path, bytes) in &produced {
                let target = expected.join(path);
                fs::create_dir_all(target.parent().unwrap()).unwrap();
                fs::write(target, bytes).unwrap();
            }
        }
        let mut on_disk = Vec::new();
        if expected.exists() {
            common::collect_files(&expected, &expected, &mut on_disk);
        }
        assert_eq!(
            on_disk.iter().map(|(path, _)| path).collect::<Vec<_>>(),
            produced.keys().collect::<Vec<_>>(),
            "fixture {} file set",
            case.display()
        );
        for (path, bytes) in on_disk {
            assert_eq!(bytes, produced[&path], "fixture {} {path}", case.display());
        }
        // Payload is listed, never read: its bytes are not in the result.
        for package in &normalized.packages {
            for payload in &package.prepared.payload {
                assert!(tree.is_file(&payload.source_path) || !remote_trees.is_empty());
            }
        }
    }
}
