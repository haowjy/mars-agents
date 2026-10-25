//! Declarative harness and generated-tree plugin format specifications.

use serde::{Deserialize, Serialize};

/// Harnesses that can consume a generated plugin artifact.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PluginHarness {
    Claude,
    Codex,
    Grok,
    Cursor,
    OpenCode,
    Unknown(String),
}

/// A distinct generated plugin tree format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginTree {
    Claude,
    Cursor,
    OpenCode,
}

#[derive(Clone, Copy)]
pub(crate) struct NameRule {
    validate: fn(&str) -> bool,
    pub message: &'static str,
}

impl NameRule {
    pub(crate) fn accepts(self, name: &str) -> bool {
        (self.validate)(name)
    }
}

#[derive(Clone, Copy)]
pub(crate) enum RootFileRenderer {
    Marketplace { source_prefix: &'static str },
    OpenCodePackage,
}

#[derive(Clone, Copy)]
pub(crate) struct RootFileSpec {
    pub path: &'static str,
    pub catalog_directory: Option<&'static str>,
    pub renderer: RootFileRenderer,
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
    pub evidence: &'static str,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct PluginSupportCells {
    pub skills: Option<&'static str>,
    pub agents: Option<&'static str>,
    pub hooks: Option<&'static str>,
    pub mcp: Option<&'static str>,
}

pub(crate) struct PluginHarnessSpec {
    pub harness: PluginHarness,
    pub tree: PluginTree,
    pub key: &'static str,
    pub name_rule: NameRule,
    pub loads_agents: bool,
    pub display_name: &'static str,
    pub support: PluginSupportCells,
    pub verification: VerificationRow,
}

pub(crate) struct PluginTreeSpec {
    pub tree: PluginTree,
    pub key: &'static str,
    pub root_file: Option<RootFileSpec>,
    pub marketplace_name_rule: Option<NameRule>,
    pub dropped_agent_fields: &'static [&'static str],
    pub approximate_lowers_to_dropped: bool,
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

pub(crate) static PLUGIN_HARNESS_SPECS: &[PluginHarnessSpec] = &[
    PluginHarnessSpec {
        harness: PluginHarness::Claude,
        key: "claude",
        tree: PluginTree::Claude,
        name_rule: CLAUDE_NAME,
        loads_agents: true,
        display_name: "Claude",
        support: PluginSupportCells {
            skills: None,
            agents: None,
            hooks: None,
            mcp: None,
        },
        verification: VerificationRow {
            install: verification(VerificationLevel::Live, " (local marketplace)"),
            skills: verification(VerificationLevel::Live, " (own and dependency skills)"),
            agents: Some(verification(VerificationLevel::Live, " (subagent preload)")),
            hooks: verification(
                VerificationLevel::Live,
                " (`${CLAUDE_PLUGIN_ROOT}`, CommonJS script)",
            ),
            mcp: verification(VerificationLevel::Live, " (`env_vars` pass-through)"),
            evidence: "p9533, p9535",
        },
    },
    PluginHarnessSpec {
        harness: PluginHarness::Codex,
        key: "codex",
        tree: PluginTree::Claude,
        name_rule: CODEX_NAME,
        loads_agents: false,
        display_name: "Codex",
        support: PluginSupportCells {
            skills: None,
            agents: Some("**impossible**: Codex plugins don't load agents"),
            hooks: Some("as Claude (reads the Claude tree's `hooks/hooks.json`)"),
            mcp: Some("supported (same file as Claude)"),
        },
        verification: VerificationRow {
            install: verification(VerificationLevel::Live, " (local marketplace)"),
            skills: verification(VerificationLevel::Live, " (own and dependency skills)"),
            agents: None,
            hooks: verification(VerificationLevel::Live, " (built tree, CommonJS script)"),
            mcp: verification(VerificationLevel::Live, " (`env_vars` pass-through)"),
            evidence: "p9533, p9535",
        },
    },
    PluginHarnessSpec {
        harness: PluginHarness::Grok,
        key: "grok",
        tree: PluginTree::Claude,
        name_rule: GROK_NAME,
        loads_agents: true,
        display_name: "Grok",
        support: PluginSupportCells {
            skills: None,
            agents: Some("supported (same file as Claude)"),
            hooks: Some("as Claude"),
            mcp: Some("supported (same file as Claude)"),
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
            evidence: "p9473, p9478, p9533, p9535",
        },
    },
    PluginHarnessSpec {
        harness: PluginHarness::Cursor,
        key: "cursor",
        tree: PluginTree::Cursor,
        name_rule: CURSOR_NAME,
        loads_agents: true,
        display_name: "Cursor",
        support: PluginSupportCells {
            skills: None,
            agents: None,
            hooks: None,
            mcp: None,
        },
        verification: VerificationRow {
            install: verification(VerificationLevel::DocsOnly, " (account required)"),
            skills: verification(VerificationLevel::DocsOnly, " (schema validation only)"),
            agents: Some(verification(
                VerificationLevel::DocsOnly,
                " (schema validation only)",
            )),
            hooks: verification(VerificationLevel::DocsOnly, " (schema validation only)"),
            mcp: verification(VerificationLevel::DocsOnly, " (schema validation only)"),
            evidence: "p9533",
        },
    },
    PluginHarnessSpec {
        harness: PluginHarness::OpenCode,
        key: "opencode",
        tree: PluginTree::OpenCode,
        name_rule: OPENCODE_NAME,
        loads_agents: false,
        display_name: "OpenCode",
        support: PluginSupportCells {
            skills: Some("supported (registered by `index.mjs`)"),
            agents: Some("**impossible**: no API adds an agent"),
            hooks: Some(
                "supported for hooks with an `opencode` fragment that has a default `{id, setup}` export; others dropped with warning",
            ),
            mcp: Some("supported (registered by `index.mjs`)"),
        },
        verification: VerificationRow {
            install: verification(VerificationLevel::Live, " (`git+file://`)"),
            skills: verification(VerificationLevel::Live, ", with `resources/`"),
            agents: None,
            hooks: verification(VerificationLevel::Live, " (git install, CommonJS script)"),
            mcp: verification(VerificationLevel::Live, " (`<plugin>:<server>`)"),
            evidence: "p9533",
        },
    },
];

pub(crate) static CLAUDE_TREE_SPEC: PluginTreeSpec = PluginTreeSpec {
    tree: PluginTree::Claude,
    key: "claude",
    root_file: Some(RootFileSpec {
        path: ".claude-plugin/marketplace.json",
        catalog_directory: Some(".claude-plugin"),
        renderer: RootFileRenderer::Marketplace {
            source_prefix: "./",
        },
    }),
    marketplace_name_rule: Some(NameRule {
        validate: valid_claude_plugin_name,
        message: "Claude marketplace names cannot contain spaces, `@`, `:`, path separators, control characters, or bidirectional formatting characters",
    }),
    dropped_agent_fields: &["hooks", "mcpServers", "permissionMode", "initialPrompt"],
    approximate_lowers_to_dropped: false,
};

pub(crate) static CURSOR_TREE_SPEC: PluginTreeSpec = PluginTreeSpec {
    tree: PluginTree::Cursor,
    key: "cursor",
    root_file: Some(RootFileSpec {
        path: ".cursor-plugin/marketplace.json",
        catalog_directory: Some(".cursor-plugin"),
        renderer: RootFileRenderer::Marketplace { source_prefix: "" },
    }),
    marketplace_name_rule: Some(NameRule {
        validate: valid_cursor_plugin_name,
        message: "Cursor marketplace names must contain only lowercase letters, digits, dots, or hyphens and start and end with a letter or digit",
    }),
    dropped_agent_fields: &[],
    approximate_lowers_to_dropped: true,
};

pub(crate) static OPENCODE_TREE_SPEC: PluginTreeSpec = PluginTreeSpec {
    tree: PluginTree::OpenCode,
    key: "opencode",
    root_file: Some(RootFileSpec {
        path: "package.json",
        catalog_directory: None,
        renderer: RootFileRenderer::OpenCodePackage,
    }),
    marketplace_name_rule: None,
    dropped_agent_fields: &[],
    approximate_lowers_to_dropped: false,
};

pub(crate) static PLUGIN_TREE_SPECS: &[&PluginTreeSpec] =
    &[&CLAUDE_TREE_SPEC, &CURSOR_TREE_SPEC, &OPENCODE_TREE_SPEC];

impl PluginHarness {
    pub(crate) fn spec(&self) -> Option<&'static PluginHarnessSpec> {
        PLUGIN_HARNESS_SPECS
            .iter()
            .find(|spec| &spec.harness == self)
    }

