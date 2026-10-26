//! Classify one package root: the ordered dispatch of `mars-normalize`, shared by `prepare`
//! (which reads the root as one package) and `inspect` (which reports it).
//!
//! 1. A directory holding a valid `.mars-plugin.json` is generated output, never input.
//! 2. A Mars manifest decides; no declaration overrides it.
//! 3. A declared dialect, else
//! 4. the dialect predicted from evidence in the root, strongest first.
//! 5. No items at all is `empty`.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::claude::discover::discover_claude;
use crate::claude::manifest::{
    MARKETPLACE_MANIFEST, Marketplace, MarketplaceEntry, PLUGIN_MANIFEST, PluginManifest,
    PluginSpec, resolve_spec,
};
use crate::diagnostic::{SchemaDiagnostic, Severity};
use crate::dialect::Dialect;
use crate::discover::{Discovery, discover};
use crate::package::{ManifestClassification, classify_manifest};
use crate::plugin_metadata::{SENTINEL_FILE, is_valid_sentinel};
use crate::prepare::{Classification, DialectEvidence, DialectEvidenceSource, PrepareContext};
use crate::tree::TreeSource;
use crate::upgrade::DefinitionKind;

/// What a root is, as `inspect` reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    #[serde(rename = "schema-1")]
    Schema1,
    MarsLegacy,
    SchemaNewer,
    Marketplace,
    Plugin,
    SkillTree,
    Invalid,
    Empty,
}

/// How the root's items are found.
#[derive(Debug, Clone)]
pub(crate) enum Layout {
    /// Nothing is read (invalid, newer, empty, generated).
    None,
    /// Mars's convention walk, output equal to source.
    Convention,
    /// Claude plugin rules, relocating into schema v1.
    Claude(PluginSpec),
    Marketplace(Marketplace),
}

/// A harness's copy of the package, found beside the one that is read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Mirror {
    pub dialect: Dialect,
    pub paths: Vec<String>,
    /// A weak signal (a Codex sidecar many Claude-shaped repositories also ship) is reported
    /// and never decides.
    pub weak: bool,
}

pub(crate) struct Dispatched {
    pub kind: Kind,
    pub classification: Classification,
    pub layout: Layout,
    pub discovery: Discovery,
    pub also_found: Vec<Mirror>,
    /// The foreign dialect read and why; `None` for Mars packages and unreadable roots.
    pub dialect: Option<(Dialect, DialectEvidence)>,
}

/// A marketplace entry the root is being read as, with whether the entry's source is the
/// marketplace root itself.
#[derive(Clone, Copy)]
pub(crate) struct AsPlugin<'a> {
    pub entry: &'a MarketplaceEntry,
    pub at_marketplace_root: bool,
}

// Claude-only frontmatter keys that predict the Claude dialect.
const CLAUDE_KEYS: &[&str] = &[
    "allowed-tools",
    "disable-model-invocation",
    "disallowedTools",
    "disallowed-tools",
    "argument-hint",
    "mcpServers",
];
const STANDARD_SKILL_KEYS: &[&str] = &[
    "name",
    "description",
    "license",
    "compatibility",
    "metadata",
];

