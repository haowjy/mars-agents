//! One-pass package classification, normalization, selection, and strict reading.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::diagnostic::{PathSegment, SchemaDiagnostic, Severity};
use crate::dialect::{Dialect, DialectChoice};
use crate::discover::{SelectedDefinition, payload_paths};
use crate::dispatch::{AsPlugin, Dispatched, Layout, dispatch};
use crate::frontmatter::Frontmatter;
use crate::read::{DefinitionRead, Reading, read_definition};
use crate::tree::TreeSource;
use crate::upgrade::{DefinitionKind, UpgradeResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PrepareContext {
    /// A package root (a dependency, or the package being published): its `mars.toml` must
    /// declare `[package]`, and a root with no manifest is read as the dialect it looks like.
    Package,
    /// A consumer's own authored directory (`.mars-src`): content without a manifest is Mars
    /// convention, and a `mars.toml` without `[package]` is consumer configuration, not read.
    AuthoredLocal,
    /// A directory that is either: `Package` unless its `mars.toml` has no `[package]`, which
    /// makes it a consumer project whose tree is read as `AuthoredLocal` content.
    Project,
}

/// What a package root is, once its manifest, declared dialect and contents have been weighed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Classification {
    #[serde(rename = "schema-1")]
    Schema1,
    MarsLegacy,
    SchemaNewer {
        schema: u64,
    },
    Invalid,
    /// Another harness's layout, read as the dialect chosen.
    Foreign(DialectChoice),
    /// A Claude marketplace: it lists plugins and holds no definitions of its own.
    Marketplace,
    Empty,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ChangeRecord {
    None,
    /// The file has no source: `normalize` wrote it (a package's generated `mars.toml`).
    Generated,
    Edited {
        paths: Vec<Vec<PathSegment>>,
    },
    FullyRewritten,
}

#[derive(Debug, Clone, Serialize)]
pub struct PreparedManifest {
    /// `None` for a generated manifest.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    pub output_path: String,
    /// The normalized file; crosses to JavaScript as a `Uint8Array`.
    #[serde(with = "serde_bytes")]
    pub normalized_bytes: Vec<u8>,
    pub reading: DefinitionRead<crate::package::PackageMetadata>,
    pub change: ChangeRecord,
}

#[derive(Debug, Clone, Serialize)]
pub struct PreparedDefinition {
    pub kind: DefinitionKind,
    pub slug: String,
    pub source_path: String,
    pub output_path: String,
    /// The normalized file; crosses to JavaScript as a `Uint8Array`.
    #[serde(with = "serde_bytes")]
    pub normalized_bytes: Vec<u8>,
    /// The Agent or skill reading `kind` names.
    pub reading: Reading,
    pub change: ChangeRecord,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PayloadEntry {
    pub source_path: String,
    pub output_path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Prepared {
    pub classification: Classification,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest: Option<PreparedManifest>,
    pub definitions: Vec<PreparedDefinition>,
    pub payload: Vec<PayloadEntry>,
    /// Package-level diagnostics by source path: discovery and foreign-dialect translation. A
    /// definition's own reading diagnostics live on its `reading` only; see
    /// [`Prepared::all_diagnostics`].
    pub diagnostics: BTreeMap<String, Vec<SchemaDiagnostic>>,
    /// What upgrading a pre-v1 package changed or dropped, by source path (legacy keys, skill
    /// tool rules). `mars migrate` lists them; a caller that only reads the package reports
    /// [`Prepared::needs_migration`] instead.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub upgrade_diagnostics: BTreeMap<String, Vec<SchemaDiagnostic>>,
}

impl Prepared {
    /// A pre-v1 package whose authored files preparation changed: the changes are in
    /// the prepared bytes, and `mars migrate` writes them back.
    pub fn needs_migration(&self) -> bool {
        self.classification == Classification::MarsLegacy
            && (self
                .manifest
                .iter()
                .any(|manifest| manifest.change != ChangeRecord::None)
                || self
                    .definitions
                    .iter()
                    .any(|definition| definition.change != ChangeRecord::None))
    }

    /// Every diagnostic by source path: package-level first, then the file's reading.
    pub fn all_diagnostics(&self) -> BTreeMap<&str, Vec<&SchemaDiagnostic>> {
        let mut all: BTreeMap<&str, Vec<&SchemaDiagnostic>> = BTreeMap::new();
        for (path, diagnostics) in &self.diagnostics {
            all.entry(path).or_default().extend(diagnostics);
        }
        if let Some(manifest) = &self.manifest
            && let Some(path) = &manifest.source_path
        {
            all.entry(path)
                .or_default()
                .extend(&manifest.reading.diagnostics);
        }
        for definition in &self.definitions {
            all.entry(&definition.source_path)
                .or_default()
                .extend(definition.reading.diagnostics());
        }
        all.retain(|_, diagnostics| !diagnostics.is_empty());
        all
    }
}

/// What a caller adds when it reads a root as one plugin of a marketplace, or imports only some
/// of the items.
#[derive(Default, Clone, Copy)]
pub(crate) struct RootOptions<'a> {
    pub as_plugin: Option<AsPlugin<'a>>,
    /// Source paths of the definitions to import; `None` imports every one found.
    pub selection: Option<&'a BTreeSet<String>>,
    /// Names a root `SKILL.md` when its own `name` cannot.
    pub fallback_name: Option<&'a str>,
}

/// `prepare` plus what was discovered, for `inspect` and `normalize`.
pub(crate) struct Rooted {
    pub prepared: Prepared,
    pub dispatched: Dispatched,
}

/// Prepare one rooted package without reading payload bytes.
pub fn prepare(
    source: &impl TreeSource,
    context: PrepareContext,
    declared_dialect: Option<Dialect>,
) -> Prepared {
    prepare_root(source, context, declared_dialect, RootOptions::default()).prepared
}

pub(crate) fn prepare_root(
    source: &impl TreeSource,
    context: PrepareContext,
    declared_dialect: Option<Dialect>,
    options: RootOptions<'_>,
) -> Rooted {
    let mut dispatched = dispatch(
        source,
        context,
        declared_dialect,
        options.as_plugin,
        options.fallback_name,
    );
    let classification = dispatched.classification.clone();
    let manifest_bytes = dispatched.manifest.take();
    let mut diagnostics = std::mem::take(&mut dispatched.discovery.diagnostics);
    let mut upgrade_diagnostics = BTreeMap::new();

    let mut manifest = manifest_bytes.as_deref().map(|bytes| {
        prepare_manifest(
            bytes,
            classification == Classification::MarsLegacy,
            &mut diagnostics,
            &mut upgrade_diagnostics,
        )
    });

    let can_read_definitions = !matches!(
        classification,
        Classification::Invalid
            | Classification::SchemaNewer { .. }
            | Classification::Empty
            | Classification::Marketplace
    );
    let all = &dispatched.discovery.definitions;
    let selected = all
        .iter()
        .filter(|definition| {
            options
                .selection
                .is_none_or(|selection| selection.contains(&definition.source_path))
        })
        .collect::<Vec<_>>();
    let selected_paths = selected
        .iter()
        .map(|definition| definition.source_path.as_str())
        .collect::<BTreeSet<_>>();
    let definitions = if can_read_definitions {
        selected
            .iter()
            .map(|definition| {
                prepare_definition(
                    source,
                    definition,
                    &classification,
                    all,
                    &selected_paths,
                    &mut diagnostics,
                    &mut upgrade_diagnostics,
                )
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    let payload = if can_read_definitions {
        payload_paths(
            source,
            &dispatched.discovery,
            selected.iter().copied(),
            &mut diagnostics,
        )
        .into_iter()
        .map(|(source_path, output_path)| PayloadEntry {
            source_path,
            output_path,
        })
        .collect()
    } else {
        Vec::new()
    };

    if can_read_definitions {
        report_unconverted(source, &dispatched, &mut diagnostics);
    }
    diagnostics.retain(|_, diagnostics| !diagnostics.is_empty());
    upgrade_diagnostics.retain(|_, diagnostics| !diagnostics.is_empty());
    let prepared = Prepared {
        classification,
        manifest: manifest.take(),
        definitions,
        payload,
        diagnostics,
        upgrade_diagnostics,
    };
    Rooted {
        prepared,
        dispatched,
    }
}

/// Commands are dropped, and hooks and MCP servers are not converted yet.
fn report_unconverted(
    source: &impl TreeSource,
    dispatched: &Dispatched,
    diagnostics: &mut BTreeMap<String, Vec<SchemaDiagnostic>>,
) {
    let Layout::Claude(spec) = &dispatched.layout else {
        return;
    };
    let mut warn = |path: &str, code: &str, message: String| {
        diagnostics
            .entry(path.to_owned())
            .or_default()
            .push(SchemaDiagnostic::new(
                code,
                Severity::Warning,
                Vec::new(),
                message,
            ));
    };
    if !dispatched.discovery.dropped.is_empty() {
        warn(
            "commands",
            "import.commands-dropped",
            format!(
                "commands are not imported (convert one to a skill): {}",
                dispatched
                    .discovery
                    .dropped
                    .iter()
                    .map(|command| command.source_path.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        );
    }
    if source.is_file("hooks/hooks.json") || spec.hooks {
        let path = if source.is_file("hooks/hooks.json") {
            "hooks/hooks.json"
        } else {
            crate::claude::manifest::PLUGIN_MANIFEST
        };
        warn(
            path,
            "import.hooks-not-converted",
            "hooks are not converted yet".into(),
        );
    }
    if source.is_file(".mcp.json") || spec.mcp {
        let path = if source.is_file(".mcp.json") {
            ".mcp.json"
        } else {
            crate::claude::manifest::PLUGIN_MANIFEST
        };
        warn(
            path,
            "import.mcp-not-converted",
            "MCP servers are not converted yet".into(),
        );
    }
}

fn prepare_manifest(
    bytes: &[u8],
    upgrade: bool,
    diagnostics: &mut BTreeMap<String, Vec<SchemaDiagnostic>>,
    upgrade_diagnostics: &mut BTreeMap<String, Vec<SchemaDiagnostic>>,
) -> PreparedManifest {
    let invalid_utf8 = std::str::from_utf8(bytes).err().map(|error| {
        SchemaDiagnostic::new(
            "text.invalid-utf8",
            Severity::Error,
            Vec::new(),
            format!("mars.toml is not UTF-8: {error}"),
        )
    });
    let (normalized_bytes, change, upgraded) = match std::str::from_utf8(bytes) {
        Ok(text) if upgrade => match crate::upgrade::upgrade_manifest(text) {
            Ok(result) => upgrade_result(text, result),
            Err(error) => {
                push(
                    diagnostics,
                    "mars.toml",
                    SchemaDiagnostic::new(
                        "manifest.upgrade-failed",
                        Severity::Error,
                        Vec::new(),
                        error.to_string(),
                    ),
                );
                (bytes.to_vec(), ChangeRecord::None, Vec::new())
            }
        },
        Ok(_) => (bytes.to_vec(), ChangeRecord::None, Vec::new()),
        Err(_) => (bytes.to_vec(), ChangeRecord::None, Vec::new()),
    };
    upgrade_diagnostics
        .entry("mars.toml".to_string())
        .or_default()
        .extend(upgraded);
    let reading = std::str::from_utf8(&normalized_bytes).map_or_else(
        |_| DefinitionRead::unread(invalid_utf8.into_iter().collect()),
        crate::manifest::read,
    );
    PreparedManifest {
        source_path: Some("mars.toml".to_string()),
        output_path: "mars.toml".to_string(),
        normalized_bytes,
        reading,
        change,
    }
}

fn prepare_definition(
    source: &impl TreeSource,
    selected: &SelectedDefinition,
    classification: &Classification,
    all: &[SelectedDefinition],
    selected_paths: &BTreeSet<&str>,
    diagnostics: &mut BTreeMap<String, Vec<SchemaDiagnostic>>,
    upgrade_diagnostics: &mut BTreeMap<String, Vec<SchemaDiagnostic>>,
) -> PreparedDefinition {
    let Some(bytes) = source.read(&selected.source_path) else {
        return PreparedDefinition {
            kind: selected.kind,
            slug: selected.slug.clone(),
            source_path: selected.source_path.clone(),
            output_path: selected.output_path.clone(),
            normalized_bytes: Vec::new(),
            reading: unreadable(
                selected.kind,
                SchemaDiagnostic::new(
                    "definition.read-missing",
                    Severity::Error,
                    Vec::new(),
                    "selected definition could not be read",
                ),
            ),
            change: ChangeRecord::None,
        };
    };
    let (normalized_bytes, change, found) = match std::str::from_utf8(&bytes) {
        Ok(text) => match classification {
            Classification::MarsLegacy => {
                match crate::upgrade::upgrade_definition(selected.kind, text) {
                    Ok(result) => upgrade_result(text, result),
                    Err(error) => {
                        push(
                            diagnostics,
                            &selected.source_path,
                            SchemaDiagnostic::new(
                                "definition.upgrade-failed",
                                Severity::Error,
                                Vec::new(),
                                error.to_string(),
                            ),
                        );
                        (bytes.clone(), ChangeRecord::None, Vec::new())
                    }
                }
            }
            Classification::Foreign(choice) => {
                translate_foreign(text, choice.dialect, selected, all, selected_paths, &bytes)
            }
            _ => (bytes.clone(), ChangeRecord::None, Vec::new()),
        },
        Err(_) => (bytes.clone(), ChangeRecord::None, Vec::new()),
    };
    // What the pre-v1 upgrade did is for `mars migrate`; what translating a foreign file did
    // is a finding about the package.
    let into = if *classification == Classification::MarsLegacy {
        upgrade_diagnostics
    } else {
        diagnostics
    };
    into.entry(selected.source_path.clone())
        .or_default()
        .extend(found);

    // Only text is upgraded or translated, so these bytes are the file's when they are not text.
    let reading = match std::str::from_utf8(&normalized_bytes) {
        Ok(text) => read_definition(selected.kind, text),
        Err(error) => unreadable(
            selected.kind,
            SchemaDiagnostic::new(
                "text.invalid-utf8",
                Severity::Error,
                Vec::new(),
                format!("definition is not UTF-8: {error}"),
            ),
        ),
    };

    PreparedDefinition {
        kind: selected.kind,
        slug: selected.slug.clone(),
        source_path: selected.source_path.clone(),
        output_path: selected.output_path.clone(),
        normalized_bytes,
        reading,
        change,
    }
}

/// Lift a foreign definition's frontmatter into schema v1. Frontmatter that cannot be read
/// stays as written, with the reader's error; only a Claude skill's unquoted `description`
/// colon is repaired first.
fn translate_foreign(
    text: &str,
    dialect: Dialect,
    selected: &SelectedDefinition,
    all: &[SelectedDefinition],
    selected_paths: &BTreeSet<&str>,
    bytes: &[u8],
) -> (Vec<u8>, ChangeRecord, Vec<SchemaDiagnostic>) {
    let claude = matches!(dialect, Dialect::Claude | Dialect::AgentSkills);
    let mut diagnostics = Vec::new();
    let mut repaired = false;
    let frontmatter = match Frontmatter::parse(text) {
        Ok(frontmatter) => frontmatter,
        Err(_) if claude && selected.kind == DefinitionKind::Skill => {
            match crate::claude::translate::repair_description(text)
                .and_then(|fixed| Frontmatter::parse(&fixed).ok())
            {
                Some(frontmatter) => {
                    repaired = true;
                    diagnostics.push(crate::claude::translate::repaired_diagnostic());
                    frontmatter
                }
                None => return (bytes.to_vec(), ChangeRecord::None, Vec::new()),
            }
        }
        Err(_) => return (bytes.to_vec(), ChangeRecord::None, Vec::new()),
    };
    let item_kind = match selected.kind {
        DefinitionKind::Agent => crate::item::ItemKind::Agent,
        DefinitionKind::Skill => crate::item::ItemKind::Skill,
    };
    let lifted = crate::lift::lift_frontmatter(dialect, item_kind, &frontmatter);
    diagnostics.extend(lifted.diagnostics);
    if claude && selected.kind == DefinitionKind::Skill {
        diagnostics.extend(crate::claude::checks::skill_checks(
            selected,
            &lifted.frontmatter,
            all,
            selected_paths,
        ));
    }
    if lifted.changed || repaired {
        (
            lifted.frontmatter.render().into_bytes(),
            ChangeRecord::FullyRewritten,
            diagnostics,
        )
    } else {
        (bytes.to_vec(), ChangeRecord::None, diagnostics)
    }
}

/// A definition with no fields to read, only why.
fn unreadable(kind: DefinitionKind, diagnostic: SchemaDiagnostic) -> Reading {
    match kind {
        DefinitionKind::Agent => Reading::Agent(DefinitionRead::unread(vec![diagnostic])),
        DefinitionKind::Skill => Reading::Skill(DefinitionRead::unread(vec![diagnostic])),
    }
}

fn upgrade_result(
    original: &str,
    result: UpgradeResult,
) -> (Vec<u8>, ChangeRecord, Vec<SchemaDiagnostic>) {
    let change = if !result.changed_from(original) {
        ChangeRecord::None
    } else if result.fully_rewritten {
        ChangeRecord::FullyRewritten
    } else {
        ChangeRecord::Edited {
            paths: result.edited_paths,
        }
    };
    (result.text.into_bytes(), change, result.diagnostics)
}

fn push(
    diagnostics: &mut BTreeMap<String, Vec<SchemaDiagnostic>>,
    path: &str,
    diagnostic: SchemaDiagnostic,
) {
    diagnostics
        .entry(path.to_owned())
        .or_default()
        .push(diagnostic);
}
