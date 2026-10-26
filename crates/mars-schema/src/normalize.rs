//! `normalize`: turn a selection of a repository into schema v1 packages.
//!
//! Each package is the [`Prepared`] value that `prepare` returns for one root, with the root
//! read under Claude's rules (relocated, translated), a generated `mars.toml`, and provenance.
//! A Mars package is the identity case: its own manifest, its own paths.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::claude::manifest::{
    MARKETPLACE_MANIFEST, MarketplaceEntry, PLUGIN_MANIFEST, PluginSource,
};
use crate::diagnostic::{SchemaDiagnostic, Severity};
use crate::dialect::Dialect;
use crate::discover::enclosing_generated_dir;
use crate::dispatch::{AsPlugin, Layout};
use crate::inspect::{Hint, Kind};
use crate::prepare::{
    ChangeRecord, Classification, PrepareContext, Prepared, PreparedManifest, RootOptions, Rooted,
    prepare_root,
};
use crate::tree::{SubTree, TreeSource};

/// Where a tree came from; names and provenance are derived from it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceInfo {
    /// The repository's name, used when nothing in the package names it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subpath: Option<String>,
}

/// What the caller chose from an inspection. An absent choice takes everything found.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    /// Marketplace plugin names.
    #[serde(default)]
    pub plugins: Option<Vec<String>>,
    /// Definition source paths by plugin name; the key `""` is a root that is not a
    /// marketplace plugin.
    #[serde(default)]
    pub items: BTreeMap<String, Vec<String>>,
}

pub struct NormalizeRequest<'a, T: TreeSource> {
    pub tree: &'a T,
    pub source: SourceInfo,
    pub hint: Hint,
    pub selection: Selection,
}

