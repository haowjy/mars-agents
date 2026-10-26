//! Classify one package root: the ordered dispatch of `mars-normalize`, shared by `prepare`
//! (which reads the root as one package) and `inspect` (which reports it).
//!
//! 1. A directory holding a valid `.mars-plugin.json` is generated output, never input.
//! 2. A Mars manifest decides; no declaration overrides it.
//! 3. A declared dialect (`mars-native` on a root with no manifest is a pre-v1 Mars package,
//!    which the upgrade reads), else
//! 4. the dialect predicted from evidence in the root, strongest first; a key only pre-v1 Mars
//!    defined, found in a definition, makes it a pre-v1 Mars package too.
//! 5. No items at all is `empty`.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::claude::discover::discover_claude;
use crate::claude::manifest::{
    MARKETPLACE_MANIFEST, Marketplace, MarketplaceEntry, PLUGIN_MANIFEST, PLUGIN_NAME_RULE,
    PluginManifest, PluginSpec, is_plugin_name, resolve_spec,
};
use crate::diagnostic::{SchemaDiagnostic, Severity};
use crate::dialect::{Dialect, DialectChoice};
use crate::discover::{Discovery, discover};
use crate::item::DefinitionKind;
use crate::package::{ManifestClassification, classify_manifest};
use crate::plugin_metadata::{SENTINEL_FILE, is_valid_sentinel};
use crate::prepare::{Classification, PrepareContext};
use crate::tree::TreeSource;

/// What a root is, as `inspect` reports it: derived from the classification and layout.
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
    /// The package manifest to prepare. Consumer configuration is not one, so it is `None`
    /// there; the root's `mars.toml` stays the consumer's to read.
    pub manifest: Option<Vec<u8>>,
    pub classification: Classification,
    pub layout: Layout,
    pub discovery: Discovery,
    pub also_found: Vec<Mirror>,
}

impl Dispatched {
    /// What the root is, as `inspect` reports it: the classification, with a foreign root's
    /// layout telling a plugin from a bare skill tree.
    pub(crate) fn kind(&self) -> Kind {
        match (&self.classification, &self.layout) {
            (Classification::Schema1, _) => Kind::Schema1,
            (Classification::MarsLegacy, _) => Kind::MarsLegacy,
            (Classification::SchemaNewer { .. }, _) => Kind::SchemaNewer,
            (Classification::Invalid, _) => Kind::Invalid,
            (Classification::Marketplace, _) => Kind::Marketplace,
            (Classification::Empty, _) => Kind::Empty,
            (Classification::Foreign(_), Layout::Claude(spec)) if !spec.is_bare_tree() => {
                Kind::Plugin
            }
            (Classification::Foreign(_), _) => Kind::SkillTree,
        }
    }

    /// The foreign dialect read and why; `None` for Mars packages and unreadable roots.
    pub(crate) fn dialect(&self) -> Option<&DialectChoice> {
        match &self.classification {
            Classification::Foreign(choice) => Some(choice),
            _ => None,
        }
    }
}

/// A marketplace entry the root is being read as, with whether the entry's source is the
/// marketplace root itself.
#[derive(Clone, Copy)]
pub(crate) struct AsPlugin<'a> {
    pub entry: &'a MarketplaceEntry,
    pub at_marketplace_root: bool,
}

// Claude's own spellings that are enough evidence to predict the Claude dialect. Deliberately
// narrower than the import's vocabulary (`claude::translate`, `skill::tool_keys`): `tools`, `model`
// and `hooks` also appear in native files.
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
    let manifest = source.read("mars.toml");
    let manifest_class = classify_manifest(manifest.as_deref());
    let consumer_tree = consumer_content(context, &manifest_class);
    let mut dispatched = classify(
        source,
        &manifest_class,
        consumer_tree,
        declared,
        as_plugin,
        fallback_name,
    );
    dispatched.manifest = manifest.filter(|_| !consumer_tree);
    dispatched
}

/// Whether the root's items are the consumer's own authored content, read as Mars convention
/// with no manifest.
fn consumer_content(context: PrepareContext, manifest: &ManifestClassification) -> bool {
    matches!(
        (context, manifest),
        (
            PrepareContext::AuthoredLocal,
            ManifestClassification::NoManifest
        ) | (
            PrepareContext::AuthoredLocal | PrepareContext::Project,
            ManifestClassification::ConsumerConfig
        )
    )
}

