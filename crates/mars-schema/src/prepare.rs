//! One-pass package classification, normalization, selection, and strict reading.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::diagnostic::{PathSegment, SchemaDiagnostic, Severity};
use crate::dialect::{Dialect, DialectChoice};
use crate::discover::{SelectedDefinition, payload_paths};
use crate::dispatch::{AsPlugin, Dispatched, Layout, dispatch};
use crate::frontmatter::Frontmatter;
use crate::item::DefinitionKind;
use crate::read::{DefinitionRead, Reading, read_definition};
use crate::tree::TreeSource;
use crate::upgrade::{UpgradeError, UpgradeResult};

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

/// The directory below a consumer project that holds its own authored content.
pub const LOCAL_SOURCE_DIR: &str = ".mars-src";

/// Where a project directory's authored content lives and how it is read, from its `mars.toml`
/// bytes: the project root as a `Package`, or, when the manifest is consumer configuration with
/// no `[package]`, its `.mars-src` as `AuthoredLocal`. A caller that writes to the authored
/// files, as `mars migrate` does, asks here instead of parsing `mars.toml` itself.
pub fn project_authored_root(project: &Path, manifest: Option<&[u8]>) -> (PathBuf, PrepareContext) {
    match crate::package::classify_manifest(manifest) {
        crate::package::ManifestClassification::ConsumerConfig => (
            project.join(LOCAL_SOURCE_DIR),
            PrepareContext::AuthoredLocal,
        ),
        _ => (project.to_path_buf(), PrepareContext::Package),
    }
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
    /// Package-level findings by source path: what discovery and translation met and could not
    /// convert. A definition's own reading diagnostics live on its `reading` only; see
    /// [`Prepared::all_diagnostics`].
    pub diagnostics: BTreeMap<String, Vec<SchemaDiagnostic>>,
    /// What preparing changed in each file, by source path: the keys a pre-v1 upgrade or a
    /// foreign translation dropped, moved or repaired. A caller that only reads the package
    /// reports [`Prepared::changed_files`]; `mars migrate` lists these.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub changes: BTreeMap<String, Vec<SchemaDiagnostic>>,
}

impl Prepared {
    /// The changes that lost a restriction (a `deny` or `ask` tool entry, `permission`,
    /// `sandbox`, `approval`), by source path. Unlike the rest of `changes`, a caller reports
    /// these on every run: each leaves an agent less restricted than its file says.
    pub fn lost_restrictions(&self) -> impl Iterator<Item = (&str, &SchemaDiagnostic)> {
        self.changes.iter().flat_map(|(path, changes)| {
            changes
                .iter()
                .filter(|change| change.code == crate::set_aside::RESTRICTION_LOST)
                .map(move |change| (path.as_str(), change))
        })
    }

