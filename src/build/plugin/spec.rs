//! Declarative plugin harness and generated-tree specifications.
//!
//! One row per consumer harness (tree, naming, loader capability, display, README
//! notes, verification evidence) and one row per generated tree (`TREE_PROFILES`,
//! defined beside the writer that emits it). Harness rows point at their tree row
//! directly, so a harness and its tree cannot disagree and no lookup falls back to
//! a search. Adding a harness is one row here; adding a tree is one profile in
//! `tree/` plus one `TREE_PROFILES` entry.

use std::path::Path;

use super::metadata::PackageMetadata;
use super::tree::{WrittenTreeContents, claude, cursor, opencode};
use crate::compiler::agents::HarnessKind;
use crate::compiler::harness_descriptor::{CompilerHarnessDescriptor, descriptor};
use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::sync::target::TargetItem;
use crate::target::ConfigEntry;

pub(crate) type AgentPolicy = fn(
    &str,
    &crate::frontmatter::Frontmatter,
    &TargetItem,
    &mut DiagnosticCollector,
) -> Result<String, MarsError>;

pub(crate) type ConfigWriter = fn(
    &TreeProfile,
    &Path,
    &[ConfigEntry],
    &[&'static PluginHarnessSpec],
    &mut DiagnosticCollector,
) -> Result<(), MarsError>;

pub(crate) type FinishWriter = for<'a> fn(
    &TreeProfile,
    &Path,
    &PackageMetadata,
    &WrittenTreeContents<'a>,
    &mut DiagnosticCollector,
) -> Result<(), MarsError>;

pub(crate) type HookCheck = fn(&crate::compiler::hooks::ParsedHookItem, &mut DiagnosticCollector);

/// A harness naming rule: the shared validator plus the message a rejected name gets.
#[derive(Clone, Copy)]
pub(crate) struct NameRule {
    pub(crate) validate: fn(&str) -> bool,
    pub(crate) message: &'static str,
}

impl NameRule {
    pub(crate) fn accepts(self, name: &str) -> bool {
        (self.validate)(name)
    }
}

/// The root file a tree owns in the package, and the format facts that come with it.
///
/// A marketplace catalog always has a parent directory and a marketplace-name rule;
/// an npm package manifest has neither. Keeping them in the variant means a tree
/// cannot declare one without the other.
#[derive(Clone, Copy)]
pub(crate) enum RootFile {
    Marketplace {
        path: &'static str,
        catalog_directory: &'static str,
        /// Prefix for the catalog's plugin `source` entry (`./` or empty).
        source_prefix: &'static str,
        name_rule: NameRule,
    },
    NpmPackage {
        path: &'static str,
    },
}

impl RootFile {
    pub(crate) fn path(self) -> &'static str {
        match self {
            Self::Marketplace { path, .. } | Self::NpmPackage { path } => path,
        }
    }

    pub(crate) fn catalog_directory(self) -> Option<&'static str> {
        match self {
            Self::Marketplace {
                catalog_directory, ..
            } => Some(catalog_directory),
            Self::NpmPackage { .. } => None,
        }
    }

    /// The rule `[plugin].marketplace` must satisfy, when this tree renders a catalog.
    pub(crate) fn marketplace_name_rule(self) -> Option<NameRule> {
        match self {
            Self::Marketplace { name_rule, .. } => Some(name_rule),
            Self::NpmPackage { .. } => None,
        }
    }

    pub(crate) fn is_marketplace(self) -> bool {
        matches!(self, Self::Marketplace { .. })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum VerificationLevel {
    Live,
    DocsOnly,
}

impl VerificationLevel {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::DocsOnly => "docs-only",
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Verification {
    pub level: VerificationLevel,
    pub detail: &'static str,
}

#[derive(Clone, Copy)]
pub(crate) struct VerificationRow {
    pub install: Verification,
    pub skills: Verification,
    pub agents: Option<Verification>,
    pub hooks: Verification,
    pub mcp: Verification,
}

/// Prose appended to a support cell the renderer computes from tree state.
///
/// The state itself is never prose: it comes from `support_state`, so a note can
/// qualify a computed answer but cannot contradict it.
#[derive(Clone, Copy)]
pub(crate) struct SupportNotes {
    pub skills: Option<&'static str>,
    pub agents: Option<&'static str>,
    pub hooks: Option<&'static str>,
    pub mcp: Option<&'static str>,
}

/// How a tree writes its package-root manifest.
#[derive(Clone, Copy)]
pub(crate) enum Finish {
    /// The standard plugin manifest: name, version, description, author, homepage, license.
    StandardManifest {
        path: &'static str,
        description: &'static str,
    },
    /// Tree-specific root files: the OpenCode npm entry point and plugin data.
    Custom(FinishWriter),
}

impl Finish {
    pub(crate) fn run(
        self,
        profile: &TreeProfile,
        root: &Path,
        metadata: &PackageMetadata,
        contents: &WrittenTreeContents<'_>,
        diag: &mut DiagnosticCollector,
    ) -> Result<(), MarsError> {
        match self {
            Self::StandardManifest { path, description } => {
                super::tree::write_standard_manifest(root, path, description, metadata)
            }
            Self::Custom(writer) => writer(profile, root, metadata, contents, diag),
        }
    }
}

/// One generated plugin tree: its format, its lowering policy, and its writers.
pub(crate) struct TreeProfile {
    /// Staged directory name and the report's tree key.
    pub(crate) key: &'static str,
    pub(crate) harness: HarnessKind,
    pub(crate) root_file: RootFile,
    pub(crate) dropped_agent_fields: &'static [&'static str],
    pub(crate) approximate_lowers_to_dropped: bool,
    pub(crate) agents: Option<AgentPolicy>,
    pub(crate) native_model_aliases: &'static [&'static str],
    pub(crate) plugin_root_token: Option<&'static str>,
    pub(crate) check_hook: HookCheck,
    pub(crate) write_config: ConfigWriter,
    pub(crate) finish: Finish,
}

