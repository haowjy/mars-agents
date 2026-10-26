//! `inspect`: what is in a repository, before anything is selected or converted.
//!
//! Pure over a file tree. Reads manifests, listings and the first lines of definitions, never
//! payload bytes. A marketplace lists its plugins; a plugin it points at in another
//! repository is fetched by the caller and inspected on its own, with the entry as the hint.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::claude::manifest::{MarketplaceEntry, PluginSource};
use crate::diagnostic::SchemaDiagnostic;
use crate::dialect::{Dialect, DialectChoice};
use crate::discover::{enclosing_generated_dir, payload_paths};
use crate::dispatch::{AsPlugin, Dispatched, Layout, dispatch};
pub use crate::dispatch::{Kind, Mirror};
use crate::item::DefinitionKind;
use crate::prepare::{Classification, PrepareContext};
use crate::tree::{SubTree, TreeSource};

/// What the caller declares about a root; everything is optional.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hint {
    /// Overrides prediction, never a Mars manifest.
    #[serde(default)]
    pub dialect: Option<Dialect>,
    /// The package root inside the tree.
    #[serde(default)]
    pub subpath: Option<String>,
    /// Read the root as this plugin of a marketplace (a fetched remote plugin tree).
    #[serde(default)]
    pub plugin: Option<PluginHint>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginHint {
    pub entry: MarketplaceEntry,
    /// Whether the entry's source is the marketplace root, which narrows the default skills.
    #[serde(default)]
    pub at_marketplace_root: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Inspection {
    pub kind: Kind,
    /// The dialect read, with how it was chosen and the files that decided it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dialect: Option<DialectChoice>,
    /// Other harnesses' copies of the package, found and not read.
    pub also_found: Vec<Mirror>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// A marketplace's plugins.
    pub plugins: Vec<InspectedPlugin>,
    /// The items of a plugin or skill tree.
    pub items: Vec<InspectedItem>,
    pub diagnostics: BTreeMap<String, Vec<SchemaDiagnostic>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InspectedPlugin {
    pub entry: MarketplaceEntry,
    pub at_marketplace_root: bool,
    /// The items of a plugin in the marketplace's own tree. A remote plugin has none until the
    /// caller fetches it and inspects the tree.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<Vec<InspectedItem>>,
    /// Why the plugin cannot be imported at all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
    pub diagnostics: BTreeMap<String, Vec<SchemaDiagnostic>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InspectedKind {
    Skill,
    Agent,
    /// Reported and not imported.
    Command,
}

impl From<DefinitionKind> for InspectedKind {
    fn from(kind: DefinitionKind) -> Self {
        match kind {
            DefinitionKind::Skill => Self::Skill,
            DefinitionKind::Agent => Self::Agent,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct InspectedItem {
    pub kind: InspectedKind,
    pub slug: String,
    /// The definition file (`…/SKILL.md`, `agents/x.md`, `commands/x.md`).
    pub source_path: String,
    /// The skill's directory, whose files travel with it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_root: Option<String>,
    /// Where the definition lands in schema v1; commands have none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_path: Option<String>,
    /// A skill's payload files, by source path.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub payload: Vec<String>,
}

pub fn inspect(source: &impl TreeSource, hint: &Hint) -> Inspection {
    match hint.subpath.as_deref().filter(|path| !path.is_empty()) {
        Some(subpath) => inspect_root(&SubTree::new(source, subpath), hint),
        None => inspect_root(source, hint),
    }
}

fn inspect_root(source: &impl TreeSource, hint: &Hint) -> Inspection {
    let dispatched = dispatch(
        source,
        PrepareContext::Package,
        hint.dialect,
        hint.plugin.as_ref().map(as_plugin),
        None,
    );
    describe(source, hint.dialect, dispatched)
}

fn as_plugin(hint: &PluginHint) -> AsPlugin<'_> {
    AsPlugin {
        entry: &hint.entry,
        at_marketplace_root: hint.at_marketplace_root,
    }
}

fn describe(
    source: &impl TreeSource,
    declared: Option<Dialect>,
    dispatched: Dispatched,
) -> Inspection {
    let mut diagnostics = dispatched.discovery.diagnostics.clone();
    let items = list_items(source, &dispatched, &mut diagnostics);
    let (name, version, description) = match &dispatched.layout {
        Layout::Claude(spec) => (
            spec.name.clone(),
            spec.version.clone(),
            spec.description.clone(),
        ),
        _ => (None, None, None),
    };
    let plugins = match &dispatched.layout {
        Layout::Marketplace(marketplace) => marketplace
            .plugins
            .iter()
            .map(|entry| inspect_plugin(source, declared, entry))
            .collect(),
        _ => Vec::new(),
    };
    Inspection {
        kind: dispatched.kind(),
        dialect: dispatched.dialect().cloned(),
        also_found: dispatched.also_found,
        name,
        version,
        description,
        plugins,
        items,
        diagnostics,
    }
}

/// A marketplace plugin: refused when its source cannot be imported, listed with its items
/// when it lives in the marketplace's own tree.
fn inspect_plugin(
    source: &impl TreeSource,
    declared: Option<Dialect>,
    entry: &MarketplaceEntry,
) -> InspectedPlugin {
    let mut plugin = InspectedPlugin {
        entry: entry.clone(),
        at_marketplace_root: false,
        items: None,
        refused: None,
        diagnostics: BTreeMap::new(),
    };
    match &entry.source {
        PluginSource::Unsupported { reason } => {
            plugin.refused = Some(format!("can't import from this source: {reason}"));
        }
        PluginSource::Remote { .. } => {}
        PluginSource::Local { path } => {
            plugin.at_marketplace_root = path.is_empty();
            let root = SubTree::new(source, path.clone());
            if let Some(generated) = enclosing_generated_dir(source, path) {
                plugin.refused = Some(format!(
                    "`{path}` is inside `{}`, plugin output Mars built, which is never input",
                    if generated.is_empty() {
                        "."
                    } else {
                        &generated
                    }
                ));
                return plugin;
            }
            if !path.is_empty() && !source.is_dir(path) {
                plugin.refused = Some(format!("`{path}` does not exist in the marketplace"));
                return plugin;
            }
            let dispatched = dispatch(
                &root,
                PrepareContext::Package,
                declared,
                Some(AsPlugin {
                    entry,
                    at_marketplace_root: plugin.at_marketplace_root,
                }),
                Some(&entry.name),
            );
            if dispatched.kind() == Kind::Invalid {
                plugin.refused = dispatched
                    .discovery
                    .diagnostics
                    .values()
                    .flatten()
                    .find(|diagnostic| diagnostic.is_error())
                    .map(|diagnostic| diagnostic.message.clone())
                    .or_else(|| Some("the plugin's manifest cannot be read".to_owned()));
            }
            plugin.items = Some(list_items(&root, &dispatched, &mut plugin.diagnostics));
            for (path, found) in dispatched.discovery.diagnostics {
                plugin.diagnostics.entry(path).or_default().extend(found);
            }
        }
    }
    plugin
}

pub(crate) fn list_items(
    source: &impl TreeSource,
    dispatched: &Dispatched,
    diagnostics: &mut BTreeMap<String, Vec<SchemaDiagnostic>>,
) -> Vec<InspectedItem> {
    let discovery = &dispatched.discovery;
    let mut payload = BTreeMap::<String, Vec<String>>::new();
    let foreign = matches!(dispatched.classification, Classification::Foreign(_));
    let by_owner = payload_paths(
        source,
        discovery,
        foreign,
        discovery.definitions.iter(),
        diagnostics,
    );
    for (path, _) in by_owner {
        // The owner is the deepest skill root containing the file.
        if let Some(owner) = discovery
            .definitions
            .iter()
            .filter_map(|definition| definition.skill_root.as_deref())
            .filter(|root| root.is_empty() || path.starts_with(&format!("{root}/")))
            .max_by_key(|root| root.len())
        {
            payload.entry(owner.to_owned()).or_default().push(path);
        }
    }
    let mut items = discovery
        .definitions
        .iter()
        .map(|definition| InspectedItem {
            kind: definition.kind.into(),
            slug: definition.slug.clone(),
            source_path: definition.source_path.clone(),
            skill_root: definition.skill_root.clone(),
            output_path: Some(definition.output_path.clone()),
            payload: definition
                .skill_root
                .as_ref()
                .and_then(|root| payload.remove(root))
                .unwrap_or_default(),
        })
        .collect::<Vec<_>>();
    items.extend(discovery.dropped.iter().map(|command| InspectedItem {
        kind: InspectedKind::Command,
        slug: command.slug.clone(),
        source_path: command.source_path.clone(),
        skill_root: None,
        output_path: None,
        payload: Vec::new(),
    }));
    items
}
