//! One-pass package classification, normalization, selection, and strict reading.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::diagnostic::{SchemaDiagnostic, Severity};
use crate::dialect::{Dialect, DialectChoice};
use crate::discover::{SelectedDefinition, payload_paths};
use crate::dispatch::{AsPlugin, Dispatched, Layout, dispatch};
use crate::frontmatter::{Frontmatter, FrontmatterError};
use crate::item::DefinitionKind;
use crate::read::{DefinitionRead, Reading, read_authored, read_definition};
use crate::selection::{Definition, Request, SelectionFacts};
use crate::tool_policy::{RuleOrigin, ToolRule};
use crate::tree::TreeSource;

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

/// What a package root is, once its manifest, declared dialect and contents have been weighed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Classification {
    #[serde(rename = "schema-1")]
    Schema1,
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
    pub reading: crate::manifest::ManifestRead,
    pub change: ChangeRecord,
}

#[derive(Debug, Clone, Serialize)]
pub struct PreparedDefinition {
    pub kind: DefinitionKind,
    pub slug: String,
    pub source_path: String,
    pub output_path: String,
    /// A skill's folder in the package tree (empty for a root skill); `None` for an agent.
    #[serde(skip)]
    pub skill_root: Option<String>,
    /// The normalized file; crosses to JavaScript as a `Uint8Array`.
    #[serde(with = "serde_bytes")]
    pub normalized_bytes: Vec<u8>,
    /// The Agent or skill reading `kind` names.
    pub reading: Reading,
    pub change: ChangeRecord,
}

/// An MCP server translated from another dialect's declaration, as the `mcp/<name>/mcp.toml` it
/// is written to. The same shape as a definition's output so a caller writes it the same way.
#[derive(Debug, Clone, Serialize)]
pub struct PreparedMcpServer {
    pub name: String,
    /// The file that declared the server (`.mcp.json`, `plugin.json`).
    pub source_path: String,
    pub output_path: String,
    /// The rendered `mcp.toml`; crosses to JavaScript as a `Uint8Array`.
    #[serde(with = "serde_bytes")]
    pub normalized_bytes: Vec<u8>,
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
    /// The MCP servers a foreign plugin declares, translated. Selection does not apply to them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<PreparedMcpServer>,
    pub payload: Vec<PayloadEntry>,
    /// What item selection reads of this package (see [`selection_facts`]).
    pub selection: SelectionFacts,
    /// Package-level findings by source path: what discovery and translation met and could not
    /// convert. A definition's own reading diagnostics live on its `reading` only; see
    /// [`Prepared::all_diagnostics`].
    pub diagnostics: BTreeMap<String, Vec<SchemaDiagnostic>>,
    /// What translating a foreign package changed in each file, by source path: the keys it
    /// dropped, moved or repaired. A caller reports [`Prepared::changed_files`].
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub changes: BTreeMap<String, Vec<SchemaDiagnostic>>,
}