#[derive(Debug, Clone, Serialize)]
pub struct Normalized {
    pub kind: Kind,
    pub packages: Vec<NormalizedPackage>,
    /// Marketplace-level findings and the reasons a package was left out, by path.
    pub diagnostics: BTreeMap<String, Vec<SchemaDiagnostic>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NormalizedPackage {
    /// The plugin name for a marketplace plugin, else the package name.
    pub id: String,
    pub provenance: Provenance,
    pub prepared: Prepared,
}

#[derive(Debug, Clone, Serialize)]
pub struct Provenance {
    /// The version of the rules that produced these files.
    pub translator: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dialect: Option<Dialect>,
    pub source: SourceInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub marketplace: Option<MarketplaceProvenance>,
    /// The definition source paths imported.
    pub selected_items: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MarketplaceProvenance {
    pub plugin: String,
    pub plugin_source: PluginSource,
}

pub fn normalize<T: TreeSource>(request: &NormalizeRequest<'_, T>) -> Normalized {
    match request
        .hint
        .subpath
        .as_deref()
        .filter(|path| !path.is_empty())
    {
        Some(subpath) => normalize_root(&SubTree::new(request.tree, subpath), request),
        None => normalize_root(request.tree, request),
    }
}

fn item_set(selection: &Selection, plugin: &str) -> Option<BTreeSet<String>> {
    selection
        .items
        .get(plugin)
        .map(|paths| paths.iter().cloned().collect())
}

/// Where a package sits in a marketplace, when it does.
struct Origin<'a> {
    entry: &'a MarketplaceEntry,
}

/// A repository that names plugins but is no plugin itself. A remote plugin source must not be
/// one: its plugins would be a second marketplace the dependency never chose from.
fn is_nested_marketplace(tree: &impl TreeSource) -> bool {
    tree.is_file(MARKETPLACE_MANIFEST) && !tree.is_file(PLUGIN_MANIFEST)
}

const NESTED_MARKETPLACE: &str = "is itself a marketplace, which cannot be imported as a plugin";

fn normalize_root<T: TreeSource>(
    root: &impl TreeSource,
    request: &NormalizeRequest<'_, T>,
) -> Normalized {
    let hint_plugin = request.hint.plugin.as_ref();
    if let Some(plugin) = hint_plugin
        && matches!(plugin.entry.source, PluginSource::Remote { .. })
        && is_nested_marketplace(root)
    {
        return Normalized {
            kind: Kind::Marketplace,
            packages: Vec::new(),
            diagnostics: BTreeMap::from([(
                MARKETPLACE_MANIFEST.to_owned(),
                vec![SchemaDiagnostic::new(
                    "marketplace.nested-marketplace",
                    Severity::Error,
                    Vec::new(),
                    format!("plugin `{}` {NESTED_MARKETPLACE}", plugin.entry.name),
                )],
            )]),
        };
    }
    let items = item_set(&request.selection, "");
    let rooted = prepare_root(
        root,
        PrepareContext::Package,
        request.hint.dialect,
        RootOptions {
            as_plugin: hint_plugin.map(|plugin| AsPlugin {
                entry: &plugin.entry,
                at_marketplace_root: plugin.at_marketplace_root,
            }),
            selection: items.as_ref(),
            fallback_name: request.source.name.as_deref(),
        },
    );
    let mut normalized = Normalized {
        kind: rooted.dispatched.kind(),
        packages: Vec::new(),
        diagnostics: BTreeMap::new(),
    };
    if let Layout::Marketplace(marketplace) = &rooted.dispatched.layout {
        normalized.diagnostics = rooted.prepared.diagnostics.clone();
        for entry in &marketplace.plugins {
            let named = request
                .selection
                .plugins
                .as_ref()
                .map(|names| names.contains(&entry.name));
            if named != Some(false) {
                normalize_plugin(root, request, entry, named == Some(true), &mut normalized);
            }
        }
        return normalized;
    }
    let origin = hint_plugin.map(|plugin| Origin {
        entry: &plugin.entry,
    });
    finish_package(rooted, request, origin, &mut normalized);
    normalized
}

fn normalize_plugin<T: TreeSource>(
    marketplace_root: &impl TreeSource,
    request: &NormalizeRequest<'_, T>,
    entry: &MarketplaceEntry,
    named: bool,
    normalized: &mut Normalized,
) {
    // A plugin the caller asked for by name must work; one taken along with all of them need not.
    let severity = if named {
        Severity::Error
    } else {
        Severity::Warning
    };
    let mut report = |severity, code: &str, message: String| {
        normalized
            .diagnostics
            .entry(MARKETPLACE_MANIFEST.to_owned())
            .or_default()
            .push(SchemaDiagnostic::new(
                code,
                severity,
                Vec::new(),
                format!("plugin `{}`: {message}", entry.name),
            ));
    };
    let items = item_set(&request.selection, &entry.name);
    let options = |at_marketplace_root| RootOptions {
        as_plugin: Some(AsPlugin {
            entry,
            at_marketplace_root,
        }),
        selection: items.as_ref(),
        fallback_name: Some(&entry.name),
    };
    let rooted = match &entry.source {
        PluginSource::Unsupported { reason } => {
            report(
                severity,
                "marketplace.plugin-unsupported",
                format!("can't import from this source: {reason}"),
            );
            return;
        }
        PluginSource::Local { path } => {
            if let Some(generated) = enclosing_generated_dir(marketplace_root, path) {
                report(
                    Severity::Warning,
                    "marketplace.plugin-generated",
                    format!(
                        "`{path}` is inside `{}`, plugin output Mars built, which is never input",
                        if generated.is_empty() {
                            "."
                        } else {
                            &generated
                        }
                    ),
                );
                return;
            }
            if !path.is_empty() && !marketplace_root.is_dir(path) {
                report(
                    Severity::Error,
                    "marketplace.plugin-missing",
                    format!("`{path}` does not exist in the marketplace"),
                );
                return;
            }
            let root = SubTree::new(marketplace_root, path.clone());
            prepare_root(
                &root,
                PrepareContext::Package,
                request.hint.dialect,
                options(path.is_empty()),
            )
        }
        // The caller fetches it and normalizes the fetched tree with `hint.plugin` set to the
        // entry: the one protocol for a plugin whose files are not in this tree.
        PluginSource::Remote { .. } => {
            report(
                severity,
                "marketplace.remote-not-fetched",
                "lives in another repository: fetch it and normalize that tree with `hint.plugin`"
                    .into(),
            );
            return;
        }
    };
    finish_package(rooted, request, Some(Origin { entry }), normalized);
}

/// Add the package. A package-level error (two skills with one slug, conflicting manifests,
/// an unreadable manifest) is reported on the package, which then holds nothing to write; a
/// definition that cannot be read does not do that, because it imports unavailable with its
/// error.
fn finish_package<T: TreeSource>(
    rooted: Rooted,
    request: &NormalizeRequest<'_, T>,
    origin: Option<Origin<'_>>,
    normalized: &mut Normalized,
) {
    let Rooted {
        mut prepared,
        dispatched,
    } = rooted;
    let refused = prepared
        .diagnostics
        .values()
        .flatten()
        .any(SchemaDiagnostic::is_error);
    if refused {
        prepared.definitions.clear();
        prepared.payload.clear();
    }

    let spec = match &dispatched.layout {
        Layout::Claude(spec) => Some(spec),
        _ => None,
    };
    if prepared.manifest.is_none()
        && !refused
        && matches!(prepared.classification, Classification::Foreign(_))
    {
        let name = spec
            .and_then(|spec| spec.name.clone())
            .or_else(|| origin.as_ref().map(|origin| origin.entry.name.clone()))
            .unwrap_or_else(|| repository_name(request));
        prepared.manifest = Some(generated_manifest(
            &name,
            spec.and_then(|spec| spec.version.as_deref()),
            spec.and_then(|spec| spec.description.as_deref()),
        ));
    }
    let manifest_name = prepared
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.reading.fields.as_ref())
        .map(|package| package.name.clone())
        .unwrap_or_else(|| repository_name(request));
    let selected_items = prepared
        .definitions
        .iter()
        .map(|definition| definition.source_path.clone())
        .collect();
    normalized.packages.push(NormalizedPackage {
        id: origin
            .as_ref()
            .map_or(manifest_name, |origin| origin.entry.name.clone()),
        provenance: Provenance {
            translator: crate::VERSION,
            dialect: dispatched.dialect().map(|choice| choice.dialect),
            source: request.source.clone(),
            marketplace: origin.as_ref().map(|origin| MarketplaceProvenance {
                plugin: origin.entry.name.clone(),
                plugin_source: origin.entry.source.clone(),
            }),
            selected_items,
        },
        prepared,
    });
}

