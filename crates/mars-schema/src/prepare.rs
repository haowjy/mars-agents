//! One-pass package classification, normalization, selection, and strict reading.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_yaml::Value;

use crate::diagnostic::{PathSegment, SchemaDiagnostic, Severity};
use crate::dialect::Dialect;
use crate::discover::{SelectedDefinition, discover, payload_paths};
use crate::frontmatter::Frontmatter;
use crate::package::{ManifestClassification, classify_manifest};
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
    Edited { paths: Vec<Vec<PathSegment>> },
    FullyRewritten,
}

#[derive(Debug, Clone, Serialize)]
pub struct PreparedManifest {
    pub source_path: String,
    pub output_path: String,
    #[serde(skip)]
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
    #[serde(skip)]
    pub normalized_bytes: Vec<u8>,
    pub reading: PreparedDefinitionRead,
    pub raw_frontmatter: Vec<(String, Value)>,
    pub change: ChangeRecord,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", content = "result", rename_all = "lowercase")]
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
    pub diagnostics: BTreeMap<String, Vec<SchemaDiagnostic>>,
}

/// Prepare one rooted package without reading payload bytes.
pub fn prepare(
    source: &impl TreeSource,
    context: PrepareContext,
    declared_dialect: Option<Dialect>,
) -> Prepared {
    if source
        .read(crate::plugin_metadata::SENTINEL_FILE)
        .is_some_and(|bytes| crate::plugin_metadata::is_valid_sentinel(&bytes))
    {
        let discovery = discover(source);
        return Prepared {
            classification: Classification::Empty,
            manifest: None,
            definitions: Vec::new(),
            payload: Vec::new(),
            diagnostics: discovery.diagnostics,
        };
    }
    let manifest_bytes = source.read("mars.toml");
    let manifest_classification = classify_manifest(manifest_bytes.as_deref());
    let mut discovery = if matches!(
        &manifest_classification,
        ManifestClassification::Invalid { .. } | ManifestClassification::SchemaNewer { .. }
    ) {
        crate::discover::Discovery::empty()
    } else {
        discover(source)
    };
    let classification = match manifest_classification {
        ManifestClassification::Schema1 => Classification::Schema1,
        ManifestClassification::MarsLegacy => Classification::MarsLegacy,
        ManifestClassification::SchemaNewer { schema } => Classification::SchemaNewer { schema },
        ManifestClassification::Invalid { .. } => Classification::Invalid,
        ManifestClassification::NoManifest if context == PrepareContext::AuthoredLocal => {
            Classification::MarsLegacy
        }
        ManifestClassification::NoManifest => {
            let (dialect, evidence) = resolve_dialect(source, declared_dialect);
            let has_plugin_manifest = [
                ".claude-plugin/plugin.json",
                ".claude-plugin/marketplace.json",
            ]
            .into_iter()
            .any(|path| source.is_file(path));
            if discovery.definitions.is_empty() && evidence.paths.is_empty() && !has_plugin_manifest
            {
                Classification::Empty
            } else {
                Classification::Foreign { dialect, evidence }
            }
        }
    };

    let mut manifest = manifest_bytes.as_deref().map(|bytes| {
        prepare_manifest(
            bytes,
            classification == Classification::MarsLegacy,
            &mut discovery.diagnostics,
        )
    });

    let can_read_definitions = !matches!(
        classification,
        Classification::Invalid | Classification::SchemaNewer { .. } | Classification::Empty
    );
    let mut definitions = if can_read_definitions {
        discovery
            .definitions
            .iter()
            .map(|definition| {
                prepare_definition(
                    source,
                    definition,
                    &classification,
                    &mut discovery.diagnostics,
                )
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    for definition in &mut definitions {
        if definition.source_path == "SKILL.md"
            && definition.slug == "skill"
            && let PreparedDefinitionRead::Skill(reading) = &definition.reading
            && let Some(name) = reading
                .fields
                .as_ref()
                .and_then(|fields| fields.name.as_deref())
                .filter(|name| !name.is_empty())
        {
            definition.slug = name.to_owned();
        }
    }

    if classification == Classification::MarsLegacy {
        let changed_path = manifest
            .as_ref()
            .filter(|manifest| manifest.change != ChangeRecord::None)
            .map(|manifest| manifest.source_path.clone())
            .or_else(|| {
                definitions
                    .iter()
                    .find(|definition| definition.change != ChangeRecord::None)
                    .map(|definition| definition.source_path.clone())
            });
        if let Some(path) = changed_path {
            discovery
                .diagnostics
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
            &discovery.definitions,
            &discovery.skipped_generated_dirs,
        )
        .into_iter()
        .map(|path| PayloadEntry {
            source_path: path.clone(),
            output_path: path,
        })
        .collect()
    } else {
        Vec::new()
    };

    discovery
        .diagnostics
        .retain(|_, diagnostics| !diagnostics.is_empty());
    Prepared {
        classification,
        manifest: manifest.take(),
        definitions,
        payload,
        diagnostics: discovery.diagnostics,
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
            unknown: Vec::new(),
            diagnostics: invalid_utf8.into_iter().collect(),
        },
        crate::manifest::read,
    );
    diagnostics
        .entry("mars.toml".to_string())
        .or_default()
        .extend(reading.diagnostics.iter().cloned());
    PreparedManifest {
        source_path: "mars.toml".to_string(),
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
    diagnostics: &mut BTreeMap<String, Vec<SchemaDiagnostic>>,
) -> PreparedDefinition {
    let Some(bytes) = source.read(&selected.source_path) else {
        let diagnostic = SchemaDiagnostic::new(
            "definition.read-missing",
            Severity::Error,
            Vec::new(),
            "selected definition could not be read",
        );
        push(diagnostics, &selected.source_path, diagnostic.clone());
        let reading = match selected.kind {
            DefinitionKind::Agent => PreparedDefinitionRead::Agent(empty_reading(Some(diagnostic))),
            DefinitionKind::Skill => PreparedDefinitionRead::Skill(empty_reading(Some(diagnostic))),
        };
        return PreparedDefinition {
            kind: selected.kind,
            slug: selected.slug.clone(),
            source_path: selected.source_path.clone(),
            output_path: selected.source_path.clone(),
            normalized_bytes: Vec::new(),
            reading,
            raw_frontmatter: Vec::new(),
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
            Classification::Foreign { dialect, .. } => match Frontmatter::parse(text) {
                Ok(frontmatter) => {
                    let item_kind = match selected.kind {
                        DefinitionKind::Agent => crate::item::ItemKind::Agent,
                        DefinitionKind::Skill => crate::item::ItemKind::Skill,
                    };
                    let (lifted, changed) = crate::lift::lift_frontmatter_with_change(
                        *dialect,
                        item_kind,
                        &frontmatter,
                    );
                    if changed {
                        (
                            lifted.render().into_bytes(),
                            ChangeRecord::FullyRewritten,
                            Vec::new(),
                        )
                    } else {
                        (bytes.clone(), ChangeRecord::None, Vec::new())
                    }
                }
                Err(_) => (bytes.clone(), ChangeRecord::None, Vec::new()),
            },
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
    diagnostics
        .entry(selected.source_path.clone())
        .or_default()
        .extend(reading.diagnostics().iter().cloned());
    let raw_frontmatter = text
        .and_then(|text| raw_frontmatter(selected.kind, text))
        .map(|frontmatter| {
            frontmatter
                .keys()
                .into_iter()
                .filter_map(|key| frontmatter.get(&key).cloned().map(|value| (key, value)))
                .collect()
        })
        .unwrap_or_default();

    PreparedDefinition {
        kind: selected.kind,
        slug: selected.slug.clone(),
        source_path: selected.source_path.clone(),
        output_path: selected.source_path.clone(),
        normalized_bytes,
        reading,
        raw_frontmatter,
        change,
    }
}

fn raw_frontmatter(kind: DefinitionKind, text: &str) -> Option<Frontmatter> {
    match kind {
        DefinitionKind::Agent => Frontmatter::parse_preserving_structure(text)
            .ok()
            .map(|(frontmatter, _)| frontmatter),
        DefinitionKind::Skill => Frontmatter::parse(text).ok(),
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
        unknown: Vec::new(),
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

fn resolve_dialect(
    source: &impl TreeSource,
    declared: Option<Dialect>,
) -> (Dialect, DialectEvidence) {
    if let Some(dialect) = declared {
        return (
            dialect,
            DialectEvidence {
                source: DialectEvidenceSource::Declared,
                paths: Vec::new(),
            },
        );
    }
    let mut matches = Vec::new();
    for (container, dialect) in [
        (".claude", Dialect::Claude),
        (".codex", Dialect::Codex),
        (".opencode", Dialect::OpenCode),
        (".cursor", Dialect::Cursor),
    ] {
        for kind in ["skills", "agents"] {
            let path = format!("{container}/{kind}");
            if source.is_dir(&path) && !source.children(&path).is_empty() {
                matches.push((dialect, path));
                break;
            }
        }
    }
    let dialect = if matches
        .iter()
        .map(|(dialect, _)| dialect)
        .collect::<std::collections::BTreeSet<_>>()
        .len()
        == 1
    {
        matches[0].0
    } else {
        Dialect::Claude
    };
    (
        dialect,
        DialectEvidence {
            source: DialectEvidenceSource::Predicted,
            paths: matches.into_iter().map(|(_, path)| path).collect(),
        },
    )
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