fn classify(
    source: &impl TreeSource,
    manifest: &ManifestClassification,
    consumer_tree: bool,
    declared: Option<Dialect>,
    as_plugin: Option<AsPlugin<'_>>,
    fallback_name: Option<&str>,
) -> Dispatched {
    let done = |classification, layout, discovery, also_found| Dispatched {
        manifest: None,
        classification,
        layout,
        discovery,
        also_found,
    };
    if source
        .read(SENTINEL_FILE)
        .is_some_and(|bytes| is_valid_sentinel(&bytes))
    {
        return done(
            Classification::Empty,
            Layout::None,
            Discovery::generated_root(),
            Vec::new(),
        );
    }

    match manifest {
        _ if consumer_tree => {
            return done(
                Classification::MarsLegacy,
                Layout::Convention,
                discover(source, fallback_name),
                Vec::new(),
            );
        }
        ManifestClassification::Schema1 => {
            return done(
                Classification::Schema1,
                Layout::Convention,
                discover(source, fallback_name),
                Vec::new(),
            );
        }
        ManifestClassification::MarsLegacy => {
            return done(
                Classification::MarsLegacy,
                Layout::Convention,
                discover(source, fallback_name),
                Vec::new(),
            );
        }
        ManifestClassification::SchemaNewer { schema } => {
            return done(
                Classification::SchemaNewer { schema: *schema },
                Layout::None,
                Discovery::empty(),
                Vec::new(),
            );
        }
        ManifestClassification::Invalid { .. } | ManifestClassification::ConsumerConfig => {
            return done(
                Classification::Invalid,
                Layout::None,
                Discovery::empty(),
                Vec::new(),
            );
        }
        ManifestClassification::NoManifest => {}
    }

    // Mars content with no manifest has no `schema`, so it is pre-v1 by definition: it takes the
    // upgrade, which leaves a file already in v1 as it is.
    if declared == Some(Dialect::MarsNative) {
        return done(
            Classification::MarsLegacy,
            Layout::Convention,
            discover(source, fallback_name),
            Vec::new(),
        );
    }

    let evidence = collect_evidence(source);
    // Hidden containers and harness manifests decide before any item is read; frontmatter
    // evidence needs the items, which Claude's rules find for both of its dialects.
    let decided = declared.map(DialectChoice::declared).or_else(|| {
        evidence
            .strongest()
            .map(|(dialect, paths)| DialectChoice::predicted(dialect, paths))
    });
    let also_found = |chosen: Dialect| evidence.mirrors(chosen, source);

    if decided
        .as_ref()
        .is_none_or(|choice| matches!(choice.dialect, Dialect::Claude | Dialect::AgentSkills))
    {
        let (layout, manifest_diagnostics) = claude_layout(source, as_plugin);
        let mut discovery = match &layout {
            Layout::Claude(spec) => discover_claude(source, spec, fallback_name),
            _ => Discovery::empty(),
        };
        for (path, diagnostic) in manifest_diagnostics {
            discovery
                .diagnostics
                .entry(path)
                .or_default()
                .push(diagnostic);
        }
        if matches!(layout, Layout::None) {
            return done(Classification::Invalid, layout, discovery, Vec::new());
        }
        let choice = match decided {
            Some(choice) => choice,
            // A Claude marketplace named this root as a plugin.
            None if as_plugin.is_some() => DialectChoice::predicted(Dialect::Claude, Vec::new()),
            None => match predict_from_items(source, &discovery) {
                Prediction::Foreign(choice) => choice,
                Prediction::MarsLegacy => {
                    return done(
                        Classification::MarsLegacy,
                        Layout::Convention,
                        discover(source, fallback_name),
                        Vec::new(),
                    );
                }
            },
        };
        let mirrors = also_found(choice.dialect);
        let has_items = !discovery.definitions.is_empty()
            || !discovery.dropped.is_empty()
            || matches!(layout, Layout::Marketplace(_));
        let bare_tree = matches!(&layout, Layout::Claude(spec) if spec.is_bare_tree());
        if !has_items && choice.evidence.paths.is_empty() && bare_tree {
            return done(Classification::Empty, Layout::None, discovery, mirrors);
        }
        let classification = if matches!(layout, Layout::Marketplace(_)) {
            Classification::Marketplace
        } else {
            Classification::Foreign(choice)
        };
        return done(classification, layout, discovery, mirrors);
    }

    let choice = decided.expect("a non-Claude dialect was decided");
    let discovery = discover(source, fallback_name);
    let mirrors = also_found(choice.dialect);
    if discovery.definitions.is_empty() && choice.evidence.paths.is_empty() {
        return done(Classification::Empty, Layout::None, discovery, mirrors);
    }
    done(
        Classification::Foreign(choice),
        Layout::Convention,
        discovery,
        mirrors,
    )
}

