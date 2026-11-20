//! The target table: one row per target directory, saying what Mars writes there beyond the
//! skills it copies. `.agents` is the one row with no harness.

use mars_schema::HarnessId;
use mars_schema::item::ItemKind;

use super::keyed_entries::Rendered;
use super::mcp_file::{agy_render, claude_render, cursor_render};
use super::plugin::PluginTarget;
use super::{
    AGENTS_TARGET, KeyedKind, KeyedSection, McpDirs, McpFile, SectionFormat, codex, mcp_file,
    opencode,
};
use crate::types::DestPath;
use std::path::Path;

/// What `${MARS_MCP_DIR}` becomes in a server's `args`.
#[derive(Debug, Clone, Copy)]
pub enum McpArgsDir {
    /// The project path of the installed copy.
    Installed,
    /// `${PLUGIN_ROOT}/<native>`: `agy` substitutes it in `args` and `env`; it was not shown to
    /// in `command`, which gets the installed path as every harness does.
    PluginRoot,
}

#[derive(Debug)]
pub struct TargetSpec {
    /// The harness this target is the native directory of; `None` for the full-fidelity
    /// `.agents` target.
    pub harness: Option<HarnessId>,
    /// The file this harness reads MCP servers from and how entries render there; `None` when
    /// the target takes no MCP servers.
    pub mcp: Option<McpFile>,
    pub mcp_args: McpArgsDir,
    /// The files of a target that is a plugin directory the harness loads through a pointer.
    pub plugin: Option<PluginTarget>,
    /// Whether the harness also reads `.agents/skills/`, so linking `.agents` beside this target
    /// puts every skill in front of it twice.
    pub reads_agents_skills: bool,
}

/// The row for a target directory name (`.claude`, `.agents`), or `None` for any other name,
/// which callers treat as a plain copy target.
pub fn spec(name: &str) -> Option<&'static TargetSpec> {
    if name == AGENTS_TARGET {
        return Some(&AGENTS);
    }
    crate::harness::registry::harness_for_target(name).map(for_harness)
}

pub fn for_harness(id: HarnessId) -> &'static TargetSpec {
    match id {
        HarnessId::Claude => &CLAUDE,
        HarnessId::Codex => &CODEX,
        HarnessId::Pi => &PI,
        HarnessId::OpenCode => &OPENCODE,
        HarnessId::Cursor => &CURSOR,
        HarnessId::Agy => &AGY,
    }
}

impl TargetSpec {
    /// The target directory name (`.claude`).
    pub fn name(&self) -> String {
        self.harness
            .map_or_else(|| AGENTS_TARGET.to_string(), HarnessId::target_dir)
    }

    /// The `variants/<key>/` skill directory this target consumes: the harness name. `.agents`
    /// selects no variants.
    pub fn skill_variant_key(&self) -> Option<&'static str> {
        self.harness.map(HarnessId::as_str)
    }

    /// Returns `None` if this target does not accept the item kind. The compiler MUST skip
    /// items for which this returns `None`.
    pub fn default_dest_path(&self, kind: ItemKind, name: &str) -> Option<DestPath> {
        let path = match (self.harness, kind) {
            (_, ItemKind::Skill) => format!("skills/{name}"),
            (None, ItemKind::Agent) => format!("agents/{name}.md"),
            (None, ItemKind::BootstrapDoc) => format!("bootstrap/{name}/BOOTSTRAP.md"),
            // Hooks and MCP servers install as target-scoped package directories, whose paths
            // `PackageDir` owns.
            _ => return None,
        };
        Some(DestPath::from(path.as_str()))
    }

    /// What `${MARS_MCP_DIR}` becomes for a server installed at `native` (relative to the
    /// target root) or `installed` (the project path of that copy).
    pub fn mcp_dirs(&self, native: &str, installed: &str) -> McpDirs {
        match self.mcp_args {
            McpArgsDir::Installed => McpDirs::same(installed),
            McpArgsDir::PluginRoot => McpDirs {
                command: installed.to_owned(),
                args: format!("${{PLUGIN_ROOT}}/{native}"),
            },
        }
    }

    /// The keyed-entry sections Mars owns entries in for this target: one per [`KeyedKind`] the
    /// target declares. Reconcile and unlink drive from this.
    pub fn keyed_sections(&self) -> Vec<(KeyedKind, KeyedSection)> {
        self.mcp
            .map(|file| (mcp_file::MCP_KIND, file.at))
            .into_iter()
            .chain(self.plugin.into_iter().flat_map(|plugin| plugin.sections()))
            .collect()
    }

    /// Entries the target has whenever it is managed, whatever packages install.
    pub fn fixed_entries(&self, project_root: &Path) -> Vec<(KeyedKind, Rendered)> {
        self.plugin
            .map(|plugin| plugin.fixed_entries(&self.name(), project_root))
            .unwrap_or_default()
    }
}

