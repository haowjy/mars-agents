//! Declarative plugin harness and generated-tree specifications.
//!
//! One row per consumer harness (tree, naming, display, README note, verification
//! evidence) and one row per generated tree (`TREE_PROFILES`, defined beside the
//! writer that emits it). Harness rows point at their tree row directly, so a
//! harness and its tree cannot disagree and no lookup falls back to a search.
//! Adding a harness is one row here; adding a tree is one profile in `tree/` plus
//! one `TREE_PROFILES` entry.
//!
//! The rows also carry the published documentation data — verification detail,
//! install evidence, support notes — that only the README freshness test reads, so
//! the shipped build marks exactly those items as expected-dead.

use std::path::Path;

use super::metadata::PackageMetadata;
use super::tree::{claude, cursor, opencode};
use crate::compiler::harness_descriptor::{CompilerHarnessDescriptor, descriptor};
use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::sync::target::TargetItem;
use crate::target::ConfigEntry;
use mars_schema::HarnessId;
use mars_schema::plugin_metadata::GeneratedRootFile;

pub(super) type AgentPolicy = fn(
    &str,
    &mars_schema::frontmatter::Frontmatter,
    &TargetItem,
    &mut DiagnosticCollector,
) -> Result<String, MarsError>;

pub(super) type ConfigWriter = fn(
    &TreeProfile,
    &Path,
    &[ConfigEntry],
    &[&'static PluginHarnessSpec],
    &mut DiagnosticCollector,
) -> Result<(), MarsError>;

pub(super) type FinishWriter = for<'a> fn(
    &TreeProfile,
    &Path,
    &PackageMetadata,
    &super::tree::WrittenTreeContents<'a>,
    &mut DiagnosticCollector,
) -> Result<(), MarsError>;

pub(super) type HookCheck = fn(&crate::compiler::hooks::ParsedHookItem, &mut DiagnosticCollector);

/// A harness naming rule: the shared validator plus the message a rejected name gets.
#[derive(Clone, Copy)]
pub(super) struct NameRule {
    pub(super) validate: fn(&str) -> bool,
    pub(super) message: &'static str,
}

impl NameRule {
    pub(super) fn accepts(self, name: &str) -> bool {
        (self.validate)(name)
    }
}

/// The root file a tree owns in the package, and the format facts that come with it.
///
/// A marketplace catalog has a marketplace-name rule and a source prefix, and its
/// catalog directory comes from the `GeneratedRootFile` table; an npm package manifest
/// has neither. Keeping them in the variant means a tree cannot declare one without the
/// other, and the file itself names a `mars_schema::plugin_metadata::GENERATED_ROOT_FILES`
/// constant, which the test below checks covers every tree, so staging and sentinel
/// validation know each generated root file.
#[derive(Clone, Copy)]
pub(super) enum RootFile {
    Marketplace {
        file: GeneratedRootFile,
        /// Prefix for the catalog's plugin `source` entry (`./` or empty).
        source_prefix: &'static str,
        name_rule: NameRule,
    },
    NpmPackage {
        file: GeneratedRootFile,
    },
}