/// The repository's name, plus `/<path>` for a subpath import.
fn repository_name<T: TreeSource>(request: &NormalizeRequest<'_, T>) -> String {
    let base = request.source.name.as_deref().unwrap_or("package");
    match request
        .source
        .subpath
        .as_deref()
        .or(request.hint.subpath.as_deref())
        .filter(|path| !path.is_empty())
    {
        Some(subpath) => format!("{base}/{subpath}"),
        None => base.to_owned(),
    }
}

/// The `mars.toml` a manifest-free package is stored with.
fn generated_manifest(
    name: &str,
    version: Option<&str>,
    description: Option<&str>,
) -> PreparedManifest {
    use toml_edit::{DocumentMut, Item, Table, value};
    let mut package = Table::new();
    package.insert("schema", value(1));
    package.insert("name", value(name));
    if let Some(version) = version {
        package.insert("version", value(version));
    }
    if let Some(description) = description {
        package.insert("description", value(description));
    }
    let mut document = DocumentMut::new();
    document.insert("package", Item::Table(package));
    let text = document.to_string();
    let mut reading = crate::manifest::read(&text);
    // A package with no declared version is not a defect of the generated file.
    reading
        .diagnostics
        .retain(|diagnostic| diagnostic.code != "manifest.version-missing");
    PreparedManifest {
        source_path: None,
        output_path: "mars.toml".to_owned(),
        normalized_bytes: text.into_bytes(),
        reading,
        change: ChangeRecord::Generated,
    }
}