/// What item selection reads of a prepared package: its Agents' and skills' skill refs, and the
/// `[dependencies]` entries and `uses-providers` its manifest reads.
pub(crate) fn selection_facts(
    manifest: Option<&PreparedManifest>,
    definitions: &[PreparedDefinition],
) -> SelectionFacts {
    let reading = manifest.map(|manifest| &manifest.reading);
    SelectionFacts {
        definitions: definitions
            .iter()
            .map(|definition| Definition {
                kind: definition.kind,
                slug: definition.slug.clone(),
                output_path: definition.output_path.clone(),
                skills: match &definition.reading {
                    Reading::Agent(read) => read
                        .fields
                        .as_ref()
                        .map(|profile| profile.skills.all())
                        .unwrap_or_default(),
                    _ => Vec::new(),
                },
            })
            .collect(),
        edges: reading
            .map(|reading| {
                reading
                    .dependencies
                    .dependencies
                    .iter()
                    .map(|(name, entry)| Request {
                        name: name.clone(),
                        filter: entry.filter.to_mode(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        uses_providers: reading
            .and_then(|reading| reading.package.fields.as_ref())
            .map(|package| package.uses_providers.clone())
            .unwrap_or_default(),
    }
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

    /// The changes that ignored a non-string `effort`, `mode` or `harness`, by source path. A
    /// sync leaves these in the count of changed files; `mars check` prints them, because the
    /// author of the file can fix them.
    pub fn ignored_execution_fields(&self) -> impl Iterator<Item = (&str, &SchemaDiagnostic)> {
        self.changes.iter().flat_map(|(path, changes)| {
            changes
                .iter()
                .filter(|change| change.code == crate::set_aside::EXECUTION_IGNORED)
                .map(move |change| (path.as_str(), change))
        })
    }

    /// The changes that quoted a value YAML could not read as plain text, by source path. Like
    /// [`Self::ignored_execution_fields`], `mars check` prints them for the author to fix and a
    /// sync only counts the file.
    pub fn repaired_values(&self) -> impl Iterator<Item = (&str, &SchemaDiagnostic)> {
        self.changes.iter().flat_map(|(path, changes)| {
            changes
                .iter()
                .filter(|change| change.code == crate::claude::translate::REPAIRED)
                .map(move |change| (path.as_str(), change))
        })
    }

    /// How many authored files translation changed: the prepared bytes differ from the file. A
    /// generated `mars.toml` has no file to change and is not counted.
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
                .extend(&manifest.reading.package.diagnostics);
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
    let mut changes = BTreeMap::new();

    let mut manifest = manifest_bytes.as_deref().map(prepare_manifest);

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
    let authored = matches!(classification, Classification::Schema1);
    let definitions = if can_read_definitions {
        selected
            .iter()
            .map(|definition| {
                let mut prepared = prepare_definition(
                    source,
                    definition,
                    &classification,
                    authored,
                    &mut diagnostics,
                    &mut changes,
                );
                if let Some(manifest) = &manifest {
                    add_package_tools(&mut prepared, &manifest.reading);
                }
                prepared
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

    let mut mcp_servers = Vec::new();
    if can_read_definitions {
        report_unconverted(source, &dispatched, &mut diagnostics);
        // A bare skill tree's `.mcp.json` is usually its contributors' own, not a plugin's.
        if let Layout::Claude(spec) = &dispatched.layout
            && !spec.is_bare_tree()
        {
            let imported = crate::claude::mcp::import(source, spec);
            for (path, diagnostic) in imported.diagnostics {
                push(&mut diagnostics, &path, diagnostic);
            }
            mcp_servers = imported
                .servers
                .into_iter()
                .map(|server| PreparedMcpServer {
                    output_path: format!("mcp/{}/mcp.toml", server.name),
                    normalized_bytes: crate::mcp_server::render_mcp_toml(&server.def).into_bytes(),
                    name: server.name,
                    source_path: server.source_path,
                })
                .collect();
        }
    }
    diagnostics.retain(|_, diagnostics| !diagnostics.is_empty());
    changes.retain(|_, diagnostics| !diagnostics.is_empty());
    let manifest = manifest.take();
    let prepared = Prepared {
        classification,
        selection: selection_facts(manifest.as_ref(), &definitions),
        manifest,
        definitions,
        mcp_servers,
        payload,
        diagnostics,
        changes,
    };
    Rooted {
        prepared,
        dispatched,
    }
}

/// Add the package's `[package.tools]` rules to an Agent's reading, as `mars sync` writes them
/// into the installed file (`package_tools::missing`). `normalized_bytes` stay the file's: the
/// rules belong to the package, not to the authored profile.
fn add_package_tools(
    definition: &mut PreparedDefinition,
    manifest: &crate::manifest::ManifestRead,
) {
    let Reading::Agent(read) = &mut definition.reading else {
        return;
    };
    // `mars sync` refuses a manifest with any error and installs none of its Agents, so an
    // Agent read beside one carries the failure instead of reading clean without the rules.
    if let Some(error) = manifest.first_error() {
        read.diagnostics.push(SchemaDiagnostic::new(
            "package-tools.unreadable",
            Severity::Error,
            Vec::new(),
            format!(
                "the package's mars.toml has an error, so its `[package.tools]` rules cannot be added and `mars sync` refuses the package: {}",
                error.message
            ),
        ));
        return;
    }
    let (Some(package), Some(fields)) = (
        manifest
            .package
            .fields
            .as_ref()
            .map(|package| &package.tools),
        read.fields.as_mut(),
    ) else {
        return;
    };
    // The reading's own `frontmatter` is ordered JSON; the shared rule reads the YAML mapping.
    let Some(profile) = std::str::from_utf8(&definition.normalized_bytes)
        .ok()
        .and_then(|text| Frontmatter::parse(text).ok())
    else {
        return;
    };
    for (name, action) in crate::package_tools::missing(&profile, package) {
        fields.tools.push_unique(
            ToolRule::from_parsed(name.parsed().clone(), action).with_origin(RuleOrigin::Package),
        );
    }
}

/// Commands are dropped and hooks are not converted yet. MCP servers are translated, not
/// reported here (`claude::mcp`).
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
    if spec.is_bare_tree() && source.is_file(".mcp.json") {
        warn(
            ".mcp.json",
            "import.mcp-not-imported",
            "this is a skill tree, not a Claude plugin (it has no `.claude-plugin/plugin.json`), so its MCP servers were not imported".into(),
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
}

fn prepare_manifest(bytes: &[u8]) -> PreparedManifest {
    let reading = match std::str::from_utf8(bytes) {
        Ok(text) => crate::manifest::read(text),
        Err(error) => crate::manifest::ManifestRead::unread(vec![SchemaDiagnostic::new(
            "text.invalid-utf8",
            Severity::Error,
            Vec::new(),
            format!("mars.toml is not UTF-8: {error}"),
        )]),
    };
    PreparedManifest {
        source_path: Some("mars.toml".to_string()),
        output_path: "mars.toml".to_string(),
        normalized_bytes: bytes.to_vec(),
        reading,
        change: ChangeRecord::None,
    }
}

fn prepare_definition(
    source: &impl TreeSource,
    selected: &SelectedDefinition,
    classification: &Classification,
    authored: bool,
    diagnostics: &mut BTreeMap<String, Vec<SchemaDiagnostic>>,
    changes: &mut BTreeMap<String, Vec<SchemaDiagnostic>>,
) -> PreparedDefinition {
    let Some(bytes) = source.read(&selected.source_path) else {
        return PreparedDefinition {
            kind: selected.kind,
            slug: selected.slug.clone(),
            source_path: selected.source_path.clone(),
            output_path: selected.output_path.clone(),
            skill_root: selected.skill_root.clone(),
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
    let (normalized_bytes, change) = match (std::str::from_utf8(&bytes), classification) {
        (Ok(text), Classification::Foreign(choice)) => {
            let translated = translate_foreign(text, choice.dialect, selected, &bytes);
            notes = translated.changes;
            findings = translated.findings;
            (translated.bytes, translated.change)
        }
        _ => (bytes.clone(), ChangeRecord::None),
    };
    changes
        .entry(selected.source_path.clone())
        .or_default()
        .extend(notes);
    diagnostics
        .entry(selected.source_path.clone())
        .or_default()
        .extend(findings);

    // Only text is translated, so these bytes are the file's when they are not text.
    let reading = match std::str::from_utf8(&normalized_bytes) {
        Ok(text) if authored => read_authored(selected.kind, text),
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
        skill_root: selected.skill_root.clone(),
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
/// stays as written, with the reader's error; a Claude definition's unquoted values that YAML
/// cannot read are quoted first.
fn translate_foreign(
    text: &str,
    dialect: Dialect,
    selected: &SelectedDefinition,
    bytes: &[u8],
) -> Translated {
    let claude = matches!(dialect, Dialect::Claude | Dialect::AgentSkills);
    let mut changes = Vec::new();
    let mut repaired = false;
    let frontmatter = match Frontmatter::parse(text) {
        Ok(frontmatter) => frontmatter,
        Err(FrontmatterError::MalformedYaml(_)) if claude => {
            match crate::claude::translate::repair_plain_values(text)
                .and_then(|(fixed, keys)| Some((Frontmatter::parse(&fixed).ok()?, keys)))
            {
                Some((frontmatter, keys)) => {
                    repaired = true;
                    changes.push(crate::claude::translate::repaired_diagnostic(&keys));
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
