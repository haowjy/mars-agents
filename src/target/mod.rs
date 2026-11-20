pub mod agents;
pub mod agy;
pub mod claude;
pub mod codex;
mod config_doc;
pub mod cursor;
pub(crate) mod hook_file;
pub(crate) mod keyed_entries;
pub mod mcp_file;
pub mod opencode;
pub mod pi;
pub(crate) mod plugin;

use crate::harness::registry;
use crate::types::DestPath;
pub use config_doc::SectionFormat;
use keyed_entries::Rendered;
pub use keyed_entries::{KeyedKind, KeyedSection};
use mars_schema::HarnessId;
use mars_schema::item::ItemKind;
pub use mcp_file::McpFile;
use plugin::PluginTarget;
use std::path::Path;

pub(crate) const AGENTS_TARGET: &str = ".agents";

const WINDOWS_INVALID_CHARS: &[char] = &[':', '*', '?', '<', '>', '|', '"', '/', '\\'];

pub(crate) const HOOK_KEY_PREFIX: &str = "hook:";

/// What `${MARS_MCP_DIR}` becomes in a server's `command` and in its `args`, which a harness may
/// resolve differently.
#[derive(Debug, Clone)]
pub struct McpDirs {
    pub command: String,
    pub args: String,
}

impl McpDirs {
    pub fn same(dir: impl Into<String>) -> Self {
        let dir = dir.into();
        Self {
            command: dir.clone(),
            args: dir,
        }
    }
}

/// Env values are variable names (symbolic). Adapters translate them to the
/// target's interpolation syntax (e.g. `${VAR}` for Claude, plain name for Codex).
#[derive(Debug, Clone)]
pub struct McpServerEntry {
    pub name: String,
    pub transport: mars_schema::mcp_server::McpTransport,
    /// The source the server came from; `_self` for the project's own.
    pub source: String,
}

impl McpServerEntry {
    pub(crate) fn is_own(&self) -> bool {
        self.source == crate::types::SourceOrigin::LocalPackage.to_string()
    }
}

#[derive(Debug, Clone)]
pub struct HookEntry {
    pub name: String,
    pub native_event: String,
    pub entries: Vec<serde_json::Value>,
}

impl HookEntry {
    pub fn key(&self) -> String {
        format!("{HOOK_KEY_PREFIX}{}:{}", self.native_event, self.name)
    }
}

pub trait TargetAdapter: std::fmt::Debug + Send + Sync {
    /// The harness this target is the native directory of; `None` for the
    /// full-fidelity `.agents` target.
    fn harness(&self) -> Option<HarnessId>;

    /// The target directory name (`.claude`).
    fn name(&self) -> &str {
        self.harness()
            .map_or(AGENTS_TARGET, |id| registry::descriptor(id).default_target)
    }

    /// The `variants/<key>/` skill directory this target consumes: the
    /// harness name. `.agents` selects no variants.
    fn skill_variant_key(&self) -> Option<&str> {
        self.harness().map(HarnessId::as_str)
    }

    /// Returns `None` if this target does not accept the item kind. The
    /// compiler MUST skip items for which this returns `None`.
    fn default_dest_path(&self, kind: ItemKind, name: &str) -> Option<DestPath>;

    /// The file this harness reads MCP servers from and how entries render
    /// there; `None` when the target takes no MCP servers.
    fn mcp_file(&self) -> Option<McpFile> {
        None
    }

    /// What `${MARS_MCP_DIR}` becomes for a server installed at `native` (relative to the
    /// target root) or `installed` (the project path of that copy).
    fn mcp_dirs(&self, _native: &str, installed: &str) -> McpDirs {
        McpDirs::same(installed)
    }

    /// The files of a target that is a plugin directory the harness loads through a pointer.
    fn plugin(&self) -> Option<PluginTarget> {
        None
    }

    /// Whether the harness also reads `.agents/skills/`, so linking `.agents` beside this
    /// target puts every skill in front of it twice.
    fn reads_agents_skills(&self) -> bool {
        false
    }

    /// The keyed-entry sections Mars owns entries in for this target: one per
    /// [`KeyedKind`] the target declares. Reconcile and unlink drive from this.
    fn keyed_sections(&self) -> Vec<(KeyedKind, KeyedSection)> {
        self.mcp_file()
            .map(|file| (mcp_file::MCP_KIND, file.at))
            .into_iter()
            .chain(
                self.plugin()
                    .into_iter()
                    .flat_map(|plugin| plugin.sections()),
            )
            .collect()
    }

    /// Entries the target has whenever it is managed, whatever packages install.
    fn fixed_entries(&self, project_root: &Path) -> Vec<(KeyedKind, Rendered)> {
        self.plugin()
            .map(|plugin| plugin.fixed_entries(self.name(), project_root))
            .unwrap_or_default()
    }
}

pub struct TargetRegistry {
    adapters: Vec<Box<dyn TargetAdapter>>,
}

impl TargetRegistry {
    pub fn new() -> Self {
        Self {
            adapters: vec![
                Box::new(agents::AgentsAdapter),
                Box::new(claude::ClaudeAdapter),
                Box::new(codex::CodexAdapter),
                Box::new(opencode::OpencodeAdapter),
                Box::new(pi::PiAdapter),
                Box::new(cursor::CursorAdapter),
                Box::new(agy::AgyAdapter),
            ],
        }
    }

    /// Returns `None` if no adapter is registered for the given name. Callers
    /// may fall back to a default behavior (currently: pass-through copy) when
    /// no adapter is found.
    pub fn get(&self, name: &str) -> Option<&dyn TargetAdapter> {
        self.adapters
            .iter()
            .find(|a| a.name() == name)
            .map(|a| a.as_ref())
    }
}

impl Default for TargetRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Return an error message when an agent name would create a Windows-invalid
/// native filename. Runs on every platform so generated packages stay portable.
pub fn validate_agent_filename(name: &str) -> Result<(), String> {
    if let Some(ch) = name.chars().find(|ch| WINDOWS_INVALID_CHARS.contains(ch)) {
        return Err(format!(
            "agent `{name}` contains portable filename-invalid character `{ch}`"
        ));
    }

    let stem = name
        .split('.')
        .next()
        .unwrap_or(name)
        .trim_end_matches([' ', '.'])
        .to_ascii_uppercase();

    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || stem
            .strip_prefix("COM")
            .is_some_and(|n| matches!(n, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"))
        || stem
            .strip_prefix("LPT")
            .is_some_and(|n| matches!(n, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"));

    if reserved {
        return Err(format!(
            "agent `{name}` would create reserved Windows device filename `{stem}`"
        ));
    }

    Ok(())
}

pub fn dest_paths_equivalent(a: &str, b: &str) -> bool {
    a.replace('\\', "/") == b.replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_invalid_agent_filename_is_rejected() {
        assert!(validate_agent_filename("bad:name").is_err());
        assert!(validate_agent_filename("team/lead").is_err());
        assert!(validate_agent_filename(r"team\lead").is_err());
        assert!(validate_agent_filename("CON").is_err());
        assert!(validate_agent_filename("com1").is_err());
    }
}