    /// How many authored files preparation changed: the prepared bytes differ from the file, and
    /// for a pre-v1 package `mars migrate` writes them back. A generated `mars.toml` has no file
    /// to change and is not counted.
    pub fn changed_files(&self) -> usize {
        self.manifest
            .iter()
            .map(|manifest| &manifest.change)
            .chain(self.definitions.iter().map(|definition| &definition.change))
            .filter(|change| !matches!(change, ChangeRecord::None | ChangeRecord::Generated))
            .count()
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
    /// Repair what a schema 1 definition can still need: the replacement of an Agent key schema 1
    /// removed in favour of a `harness-overrides` key. Only `mars migrate` asks, because it
    /// writes the result back; a reader leaves it off, because an authored schema 1 file is read
    /// exactly as written.
    pub migrate: bool,
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

/// `prepare` as `mars migrate` runs it: a definition it could repair is repaired in
/// `normalized_bytes`, for the migration to write back to the authored file.
pub fn prepare_for_migration(
    source: &impl TreeSource,
    context: PrepareContext,
    declared_dialect: Option<Dialect>,
) -> Prepared {
    let options = RootOptions {
        migrate: true,
        ..RootOptions::default()
    };
    prepare_root(source, context, declared_dialect, options).prepared
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
    let mut changes = BTreeMap::new();

    let mut manifest = manifest_bytes.as_deref().map(|bytes| {
        prepare_manifest(
            bytes,
            classification == Classification::MarsLegacy,
            &mut diagnostics,
            &mut changes,
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
    let shaping = Shaping::of(&classification, options.migrate);
    let definitions = if can_read_definitions {
        selected
            .iter()
            .map(|definition| {
                prepare_definition(
                    source,
                    definition,
                    shaping,
                    all,
                    &selected_paths,
                    &mut diagnostics,
                    &mut changes,
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
            matches!(classification, Classification::Foreign(_)),
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
    changes.retain(|_, diagnostics| !diagnostics.is_empty());
    let prepared = Prepared {
        classification,
        manifest: manifest.take(),
        definitions,
        payload,
        diagnostics,
        changes,
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
    changes: &mut BTreeMap<String, Vec<SchemaDiagnostic>>,
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
    changes
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

/// A definition's authored keys, made schema v1.
type Rewrite = fn(DefinitionKind, &str) -> Result<UpgradeResult, UpgradeError>;

/// How the root's definitions are made schema v1, decided once for the root.
#[derive(Clone, Copy)]
enum Shaping {
    /// Nothing to convert: each file is read as written.
    None,
    /// The pre-v1 upgrade, or `mars migrate`'s repair of a schema 1 file.
    Rewrite(Rewrite),
    /// A foreign dialect, translated into schema v1.
    Foreign(Dialect),
}

impl Shaping {
    fn of(classification: &Classification, migrate: bool) -> Self {
        match classification {
            Classification::MarsLegacy => Self::Rewrite(crate::upgrade::upgrade_definition),
            // Only `mars migrate` writes the result back, so only it repairs a schema 1 file.
            Classification::Schema1 if migrate => {
                Self::Rewrite(crate::upgrade::relocate_removed_fields)
            }
            Classification::Foreign(choice) => Self::Foreign(choice.dialect),
            _ => Self::None,
        }
    }
}

fn prepare_definition(
    source: &impl TreeSource,
    selected: &SelectedDefinition,
    shaping: Shaping,
    all: &[SelectedDefinition],
    selected_paths: &BTreeSet<&str>,
    diagnostics: &mut BTreeMap<String, Vec<SchemaDiagnostic>>,
    changes: &mut BTreeMap<String, Vec<SchemaDiagnostic>>,
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
    let (mut notes, mut findings) = (Vec::new(), Vec::new());
    let (normalized_bytes, change) = match std::str::from_utf8(&bytes) {
        Ok(text) => match shaping {
            Shaping::Rewrite(rewrite) => match rewrite(selected.kind, text) {
                Ok(result) => {
                    let (rewritten, change, upgrade_notes) = upgrade_result(text, result);
                    notes = upgrade_notes;
                    (rewritten, change)
                }
                // The reader reports a file that does not parse; the upgrade only met its
                // error, so a second diagnostic would say the same thing twice.
                Err(UpgradeError::Frontmatter(_)) => (bytes.clone(), ChangeRecord::None),
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
                    (bytes.clone(), ChangeRecord::None)
                }
            },
            Shaping::Foreign(dialect) => {
                let translated =
                    translate_foreign(text, dialect, selected, all, selected_paths, &bytes);
                notes = translated.changes;
                findings = translated.findings;
                (translated.bytes, translated.change)
            }
            Shaping::None => (bytes.clone(), ChangeRecord::None),
        },
        Err(_) => (bytes.clone(), ChangeRecord::None),
    };
    changes
        .entry(selected.source_path.clone())
        .or_default()
        .extend(notes);
    diagnostics
        .entry(selected.source_path.clone())
        .or_default()
        .extend(findings);

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

/// What translating one foreign definition produced.
struct Translated {
    bytes: Vec<u8>,
    change: ChangeRecord,
    /// What the translation dropped, moved or repaired.
    changes: Vec<SchemaDiagnostic>,
    /// What it met and could not convert.
    findings: Vec<SchemaDiagnostic>,
}

impl Translated {
    fn untouched(bytes: &[u8]) -> Self {
        Self {
            bytes: bytes.to_vec(),
            change: ChangeRecord::None,
            changes: Vec::new(),
            findings: Vec::new(),
        }
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
) -> Translated {
    let claude = matches!(dialect, Dialect::Claude | Dialect::AgentSkills);
    let mut changes = Vec::new();
    let mut repaired = false;
    let frontmatter = match Frontmatter::parse(text) {
        Ok(frontmatter) => frontmatter,
        Err(_) if claude && selected.kind == DefinitionKind::Skill => {
            match crate::claude::translate::repair_description(text)
                .and_then(|fixed| Frontmatter::parse(&fixed).ok())
            {
                Some(frontmatter) => {
                    repaired = true;
                    changes.push(crate::claude::translate::repaired_diagnostic());
                    frontmatter
                }
                None => return Translated::untouched(bytes),
            }
        }
        Err(_) => return Translated::untouched(bytes),
    };
    let lifted = crate::lift::lift_frontmatter(dialect, selected.kind, &frontmatter);
    changes.extend(lifted.changes);
    let mut findings = lifted.findings;
    if claude && selected.kind == DefinitionKind::Skill {
        findings.extend(crate::claude::checks::skill_checks(
            selected,
            &lifted.frontmatter,
            all,
            selected_paths,
        ));
    }
    let (bytes, change) = if lifted.changed || repaired {
        (
            lifted.frontmatter.render().into_bytes(),
            ChangeRecord::FullyRewritten,
        )
    } else {
        (bytes.to_vec(), ChangeRecord::None)
    };
    Translated {
        bytes,
        change,
        changes,
        findings,
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