impl RootFile {
    pub(super) fn path(self) -> &'static str {
        match self {
            Self::Marketplace { file, .. } | Self::NpmPackage { file } => file.path(),
        }
    }

    pub(super) fn catalog_directory(self) -> Option<&'static str> {
        match self {
            Self::Marketplace { file, .. } | Self::NpmPackage { file } => file.catalog_directory(),
        }
    }

    /// The rule `[plugin].marketplace` must satisfy, when this tree renders a catalog.
    pub(super) fn marketplace_name_rule(self) -> Option<NameRule> {
        match self {
            Self::Marketplace { name_rule, .. } => Some(name_rule),
            Self::NpmPackage { .. } => None,
        }
    }

    pub(super) fn is_marketplace(self) -> bool {
        matches!(self, Self::Marketplace { .. })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum VerificationLevel {
    Live,
    DocsOnly,
}

impl VerificationLevel {
    #[cfg_attr(not(test), expect(dead_code, reason = "README data"))]
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::DocsOnly => "docs-only",
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Verification {
    pub level: VerificationLevel,
    #[cfg_attr(not(test), expect(dead_code, reason = "README data"))]
    pub detail: &'static str,
}

#[derive(Clone, Copy)]
pub(super) struct VerificationRow {
    #[cfg_attr(not(test), expect(dead_code, reason = "README data"))]
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
pub(super) struct SupportNotes {
    #[cfg_attr(not(test), expect(dead_code, reason = "README data"))]
    pub skills: Option<&'static str>,
    #[cfg_attr(not(test), expect(dead_code, reason = "README data"))]
    pub agents: Option<&'static str>,
    #[cfg_attr(not(test), expect(dead_code, reason = "README data"))]
    pub hooks: Option<&'static str>,
    #[cfg_attr(not(test), expect(dead_code, reason = "README data"))]
    pub mcp: Option<&'static str>,
}

/// How a tree writes its package-root manifest.
#[derive(Clone, Copy)]
pub(super) enum Finish {
    /// The standard plugin manifest: name, version, description, author, homepage, license.
    StandardManifest {
        path: &'static str,
        description: &'static str,
    },
    /// Tree-specific root files: the OpenCode npm entry point and plugin data.
    Custom(FinishWriter),
}

/// How a tree's generated files refer to the tree's own package root.
#[derive(Clone, Copy)]
pub(super) enum PluginRoot {
    /// The harness substitutes this token at load time; the tree writes it literally.
    Token(&'static str),
    /// The tree's own runtime code resolves its root at load time, so the Mars
    /// token stays in the generated file for that runtime to substitute.
    RuntimeResolved,
}

impl PluginRoot {
    /// `relative`, a package-root-relative path that an authored file names with
    /// `mars_token`, as this tree's generated files write it.
    pub(super) fn resolve(self, mars_token: &str, relative: &str) -> String {
        match self {
            Self::Token(token) => format!("{token}/{relative}"),
            Self::RuntimeResolved => mars_token.to_string(),
        }
    }
}

/// One generated plugin tree: its format, its lowering policy, and its writers.
pub(super) struct TreeProfile {
    /// Staged directory name and the report's tree key.
    pub(super) key: &'static str,
    pub(super) harness: HarnessId,
    pub(super) root_file: RootFile,
    pub(super) dropped_agent_fields: &'static [&'static str],
    pub(super) approximate_lowers_to_dropped: bool,
    pub(super) agents: Option<AgentPolicy>,
    pub(super) native_model_aliases: &'static [&'static str],
    pub(super) plugin_root: PluginRoot,
    pub(super) check_hook: HookCheck,
    pub(super) write_config: ConfigWriter,
    pub(super) finish: Finish,
}

impl TreeProfile {
    pub(super) fn descriptor(&self) -> &'static CompilerHarnessDescriptor {
        descriptor(self.harness)
    }
}

pub(crate) struct PluginHarnessSpec {
    pub(super) tree: &'static TreeProfile,
    pub(crate) key: &'static str,
    pub(super) name_rule: NameRule,
    pub(super) display_name: &'static str,
    #[cfg_attr(not(test), expect(dead_code, reason = "README data"))]
    pub(super) notes: SupportNotes,
    /// Agent evidence is also the `loads agents` fact: a harness without the
    /// surface has no evidence, and `support_state` reports it impossible.
    pub(super) verification: VerificationRow,
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
pub(super) static TREE_PROFILES: &[&TreeProfile] =
    &[&claude::PROFILE, &cursor::PROFILE, &opencode::PROFILE];

pub(super) static PLUGIN_HARNESS_SPECS: &[PluginHarnessSpec] = &[
    PluginHarnessSpec {
        key: "claude",
        tree: &claude::PROFILE,
        name_rule: CLAUDE_NAME,
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
        display_name: "Grok",
        notes: SupportNotes {
            agents: Some("same file as Claude"),
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

pub(super) fn harness_specs() -> impl Iterator<Item = &'static PluginHarnessSpec> {
    PLUGIN_HARNESS_SPECS.iter()
}

pub(super) fn harness_spec(key: &str) -> Option<&'static PluginHarnessSpec> {
    harness_specs().find(|spec| spec.key == key)
}

pub(super) fn trees() -> impl Iterator<Item = &'static TreeProfile> {
    TREE_PROFILES.iter().copied()
}

pub(super) fn tree_for_root_file(path: &Path) -> Option<&'static TreeProfile> {
    trees().find(|tree| Path::new(tree.root_file.path()) == path)
}

pub(super) fn unknown_plugin_harness_messages(harnesses: &[String]) -> Vec<String> {
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

pub(super) fn valid_claude_plugin_name(name: &str) -> bool {
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

pub(super) fn valid_cursor_plugin_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b".-".contains(byte))
}

pub(super) fn valid_codex_plugin_name(name: &str) -> bool {
    name.len() <= 64
        && !name.contains("--")
        && !name.contains("..")
        && valid_cursor_plugin_name(name)
}

pub(super) fn valid_npm_package_name(name: &str) -> bool {
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

#[cfg(test)]
mod tests {
    use mars_schema::plugin_metadata::GENERATED_ROOT_FILES;

    use super::trees;

    /// Staging excludes and sentinel validation accept only `GENERATED_ROOT_FILES`,
    /// so a tree whose root file is missing from it, or whose marketplace variant
    /// needs a catalog directory the table does not declare, would fail every
    /// configured build at the sentinel check.
    #[test]
    fn tree_root_files_match_the_generated_root_file_table() {
        for tree in trees() {
            let path = tree.root_file.path();
            let declared = GENERATED_ROOT_FILES
                .iter()
                .find(|file| file.path() == path)
                .unwrap_or_else(|| {
                    panic!(
                        "tree `{}` generates `{path}`, which `GENERATED_ROOT_FILES` does not declare",
                        tree.key
                    )
                });
            assert_eq!(
                declared.catalog_directory().is_some(),
                tree.root_file.is_marketplace(),
                "tree `{}` generates `{path}`, whose `GENERATED_ROOT_FILES` entry {} a catalog directory",
                tree.key,
                if tree.root_file.is_marketplace() {
                    "has no"
                } else {
                    "has"
                }
            );
        }
    }
}