impl TreeProfile {
    pub(crate) fn descriptor(&self) -> &'static CompilerHarnessDescriptor {
        descriptor(self.harness)
    }
}

pub(crate) struct PluginHarnessSpec {
    pub tree: &'static TreeProfile,
    pub key: &'static str,
    pub name_rule: NameRule,
    pub loads_agents: bool,
    pub display_name: &'static str,
    pub notes: SupportNotes,
    pub verification: VerificationRow,
}

const fn verification(level: VerificationLevel, detail: &'static str) -> Verification {
    Verification { level, detail }
}

const CLAUDE_NAME: NameRule = NameRule {
    validate: valid_claude_plugin_name,
    message: "Claude names cannot contain spaces, `@`, `:`, path separators, control characters, or bidirectional formatting characters",
};
const GROK_NAME: NameRule = NameRule {
    validate: valid_claude_plugin_name,
    message: "Grok reads the Claude tree, whose names cannot contain spaces, `@`, `:`, path separators, control characters, or bidirectional formatting characters",
};
const CODEX_NAME: NameRule = NameRule {
    validate: valid_codex_plugin_name,
    message: "Codex names must be 1-64 lowercase letters, digits, dots, or hyphens, start and end with a letter or digit, and contain neither `--` nor `..`",
};
const CURSOR_NAME: NameRule = NameRule {
    validate: valid_cursor_plugin_name,
    message: "Cursor names must contain only lowercase letters, digits, dots, or hyphens and start and end with a letter or digit",
};
const OPENCODE_NAME: NameRule = NameRule {
    validate: valid_npm_package_name,
    message: "OpenCode names must be valid lowercase npm package names",
};

/// Every generated tree. Adding a tree starts here and in its `tree/` module.
pub(crate) static TREE_PROFILES: &[&TreeProfile] =
    &[&claude::PROFILE, &cursor::PROFILE, &opencode::PROFILE];