pub(crate) fn dispatch(
    source: &impl TreeSource,
    context: PrepareContext,
    declared: Option<Dialect>,
    as_plugin: Option<AsPlugin<'_>>,
    fallback_name: Option<&str>,
) -> Dispatched {
    let finished = |kind, classification, layout, discovery, also_found| Dispatched {
        kind,
        classification,
        layout,
        discovery,
        also_found,
        dialect: None,
    };
    if source
        .read(SENTINEL_FILE)
        .is_some_and(|bytes| is_valid_sentinel(&bytes))
    {
        return finished(
            Kind::Empty,
            Classification::Empty,
            Layout::None,
            discover(source, fallback_name),
            Vec::new(),
        );
    }

    match classify_manifest(source.read("mars.toml").as_deref()) {
        ManifestClassification::Schema1 => {
            return finished(
                Kind::Schema1,
                Classification::Schema1,
                Layout::Convention,
                discover(source, fallback_name),
                Vec::new(),
            );
        }
        ManifestClassification::MarsLegacy => {
            return finished(
                Kind::MarsLegacy,
                Classification::MarsLegacy,
                Layout::Convention,
                discover(source, fallback_name),
                Vec::new(),
            );
        }
        ManifestClassification::SchemaNewer { schema } => {
            return finished(
                Kind::SchemaNewer,
                Classification::SchemaNewer { schema },
                Layout::None,
                Discovery::empty(),
                Vec::new(),
            );
        }
        ManifestClassification::Invalid { .. } => {
            return finished(
                Kind::Invalid,
                Classification::Invalid,
                Layout::None,
                Discovery::empty(),
                Vec::new(),
            );
        }
        ManifestClassification::NoManifest if context == PrepareContext::AuthoredLocal => {
            return finished(
                Kind::MarsLegacy,
                Classification::MarsLegacy,
                Layout::Convention,
                discover(source, fallback_name),
                Vec::new(),
            );
        }
        ManifestClassification::NoManifest => {}
    }

    let evidence = collect_evidence(source);
    // Hidden containers and harness manifests decide before any item is read; frontmatter
    // evidence needs the items, which Claude's rules find for both of its dialects.
    let mut decided = declared.map(|dialect| {
        (
            dialect,
            DialectEvidence {
                source: DialectEvidenceSource::Declared,
                paths: Vec::new(),
            },
        )
    });
    if decided.is_none()
        && let Some((dialect, paths)) = evidence.strongest()
    {
        decided = Some((
            dialect,
            DialectEvidence {
                source: DialectEvidenceSource::Predicted,
                paths,
            },
        ));
    }
    let dialect_of = |decided: &Option<(Dialect, DialectEvidence)>| decided.as_ref().map(|d| d.0);

    let also_found = |chosen: Dialect| evidence.mirrors(chosen, source);

    if matches!(
        dialect_of(&decided),
        None | Some(Dialect::Claude | Dialect::AgentSkills)
    ) {
        let (layout, kind, mut manifest_diagnostics) = claude_layout(source, as_plugin);
        let discovery = match &layout {
            Layout::Claude(spec) => discover_claude(source, spec, fallback_name),
            _ => Discovery::empty(),
        };
        let mut discovery = discovery;
        for (path, diagnostic) in manifest_diagnostics.drain(..) {
            discovery
                .diagnostics
                .entry(path)
                .or_default()
                .push(diagnostic);
        }
        if matches!(layout, Layout::None) {
            return finished(
                Kind::Invalid,
                Classification::Invalid,
                layout,
                discovery,
                Vec::new(),
            );
        }
        let decided = match decided {
            Some(decided) => decided,
            None => predict_from_items(source, &discovery),
        };
        let dialect = decided.0;
        let mirrors = also_found(dialect);
        let has_items = !discovery.definitions.is_empty()
            || !discovery.dropped.is_empty()
            || matches!(layout, Layout::Marketplace(_));
        if !has_items && decided.1.paths.is_empty() && kind == Kind::SkillTree {
            return finished(
                Kind::Empty,
                Classification::Empty,
                Layout::None,
                discovery,
                mirrors,
            );
        }
        let classification = if kind == Kind::Marketplace {
            Classification::Marketplace
        } else {
            Classification::Foreign {
                dialect,
                evidence: decided.1.clone(),
            }
        };
        let mut dispatched = finished(kind, classification, layout, discovery, mirrors);
        dispatched.dialect = Some((dialect, decided.1));
        return dispatched;
    }

    let (dialect, evidence_of) = decided.expect("a non-Claude dialect was decided");
    let discovery = discover(source, fallback_name);
    let mirrors = also_found(dialect);
    if discovery.definitions.is_empty() && evidence_of.paths.is_empty() {
        return finished(
            Kind::Empty,
            Classification::Empty,
            Layout::None,
            discovery,
            mirrors,
        );
    }
    let mut dispatched = finished(
        Kind::SkillTree,
        Classification::Foreign {
            dialect,
            evidence: evidence_of.clone(),
        },
        Layout::Convention,
        discovery,
        mirrors,
    );
    dispatched.dialect = Some((dialect, evidence_of));
    dispatched
}

/// Marketplace, plugin or bare tree, from the manifests in the root. A manifest that does not
/// parse makes the root invalid.
fn claude_layout(
    source: &impl TreeSource,
    as_plugin: Option<AsPlugin<'_>>,
) -> (Layout, Kind, Vec<(String, SchemaDiagnostic)>) {
    let mut diagnostics = Vec::new();
    let invalid = |diagnostic: SchemaDiagnostic, path: &str| {
        (
            Layout::None,
            Kind::Invalid,
            vec![(path.to_owned(), diagnostic)],
        )
    };
    if as_plugin.is_none()
        && let Some(bytes) = source.read(MARKETPLACE_MANIFEST)
    {
        return match Marketplace::parse(&bytes, &mut diagnostics) {
            Ok(marketplace) => (
                Layout::Marketplace(marketplace),
                Kind::Marketplace,
                diagnostics,
            ),
            Err(diagnostic) => invalid(diagnostic, MARKETPLACE_MANIFEST),
        };
    }
    let manifest = match source.read(PLUGIN_MANIFEST) {
        Some(bytes) => match PluginManifest::parse(&bytes, &mut diagnostics) {
            Ok(manifest) => Some(manifest),
            Err(diagnostic) => return invalid(diagnostic, PLUGIN_MANIFEST),
        },
        None => None,
    };
    if manifest.is_none() && as_plugin.is_none() {
        return (
            Layout::Claude(PluginSpec::walk()),
            Kind::SkillTree,
            diagnostics,
        );
    }
    match resolve_spec(
        manifest.as_ref(),
        as_plugin.map(|plugin| plugin.entry),
        as_plugin.is_some_and(|plugin| plugin.at_marketplace_root),
    ) {
        Ok(spec) => (Layout::Claude(spec), Kind::Plugin, diagnostics),
        Err(message) => invalid(
            SchemaDiagnostic::new(
                "plugin.conflicting-manifests",
                Severity::Error,
                Vec::new(),
                message,
            ),
            PLUGIN_MANIFEST,
        ),
    }
}

