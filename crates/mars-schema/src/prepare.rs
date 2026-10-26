//! One-pass package classification, normalization, selection, and strict reading.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::diagnostic::{PathSegment, SchemaDiagnostic, Severity};
use crate::dialect::Dialect;
use crate::discover::{SelectedDefinition, payload_paths};
use crate::dispatch::{AsPlugin, Dispatched, Layout, dispatch};
use crate::frontmatter::Frontmatter;
use crate::read::DefinitionRead;
use crate::tree::TreeSource;
use crate::upgrade::{DefinitionKind, UpgradeResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PrepareContext {
    Package,
    AuthoredLocal,
}

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
    Foreign {
        dialect: Dialect,
        evidence: DialectEvidence,
    },
    /// A Claude marketplace: it lists plugins and holds no definitions of its own.
    Marketplace,
    Empty,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DialectEvidence {
    pub source: DialectEvidenceSource,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DialectEvidenceSource {
    Declared,
    Predicted,
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
    pub reading: PreparedDefinitionRead,
    pub change: ChangeRecord,
}

/// Serializes as the bare [`DefinitionRead`]; `PreparedDefinition::kind` names the variant.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum PreparedDefinitionRead {
    Agent(DefinitionRead<crate::agent::AgentProfile>),
    Skill(DefinitionRead<crate::skill::SkillProfile>),
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
    /// Package-level diagnostics by source path: discovery and upgrade. A definition's
    /// own reading diagnostics live on its `reading` only; see [`Prepared::all_diagnostics`].
    pub diagnostics: BTreeMap<String, Vec<SchemaDiagnostic>>,
}

impl Prepared {
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
    let manifest_bytes = source.read("mars.toml");
    let mut diagnostics = std::mem::take(&mut dispatched.discovery.diagnostics);

    let mut manifest = manifest_bytes.as_deref().map(|bytes| {
        prepare_manifest(
            bytes,
            classification == Classification::MarsLegacy,
            &mut diagnostics,
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
                )
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    if classification == Classification::MarsLegacy {
        let changed_path = manifest
            .as_ref()
            .filter(|manifest| manifest.change != ChangeRecord::None)
            .and_then(|manifest| manifest.source_path.clone())
            .or_else(|| {
                definitions
                    .iter()
                    .find(|definition| definition.change != ChangeRecord::None)
                    .map(|definition| definition.source_path.clone())
            });
        if let Some(path) = changed_path {
            diagnostics
                .entry(path)
                .or_default()
                .push(SchemaDiagnostic::new(
                    "legacy-package",
                    Severity::Warning,
                    Vec::new(),
                    "package uses the pre-v1 format; apply the reported authored-file changes",
                ));
        }
    }

    let payload = if can_read_definitions {
        payload_paths(
            source,
            all,
            selected.iter().copied(),
            &dispatched.discovery.skipped_generated_dirs,
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
    let prepared = Prepared {
        classification,
        manifest: manifest.take(),
        definitions,
        payload,
        diagnostics,
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
) -> PreparedManifest {
    let invalid_utf8 = std::str::from_utf8(bytes).err().map(|error| {
        SchemaDiagnostic::new(
            "text.invalid-utf8",
            Severity::Error,
            Vec::new(),
            format!("mars.toml is not UTF-8: {error}"),
        )
    });
    let (normalized_bytes, change, upgrade_diagnostics) = match std::str::from_utf8(bytes) {
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
    diagnostics
        .entry("mars.toml".to_string())
        .or_default()
        .extend(upgrade_diagnostics);
    let reading = std::str::from_utf8(&normalized_bytes).map_or_else(
        |_| DefinitionRead {
            frontmatter: Vec::new(),
            fields: None,
            body: None,
            diagnostics: invalid_utf8.into_iter().collect(),
        },
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
) -> PreparedDefinition {
    let Some(bytes) = source.read(&selected.source_path) else {
        let diagnostic = SchemaDiagnostic::new(
            "definition.read-missing",
            Severity::Error,
            Vec::new(),
            "selected definition could not be read",
        );
        let reading = match selected.kind {
            DefinitionKind::Agent => PreparedDefinitionRead::Agent(empty_reading(Some(diagnostic))),
            DefinitionKind::Skill => PreparedDefinitionRead::Skill(empty_reading(Some(diagnostic))),
        };
        return PreparedDefinition {
            kind: selected.kind,
            slug: selected.slug.clone(),
            source_path: selected.source_path.clone(),
            output_path: selected.output_path.clone(),
            normalized_bytes: Vec::new(),
            reading,
            change: ChangeRecord::None,
        };
    };
    let invalid_utf8 = std::str::from_utf8(&bytes).err().map(|error| {
        SchemaDiagnostic::new(
            "text.invalid-utf8",
            Severity::Error,
            Vec::new(),
            format!("definition is not UTF-8: {error}"),
        )
    });
    let (normalized_bytes, change, upgrade_diagnostics) = match std::str::from_utf8(&bytes) {
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
            Classification::Foreign { dialect, .. } => {
                translate_foreign(text, *dialect, selected, all, selected_paths, &bytes)
            }
            _ => (bytes.clone(), ChangeRecord::None, Vec::new()),
        },
        Err(_) => (bytes.clone(), ChangeRecord::None, Vec::new()),
    };
    diagnostics
        .entry(selected.source_path.clone())
        .or_default()
        .extend(upgrade_diagnostics);

    let text = std::str::from_utf8(&normalized_bytes).ok();
    let reading = match (selected.kind, text) {
        (DefinitionKind::Agent, Some(text)) => {
            PreparedDefinitionRead::Agent(crate::agent::read(text))
        }
        (DefinitionKind::Skill, Some(text)) => {
            PreparedDefinitionRead::Skill(crate::skill::read(text))
        }
        (DefinitionKind::Agent, None) => PreparedDefinitionRead::Agent(empty_reading(invalid_utf8)),
        (DefinitionKind::Skill, None) => PreparedDefinitionRead::Skill(empty_reading(invalid_utf8)),
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

impl PreparedDefinitionRead {
    fn diagnostics(&self) -> &[SchemaDiagnostic] {
        match self {
            Self::Agent(reading) => &reading.diagnostics,
            Self::Skill(reading) => &reading.diagnostics,
        }
    }
}

fn empty_reading<T>(diagnostic: Option<SchemaDiagnostic>) -> DefinitionRead<T> {
    DefinitionRead {
        frontmatter: Vec::new(),
        fields: None,
        body: None,
        diagnostics: diagnostic.into_iter().collect(),
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
