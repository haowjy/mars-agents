pub mod agents;
pub mod claude;
pub mod codex;
mod config_doc;
pub mod cursor;
pub(crate) mod hook_file;
pub mod mcp_file;
pub mod opencode;
pub mod pi;

use crate::types::DestPath;
pub use config_doc::SectionFormat;
use mars_schema::item::ItemKind;
pub use mcp_file::{McpFile, McpLocation};

const WINDOWS_INVALID_CHARS: &[char] = &[':', '*', '?', '<', '>', '|', '"', '/', '\\'];

pub(crate) const MCP_KEY_PREFIX: &str = "mcp:";
pub(crate) const HOOK_KEY_PREFIX: &str = "hook:";

/// Env values are variable names (symbolic). Adapters translate them to the
/// target's interpolation syntax (e.g. `${VAR}` for Claude, plain name for Codex).
#[derive(Debug, Clone)]
pub struct McpServerEntry {
    pub name: String,
    pub transport: mars_schema::mcp_server::McpTransport,
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
    fn name(&self) -> &str;

    /// Skill variant harness key used when projecting skills to this target.
    ///
    /// Native harness targets return the `variants/<key>/` directory name they
    /// consume. Full-fidelity targets that should not select skill variants
    /// return `None`.
    fn skill_variant_key(&self) -> Option<&str>;

    /// Returns `None` if this target does not accept the item kind. The
    /// compiler MUST skip items for which this returns `None`.
    fn default_dest_path(&self, kind: ItemKind, name: &str) -> Option<DestPath>;

    /// The file this harness reads MCP servers from and how entries render
    /// there; `None` when the target takes no MCP servers.
    fn mcp_file(&self) -> Option<McpFile> {
        None
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

pub fn paths_equivalent(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.replace('\\', "/") == b.replace('\\', "/")
    } else {
        a == b
    }
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