/// Marketplace, plugin or bare tree, from the manifests in the root. A manifest that does not
/// parse makes the root invalid.
fn claude_layout(
    source: &impl TreeSource,
    as_plugin: Option<AsPlugin<'_>>,
) -> (Layout, Vec<(String, SchemaDiagnostic)>) {
    let mut diagnostics = Vec::new();
    let invalid = |diagnostic: SchemaDiagnostic, path: &str| {
        (Layout::None, vec![(path.to_owned(), diagnostic)])
    };
    if let Some(plugin) = as_plugin
        && !is_plugin_name(&plugin.entry.name)
    {
        return invalid(
            SchemaDiagnostic::new(
                "marketplace.plugin-name-invalid",
                Severity::Error,
                Vec::new(),
                format!(
                    "plugin name `{}` is not usable ({PLUGIN_NAME_RULE})",
                    plugin.entry.name
                ),
            ),
            MARKETPLACE_MANIFEST,
        );
    }
    if as_plugin.is_none()
        && let Some(bytes) = source.read(MARKETPLACE_MANIFEST)
    {
        return match Marketplace::parse(&bytes, &mut diagnostics) {
            Ok(marketplace) => (Layout::Marketplace(marketplace), diagnostics),
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
        return (Layout::Claude(PluginSpec::walk()), diagnostics);
    }
    match resolve_spec(
        manifest.as_ref(),
        as_plugin.map(|plugin| plugin.entry),
        as_plugin.is_some_and(|plugin| plugin.at_marketplace_root),
    ) {
        Ok(spec) => (Layout::Claude(spec), diagnostics),
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

/// What the items of a manifest-free root say they are.
enum Prediction {
    Foreign(DialectChoice),
    /// A key only pre-v1 Mars defined (see [`is_pre_v1_mars`]).
    MarsLegacy,
}

/// A file only pre-v1 Mars could have written: a removed key or value no foreign import
/// reads (`disallowed-tools` is one Claude's import reads, so it is not evidence).
fn is_pre_v1_mars(kind: DefinitionKind, frontmatter: &crate::frontmatter::Frontmatter) -> bool {
    let removed = match kind {
        DefinitionKind::Agent => crate::agent::REMOVED_AGENT_FIELDS,
        DefinitionKind::Skill => crate::skill::REMOVED_SKILL_FIELDS,
    };
    frontmatter.keys().iter().any(|key| {
        removed
            .iter()
            .any(|field| field.mars_only && field.key == key.as_str())
    }) || (kind == DefinitionKind::Agent
        && crate::agent::REMOVED_AGENT_VALUES.iter().any(|removed| {
            removed.mars_only
                && frontmatter
                    .get(removed.key)
                    .and_then(serde_yaml::Value::as_str)
                    == Some(removed.value)
        }))
}

/// No manifest or container decided: the discovered files do. A key only pre-v1 Mars defined
/// makes the root a pre-v1 Mars package; Claude-only keys predict Claude; skills with only the
/// standard's keys are `agent-skills`; anything else is Claude, Mars's default.
fn predict_from_items(source: &impl TreeSource, discovery: &Discovery) -> Prediction {
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
        if is_pre_v1_mars(definition.kind, &frontmatter) {
            return Prediction::MarsLegacy;
        }
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
    Prediction::Foreign(if !claude_paths.is_empty() {
        claude_paths.truncate(5);
        DialectChoice::predicted(Dialect::Claude, claude_paths)
    } else if skills > 0 && standard_only {
        DialectChoice::predicted(Dialect::AgentSkills, Vec::new())
    } else {
        DialectChoice::predicted(Dialect::Claude, Vec::new())
    })
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