pub(crate) static PLUGIN_HARNESS_SPECS: &[PluginHarnessSpec] = &[
    PluginHarnessSpec {
        key: "claude",
        tree: &claude::PROFILE,
        name_rule: CLAUDE_NAME,
        loads_agents: true,
        display_name: "Claude",
        notes: SupportNotes::none(),
        verification: VerificationRow {
            install: verification(VerificationLevel::Live, " (local marketplace)"),
            skills: verification(VerificationLevel::Live, " (own and dependency skills)"),
            agents: Some(verification(VerificationLevel::Live, " (subagent preload)")),
            hooks: verification(
                VerificationLevel::Live,
                " (`${CLAUDE_PLUGIN_ROOT}`, CommonJS script)",
            ),
            mcp: verification(VerificationLevel::Live, " (`env_vars` pass-through)"),
        },
    },
    PluginHarnessSpec {
        key: "codex",
        tree: &claude::PROFILE,
        name_rule: CODEX_NAME,
        loads_agents: false,
        display_name: "Codex",
        notes: SupportNotes {
            hooks: Some("reads the Claude tree's `hooks/hooks.json`"),
            mcp: Some("same file as Claude"),
            ..SupportNotes::none()
        },
        verification: VerificationRow {
            install: verification(VerificationLevel::Live, " (local marketplace)"),
            skills: verification(VerificationLevel::Live, " (own and dependency skills)"),
            agents: None,
            hooks: verification(VerificationLevel::Live, " (built tree, CommonJS script)"),
            mcp: verification(VerificationLevel::Live, " (`env_vars` pass-through)"),
        },
    },
    PluginHarnessSpec {
        key: "grok",
        tree: &claude::PROFILE,
        name_rule: GROK_NAME,
        loads_agents: true,
        display_name: "Grok",
        notes: SupportNotes {
            agents: Some("same file as Claude"),
            hooks: Some("as Claude"),
            mcp: Some("same file as Claude"),
            ..SupportNotes::none()
        },
        verification: VerificationRow {
            install: verification(VerificationLevel::Live, " (local marketplace)"),
            skills: verification(VerificationLevel::Live, ", with `resources/`"),
            agents: Some(verification(
                VerificationLevel::Live,
                " (subagent with native skill preload)",
            )),
            hooks: verification(VerificationLevel::Live, " (`CLAUDE_PLUGIN_ROOT`)"),
            mcp: verification(VerificationLevel::Live, " (`env_vars` pass-through)"),
        },
    },
    PluginHarnessSpec {
        key: "cursor",
        tree: &cursor::PROFILE,
        name_rule: CURSOR_NAME,
        loads_agents: true,
        display_name: "Cursor",
        notes: SupportNotes::none(),
        verification: VerificationRow {
            install: verification(VerificationLevel::DocsOnly, " (account required)"),
            skills: verification(VerificationLevel::DocsOnly, " (schema validation only)"),
            agents: Some(verification(
                VerificationLevel::DocsOnly,
                " (schema validation only)",
            )),
            hooks: verification(VerificationLevel::DocsOnly, " (schema validation only)"),
            mcp: verification(VerificationLevel::DocsOnly, " (schema validation only)"),
        },
    },
    PluginHarnessSpec {
        key: "opencode",
        tree: &opencode::PROFILE,
        name_rule: OPENCODE_NAME,
        loads_agents: false,
        display_name: "OpenCode",
        notes: SupportNotes {
            skills: Some("registered by `index.mjs`"),
            hooks: Some("each fragment needs a default `{id, setup}` export"),
            mcp: Some("registered by `index.mjs`"),
            ..SupportNotes::none()
        },
        verification: VerificationRow {
            install: verification(VerificationLevel::Live, " (`git+file://`)"),
            skills: verification(VerificationLevel::Live, ", with `resources/`"),
            agents: None,
            hooks: verification(VerificationLevel::Live, " (git install, CommonJS script)"),
            mcp: verification(VerificationLevel::Live, " (`<plugin>:<server>`)"),
        },
    },
];

impl SupportNotes {
    const fn none() -> Self {
        Self {
            skills: None,
            agents: None,
            hooks: None,
            mcp: None,
        }
    }
}

pub(crate) fn harness_specs() -> impl Iterator<Item = &'static PluginHarnessSpec> {
    PLUGIN_HARNESS_SPECS.iter()
}

pub(crate) fn harness_spec(key: &str) -> Option<&'static PluginHarnessSpec> {
    harness_specs().find(|spec| spec.key == key)
}

pub(crate) fn trees() -> impl Iterator<Item = &'static TreeProfile> {
    TREE_PROFILES.iter().copied()
}

pub(crate) fn tree_for_root_file(path: &Path) -> Option<&'static TreeProfile> {
    trees().find(|tree| Path::new(tree.root_file.path()) == path)
}

pub(crate) fn unknown_plugin_harness_messages(harnesses: &[String]) -> Vec<String> {
    let valid = harness_specs()
        .map(|spec| spec.key)
        .collect::<Vec<_>>()
        .join(", ");
    harnesses
        .iter()
        .filter(|harness| harness_spec(harness).is_none())
        .map(|harness| {
            format!(
                "mars.toml [plugin].harnesses contains unknown harness `{harness}`; expected one of: {valid}"
            )
        })
        .collect()
}

pub(crate) fn valid_claude_plugin_name(name: &str) -> bool {
    !name.is_empty()
        && !name.chars().any(|character| {
            character.is_whitespace()
                || character.is_control()
                || matches!(character, '@' | ':' | '/' | '\\')
                || matches!(
                    character,
                    '\u{061c}'
                        | '\u{200e}'
                        | '\u{200f}'
                        | '\u{202a}'..='\u{202e}'
                        | '\u{2066}'..='\u{2069}'
                )
        })
}

pub(crate) fn valid_cursor_plugin_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b".-".contains(byte))
}

pub(crate) fn valid_codex_plugin_name(name: &str) -> bool {
    name.len() <= 64
        && !name.contains("--")
        && !name.contains("..")
        && valid_cursor_plugin_name(name)
}

pub(crate) fn valid_npm_package_name(name: &str) -> bool {
    if name.is_empty()
        || name.len() > 214
        || name != name.to_ascii_lowercase()
        || matches!(name, "node_modules" | "favicon.ico")
    {
        return false;
    }
    let segments: Vec<_> = if let Some(scoped) = name.strip_prefix('@') {
        let Some((scope, package)) = scoped.split_once('/') else {
            return false;
        };
        if package.contains('/') {
            return false;
        }
        vec![scope, package]
    } else {
        if name.contains('/') {
            return false;
        }
        vec![name]
    };
    segments.into_iter().all(|segment| {
        !segment.is_empty()
            && !segment.starts_with(['.', '_'])
            && segment.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'-' | b'_' | b'.' | b'~')
            })
    })
}