/// No manifest or container decided: the discovered files do. Claude-only keys predict
/// Claude; skills with only the standard's keys are `agent-skills`; anything else is Claude,
/// Mars's default.
fn predict_from_items(
    source: &impl TreeSource,
    discovery: &Discovery,
) -> (Dialect, DialectEvidence) {
    let mut claude_paths = Vec::new();
    let mut standard_only = true;
    let mut skills = 0;
    for definition in &discovery.definitions {
        if definition.kind == DefinitionKind::Agent {
            standard_only = false;
        } else {
            skills += 1;
        }
        let Some(frontmatter) = source
            .read(&definition.source_path)
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .and_then(|text| crate::frontmatter::Frontmatter::parse(&text).ok())
        else {
            continue;
        };
        let keys = frontmatter.keys();
        if keys.iter().any(|key| CLAUDE_KEYS.contains(&key.as_str())) {
            claude_paths.push(definition.source_path.clone());
        }
        if definition.kind == DefinitionKind::Skill
            && keys
                .iter()
                .any(|key| !STANDARD_SKILL_KEYS.contains(&key.as_str()))
        {
            standard_only = false;
        }
    }
    let predicted = |dialect, paths| {
        (
            dialect,
            DialectEvidence {
                source: DialectEvidenceSource::Predicted,
                paths,
            },
        )
    };
    if !claude_paths.is_empty() {
        claude_paths.truncate(5);
        predicted(Dialect::Claude, claude_paths)
    } else if skills > 0 && standard_only {
        predicted(Dialect::AgentSkills, Vec::new())
    } else {
        predicted(Dialect::Claude, Vec::new())
    }
}

/// Files that vouch for a dialect, found in the root only.
#[derive(Default)]
struct Evidence(BTreeMap<Dialect, Vec<String>>);

/// Strongest first. A Claude plugin root outranks a Cursor or Codex mirror of it.
const STRENGTH: &[Dialect] = &[
    Dialect::Claude,
    Dialect::Cursor,
    Dialect::Codex,
    Dialect::OpenCode,
];

fn collect_evidence(source: &impl TreeSource) -> Evidence {
    let mut evidence = Evidence::default();
    let mut add = |dialect, path: String| evidence.0.entry(dialect).or_default().push(path);
    for path in [PLUGIN_MANIFEST, MARKETPLACE_MANIFEST] {
        if source.is_file(path) {
            add(Dialect::Claude, path.to_owned());
        }
    }
    for (container, dialect) in [
        (".claude", Dialect::Claude),
        (".cursor", Dialect::Cursor),
        (".codex", Dialect::Codex),
        (".opencode", Dialect::OpenCode),
    ] {
        for kind in ["skills", "agents"] {
            let path = format!("{container}/{kind}");
            if source.is_dir(&path) && !source.children(&path).is_empty() {
                add(dialect, path);
                break;
            }
        }
    }
    if source.is_dir(".cursor-plugin") {
        add(Dialect::Cursor, ".cursor-plugin".to_owned());
    }
    for rules in [".cursor/rules", "rules"] {
        if let Some(rule) = source
            .children(rules)
            .into_iter()
            .find(|path| path.ends_with(".mdc"))
        {
            add(Dialect::Cursor, rule);
        }
    }
    if source.is_dir(".codex-plugin") {
        add(Dialect::Codex, ".codex-plugin".to_owned());
    }
    evidence
}

impl Evidence {
    fn strongest(&self) -> Option<(Dialect, Vec<String>)> {
        STRENGTH
            .iter()
            .find_map(|dialect| Some((*dialect, self.0.get(dialect)?.clone())))
    }

    /// Everything found for a dialect other than the one read, plus the weak Codex sidecar.
    fn mirrors(&self, chosen: Dialect, source: &impl TreeSource) -> Vec<Mirror> {
        let mut mirrors = STRENGTH
            .iter()
            .filter(|dialect| **dialect != chosen)
            .filter_map(|dialect| {
                Some(Mirror {
                    dialect: *dialect,
                    paths: self.0.get(dialect)?.clone(),
                    weak: false,
                })
            })
            .collect::<Vec<_>>();
        if chosen != Dialect::Codex {
            let sidecars = openai_sidecars(source);
            if !sidecars.is_empty() {
                mirrors.push(Mirror {
                    dialect: Dialect::Codex,
                    paths: sidecars,
                    weak: true,
                });
            }
        }
        mirrors
    }
}

/// `agents/openai.yaml` beside a `SKILL.md`: a Codex sidecar, found one skill level down.
fn openai_sidecars(source: &impl TreeSource) -> Vec<String> {
    let mut found = BTreeSet::new();
    let mut check = |dir: &str| {
        let path = format!("{dir}/agents/openai.yaml");
        if source.is_dir(dir) && source.is_file(&format!("{dir}/SKILL.md")) && source.is_file(&path)
        {
            found.insert(path);
        }
    };
    for group in source.children("skills") {
        check(&group);
        for child in source.children(&group) {
            check(&child);
        }
    }
    found.into_iter().take(5).collect()
}
