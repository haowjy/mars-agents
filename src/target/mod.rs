mod codex;
mod config_doc;
pub(crate) mod hook_file;
pub(crate) mod keyed_entries;
pub mod mcp_file;
mod opencode;
pub(crate) mod plugin;
mod table;

pub use table::{TargetSpec, for_harness, spec};

/// The keyed sections Mars writes entries into for `name`. A retired target has none a sync
/// writes; `mars unlink` reads the MCP file earlier syncs wrote there to remove its entries.
pub fn keyed_sections(name: &str) -> Vec<(KeyedKind, KeyedSection)> {
    if let Some(retired) = mars_schema::retired::retired_target(name) {
        return vec![(
            mcp_file::MCP_KIND,
            KeyedSection {
                path: retired.mcp_path,
                format: SectionFormat::JsonSection(retired.mcp_section),
            },
        )];
    }
    spec(name)
        .map(TargetSpec::keyed_sections)
        .unwrap_or_default()
}

pub use config_doc::SectionFormat;
pub use keyed_entries::{KeyedKind, KeyedSection};
pub use mcp_file::McpFile;
pub(crate) use plugin::HOOK_ENTRY_KIND;

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

    /// `(native event, name)` of a record key [`Self::key`] made.
    pub(crate) fn parse_key(key: &str) -> Option<(&str, &str)> {
        key.strip_prefix(HOOK_KEY_PREFIX)?.split_once(':')
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

/// Delete a config file Mars's change emptied, and its directory if that leaves it empty (Mars
/// made `.agents/` for `plugins.json`).
pub(crate) fn remove_emptied_file(path: &std::path::Path) -> Result<(), crate::error::MarsError> {
    std::fs::remove_file(path)
        .map_err(|error| crate::error::MarsError::io("remove", path, error))?;
    if let Some(parent) = path.parent() {
        crate::target_sync::removal::remove_empty_ancestors(parent, parent);
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
