//! Declarative harness and generated-tree plugin format specifications.

use serde::{Deserialize, Serialize};

/// Harnesses that can consume a generated plugin artifact.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum PluginHarness {
    Claude,
    Codex,
    Grok,
    Cursor,
    OpenCode,
    #[serde(other)]
    Unknown,
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
    #[allow(dead_code)]
    InstallOnly,
    DocsOnly,
}

#[cfg(test)]
impl VerificationLevel {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::InstallOnly => "install-only",
            Self::DocsOnly => "docs-only",
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Verification {
    pub level: VerificationLevel,
    #[allow(dead_code)]
    pub detail: &'static str,
}

#[derive(Clone, Copy)]
pub(crate) struct VerificationRow {
    #[allow(dead_code)]
    pub install: Verification,
    pub skills: Verification,
    pub agents: Option<Verification>,
    pub hooks: Verification,
    pub mcp: Verification,
    #[allow(dead_code)]
    pub evidence: &'static str,
}

pub(crate) struct PluginHarnessSpec {
    pub harness: PluginHarness,
    pub tree: PluginTree,
    pub key: &'static str,
    pub name_rule: NameRule,
    pub loads_agents: bool,
    pub mcp_callable: bool,
    pub display_name: &'static str,
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
        mcp_callable: true,
        display_name: "Claude",
        verification: VerificationRow {
            install: verification(VerificationLevel::Live, ""),
            skills: verification(VerificationLevel::Live, ""),
            agents: Some(verification(VerificationLevel::Live, " (subagent preload)")),
            hooks: verification(VerificationLevel::Live, " (`${CLAUDE_PLUGIN_ROOT}`)"),
            mcp: verification(VerificationLevel::Live, ""),
            evidence: "p9291, p9305, p9329, p9478",
        },
    },
    PluginHarnessSpec {
        harness: PluginHarness::Codex,
        key: "codex",
        tree: PluginTree::Claude,
        name_rule: CODEX_NAME,
        loads_agents: false,
        mcp_callable: false,
        display_name: "Codex",
        verification: VerificationRow {
            install: verification(VerificationLevel::Live, ""),
            skills: verification(VerificationLevel::Live, ""),
            agents: None,
            hooks: verification(
                VerificationLevel::Live,
                " on a hand-built Claude-format tree (p9291); V re-runs it on the built tree",
            ),
            mcp: verification(VerificationLevel::Live, ": listed, not callable"),
            evidence: "p9291, p9478",
        },
    },
    PluginHarnessSpec {
        harness: PluginHarness::Grok,
        key: "grok",
        tree: PluginTree::Claude,
        name_rule: GROK_NAME,
        loads_agents: true,
        mcp_callable: true,
        display_name: "Grok",
        verification: VerificationRow {
            install: verification(VerificationLevel::Live, ""),
            skills: verification(VerificationLevel::Live, ", with `resources/` (p9479)"),
            agents: Some(verification(
                VerificationLevel::Live,
                ": subagent, native preload, skills line (p9479)",
            )),
            hooks: verification(
                VerificationLevel::Live,
                ": `SessionEnd`, `SubagentStop`, `CLAUDE_PLUGIN_ROOT` (p9479)",
            ),
            mcp: verification(VerificationLevel::DocsOnly, " (no server in any fixture)"),
            evidence: "p9473, p9478, p9479",
        },
    },
    PluginHarnessSpec {
        harness: PluginHarness::Cursor,
        key: "cursor",
        tree: PluginTree::Cursor,
        name_rule: CURSOR_NAME,
        loads_agents: true,
        mcp_callable: true,
        display_name: "Cursor",
        verification: VerificationRow {
            install: verification(VerificationLevel::DocsOnly, ""),
            skills: verification(VerificationLevel::DocsOnly, ""),
            agents: Some(verification(VerificationLevel::DocsOnly, "")),
            hooks: verification(VerificationLevel::DocsOnly, ""),
            mcp: verification(VerificationLevel::DocsOnly, ""),
            evidence: "p9474",
        },
    },
    PluginHarnessSpec {
        harness: PluginHarness::OpenCode,
        key: "opencode",
        tree: PluginTree::OpenCode,
        name_rule: OPENCODE_NAME,
        loads_agents: false,
        mcp_callable: true,
        display_name: "OpenCode",
        verification: VerificationRow {
            install: verification(VerificationLevel::Live, " (git)"),
            skills: verification(VerificationLevel::Live, ", with `resources/`"),
            agents: None,
            hooks: verification(
                VerificationLevel::Live,
                " (composed fragment from a local path, p9477)",
            ),
            mcp: verification(VerificationLevel::Live, " (`<plugin>:<server>`, p9477)"),
            evidence: "p9475, p9477",
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
    pub(crate) fn spec(self) -> Option<&'static PluginHarnessSpec> {
        PLUGIN_HARNESS_SPECS
            .iter()
            .find(|spec| spec.harness == self)
    }

    pub(crate) fn specs() -> impl Iterator<Item = &'static PluginHarnessSpec> {
        PLUGIN_HARNESS_SPECS.iter()
    }

    pub fn as_str(self) -> &'static str {
        self.spec().map_or("unknown", |spec| spec.key)
    }

    pub(crate) fn tree(self) -> PluginTree {
        self.spec()
            .expect("plugin build validation rejects unknown harnesses")
            .tree
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