    pub(crate) fn specs() -> impl Iterator<Item = &'static PluginHarnessSpec> {
        PLUGIN_HARNESS_SPECS.iter()
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Unknown(value) => value,
            _ => self.spec().expect("known plugin harness has a spec").key,
        }
    }

    pub(crate) fn tree(&self) -> PluginTree {
        self.spec()
            .expect("plugin build validation rejects unknown harnesses")
            .tree
    }
}

pub(crate) fn unknown_plugin_harness_messages(harnesses: &[PluginHarness]) -> Vec<String> {
    let valid = PluginHarness::specs()
        .map(|spec| spec.key)
        .collect::<Vec<_>>()
        .join(", ");
    harnesses
        .iter()
        .filter(|harness| harness.spec().is_none())
        .map(|harness| {
            format!(
                "mars.toml [plugin].harnesses contains unknown harness `{}`; expected one of: {valid}",
                harness.as_str()
            )
        })
        .collect()
}

impl Serialize for PluginHarness {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for PluginHarness {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(match value.as_str() {
            "claude" => Self::Claude,
            "codex" => Self::Codex,
            "grok" => Self::Grok,
            "cursor" => Self::Cursor,
            "opencode" => Self::OpenCode,
            _ => Self::Unknown(value),
        })
    }
}

impl PluginTree {
    pub(crate) fn spec(self) -> &'static PluginTreeSpec {
        PLUGIN_TREE_SPECS
            .iter()
            .copied()
            .find(|spec| spec.tree == self)
            .expect("every plugin tree has a format spec")
    }

    pub fn as_str(self) -> &'static str {
        self.spec().key
    }
}

fn valid_claude_plugin_name(name: &str) -> bool {
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

fn valid_cursor_plugin_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b".-".contains(byte))
}

fn valid_codex_plugin_name(name: &str) -> bool {
    name.len() <= 64
        && !name.contains("--")
        && !name.contains("..")
        && valid_cursor_plugin_name(name)
}

fn valid_npm_package_name(name: &str) -> bool {
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