const fn json_mcp(
    path: &'static str,
    render: fn(&super::McpServerEntry) -> mars_schema::mcp_servers::McpRender,
) -> McpFile {
    McpFile {
        at: KeyedSection {
            path,
            format: SectionFormat::JsonSection("mcpServers"),
        },
        render,
    }
}

// Legacy `.agents` outputs retain full agent frontmatter.
const AGENTS: TargetSpec = TargetSpec {
    harness: None,
    mcp: None,
    mcp_args: McpArgsDir::Installed,
    plugin: None,
    reads_agents_skills: false,
};

const CLAUDE: TargetSpec = TargetSpec {
    harness: Some(HarnessId::Claude),
    // The project-root file: Claude reads `<project>/.mcp.json`, not `.claude/.mcp.json`.
    // Claude asks the user to approve project servers; mars does not.
    mcp: Some(json_mcp(".mcp.json", claude_render)),
    mcp_args: McpArgsDir::Installed,
    plugin: None,
    reads_agents_skills: false,
};

const CODEX: TargetSpec = TargetSpec {
    harness: Some(HarnessId::Codex),
    // Codex reads `[mcp_servers.<name>]` from the project's `.codex/config.toml`, for trusted
    // projects only.
    mcp: Some(McpFile {
        at: KeyedSection {
            path: ".codex/config.toml",
            format: SectionFormat::TomlTable("mcp_servers"),
        },
        render: codex::render,
    }),
    mcp_args: McpArgsDir::Installed,
    plugin: None,
    reads_agents_skills: false,
};

const OPENCODE: TargetSpec = TargetSpec {
    harness: Some(HarnessId::OpenCode),
    mcp: Some(McpFile {
        at: KeyedSection {
            path: ".opencode/opencode.json",
            format: SectionFormat::JsonSection("mcp"),
        },
        render: opencode::render,
    }),
    mcp_args: McpArgsDir::Installed,
    plugin: None,
    reads_agents_skills: false,
};

// Pi reads project MCP servers only from trusted projects.
const PI: TargetSpec = TargetSpec {
    harness: Some(HarnessId::Pi),
    mcp: Some(json_mcp(".pi/mcp.json", claude_render)),
    mcp_args: McpArgsDir::Installed,
    plugin: None,
    reads_agents_skills: false,
};

const CURSOR: TargetSpec = TargetSpec {
    harness: Some(HarnessId::Cursor),
    mcp: Some(json_mcp(".cursor/mcp.json", cursor_render)),
    mcp_args: McpArgsDir::Installed,
    plugin: None,
    reads_agents_skills: false,
};

/// `agy` reads nothing from `.agy/` itself: the project's `.agents/plugins.json` points at it as
/// a plugin, which carries the skills, agents, hooks and MCP servers.
const AGY: TargetSpec = TargetSpec {
    harness: Some(HarnessId::Agy),
    mcp: Some(json_mcp(".agy/mcp_config.json", agy_render)),
    mcp_args: McpArgsDir::PluginRoot,
    plugin: Some(PluginTarget {
        pointer: KeyedSection {
            path: ".agents/plugins.json",
            format: SectionFormat::JsonArrayByField {
                section: "entries",
                field: "path",
            },
        },
        manifest: KeyedSection {
            path: ".agy/plugin.json",
            format: SectionFormat::JsonTopLevel,
        },
        hooks: KeyedSection {
            path: ".agy/hooks.json",
            format: SectionFormat::JsonTopLevel,
        },
    }),
    // `agy` loads `.agents/skills/` too, so a skill in both is in front of it twice.
    reads_agents_skills: true,
};
