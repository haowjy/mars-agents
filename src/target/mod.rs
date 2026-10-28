/// Per-target compilation adapters.
///
/// Each native target root (`.claude`, `.codex`, `.opencode`, `.pi`, `.cursor`)
/// has an adapter that knows how to lower agents, format config entries, translate
/// hooks, and resolve model aliases for that target.
///
/// The deprecated `.agents` adapter remains available only for explicit legacy
/// link targets; `.mars/` is the canonical compiled store.
///
/// The adapter boundary isolates all per-target branching here, keeping shared
/// compiler code free of `if target == ...` chains.
pub mod agents;
pub mod claude;
pub mod codex;
mod config_doc;
pub mod cursor;
pub(crate) mod hook_file;
pub mod mcp_file;
pub mod opencode;
pub mod pi;

use std::path::{Path, PathBuf};

use crate::error::MarsError;
use crate::surface_ownership::retention::{ConfigWrite, RemovalOperation, RemovalReport};
use crate::types::DestPath;
pub use config_doc::SectionFormat;
use mars_schema::item::ItemKind;
pub use mcp_file::{McpFile, McpLocation};

const WINDOWS_INVALID_CHARS: &[char] = &[':', '*', '?', '<', '>', '|', '"', '/', '\\'];

/// Lock-record key prefixes: `mcp:<name>` for a server, `hook:<event>:<name>` for a hook.
pub(crate) const MCP_KEY_PREFIX: &str = "mcp:";
pub(crate) const HOOK_KEY_PREFIX: &str = "hook:";

/// An MCP server entry ready to be written into a target config file.
///
/// Env values are variable names (symbolic). Adapters translate them to the
/// target's interpolation syntax (e.g. `${VAR}` for Claude, plain name for Codex).
#[derive(Debug, Clone)]
pub struct McpServerEntry {
    /// Server name as it appears in the target config.
    pub name: String,
    /// How the server is reached, with `${MARS_MCP_DIR}` already resolved.
    pub transport: mars_schema::mcp_server::McpTransport,
}

/// A native fragment contribution ready to be merged into a target config.
#[derive(Debug, Clone)]
pub struct HookEntry {
    /// Hook name, retained as ownership provenance.
    pub name: String,
    /// Native event name for this target.
    pub native_event: String,
    /// Opaque native entries, in author-declared order.
    pub entries: Vec<serde_json::Value>,
}

impl HookEntry {
    /// The lock-record key for this entry.
    pub fn key(&self) -> String {
        format!("{HOOK_KEY_PREFIX}{}:{}", self.native_event, self.name)
    }
}

/// How a target consumes hook fragments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookFragmentMode {
    MergeJson { events: &'static [&'static str] },
    File,
}

/// Per-target compilation adapter.
///
/// Implementations encapsulate all per-target knowledge:
/// - Which item kinds this target accepts
/// - Default destination path layout
/// - Config-entry format (future: MCP, hooks, model aliases)
///
/// The trait is split into file-output surfaces and config-entry surfaces so
/// parallel pipeline lanes can own disjoint write responsibilities without
/// interfering with each other.
///
/// # Object safety
/// All methods take `&self` and return concrete types to ensure the trait can
/// be used as `dyn TargetAdapter`.
pub trait TargetAdapter: std::fmt::Debug + Send + Sync {
    /// Target root name (e.g., `.claude`, `.codex`).
    fn name(&self) -> &str;

    /// Native fragment placement mechanism declared by this adapter.
    fn hook_fragment_mode(&self) -> Option<HookFragmentMode> {
        None
    }

    /// Relative destination for an opaque file-mode fragment.
    fn hook_file_dest_path(&self, _name: &str) -> Option<PathBuf> {
        None
    }

    /// Skill variant harness key used when projecting skills to this target.
    ///
    /// Native harness targets return the `variants/<key>/` directory name they
    /// consume. Full-fidelity targets that should not select skill variants
    /// return `None`.
    fn skill_variant_key(&self) -> Option<&str>;

    // -----------------------------------------------------------------------
    // Path resolution
    // -----------------------------------------------------------------------

    /// Default destination path for an item of the given kind and name.
    ///
    /// Returns `None` if this target does not accept the item kind. The
    /// compiler MUST skip items for which this returns `None`.
    fn default_dest_path(&self, kind: ItemKind, name: &str) -> Option<DestPath>;

    // -----------------------------------------------------------------------
    // Config-file writing
    // -----------------------------------------------------------------------

    /// Write hook entries to this target's hook config file. MCP servers are
    /// written by [`mcp_file::reconcile`] from [`Self::mcp_file`].
    ///
    /// Default: no-op — targets that don't use a config file leave this as-is.
    fn write_hook_entries(
        &self,
        write: ConfigWrite<'_>,
        project_root: &Path,
    ) -> Result<(), MarsError> {
        let _ = write.into_parts(project_root);
        Ok(())
    }

    /// The file this harness reads MCP servers from and how entries render
    /// there; `None` when the target takes no MCP servers.
    fn mcp_file(&self) -> Option<McpFile> {
        None
    }

    /// Config files mutated by merge-mode hook entries.
    fn hook_config_file_names(&self) -> &'static [&'static str] {
        &[]
    }

    /// Name what is wrong with an opaque file-mode hook fragment, if anything.
    ///
    /// Most targets treat file fragments as opaque text. Targets with a native
    /// loader report a contract violation; callers build the diagnostic text.
    fn hook_fragment_problem(&self, _content: &str) -> Option<&'static str> {
        None
    }

    /// Remove hook entries recorded in the previous lock by structural equality.
    fn remove_owned_hook_entries(
        &self,
        operation: RemovalOperation<'_>,
        project_root: &Path,
        _diag: &mut crate::diagnostic::DiagnosticCollector,
    ) -> RemovalReport {
        let (_, _) = operation.into_parts(project_root);
        RemovalReport::confirmed()
    }
}

/// Registry of target adapters, keyed by target root name.
///
/// Constructed once per sync run. Adapters are registered at startup; no
/// dynamic registration is needed.
pub struct TargetRegistry {
    adapters: Vec<Box<dyn TargetAdapter>>,
}

impl TargetRegistry {
    /// Build a registry containing all built-in target adapters.
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

    /// Look up an adapter by target root name.
    ///
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
    fn registry_contains_all_builtin_adapters() {
        let registry = TargetRegistry::new();

        for name in [
            ".agents",
            ".claude",
            ".codex",
            ".opencode",
            ".pi",
            ".cursor",
        ] {
            let adapter = registry
                .get(name)
                .unwrap_or_else(|| panic!("built-in target adapter `{name}` is not registered"));
            assert_eq!(adapter.name(), name);
        }
    }

    #[test]
    fn registry_get_unknown_name_returns_none() {
        let registry = TargetRegistry::new();
        assert!(registry.get(".unknown-target").is_none());
    }

    #[test]
    fn native_adapters_expose_skill_variant_keys() {
        let registry = TargetRegistry::new();
        let expected = [
            (".claude", Some("claude")),
            (".codex", Some("codex")),
            (".opencode", Some("opencode")),
            (".pi", Some("pi")),
            (".cursor", Some("cursor")),
            (".agents", None),
        ];

        for (target, key) in expected {
            let adapter = registry.get(target).unwrap();
            assert_eq!(adapter.skill_variant_key(), key);
        }
    }

    #[test]
    fn hook_event_allowlists_match_supported_command_hook_targets() {
        let registry = TargetRegistry::new();
        let Some(HookFragmentMode::MergeJson { events: claude }) =
            registry.get(".claude").unwrap().hook_fragment_mode()
        else {
            panic!("Claude must declare merge-mode hooks");
        };
        let Some(HookFragmentMode::MergeJson { events: codex }) =
            registry.get(".codex").unwrap().hook_fragment_mode()
        else {
            panic!("Codex must declare merge-mode hooks");
        };
        assert_eq!(claude.len(), 29);
        assert!(claude.contains(&"SessionEnd"));
        assert_eq!(codex.len(), 10);
        assert!(!codex.contains(&"SessionEnd"));
        let Some(HookFragmentMode::MergeJson { events: cursor }) =
            registry.get(".cursor").unwrap().hook_fragment_mode()
        else {
            panic!("Cursor must declare merge-mode hooks");
        };
        assert_eq!(cursor.len(), 21);
        assert!(cursor.contains(&"beforeShellExecution"));
        assert!(cursor.contains(&"sessionStart"));

        for target in [".opencode", ".pi"] {
            assert_eq!(
                registry.get(target).unwrap().hook_fragment_mode(),
                Some(HookFragmentMode::File)
            );
        }
    }

    #[test]
    fn agents_adapter_default_dest_path_agent() {
        let registry = TargetRegistry::new();
        let adapter = registry.get(".agents").unwrap();
        let path = adapter.default_dest_path(ItemKind::Agent, "coder").unwrap();
        assert_eq!(path.as_str(), "agents/coder.md");
    }

    #[test]
    fn agents_adapter_default_dest_path_skill() {
        let registry = TargetRegistry::new();
        let adapter = registry.get(".agents").unwrap();
        let path = adapter
            .default_dest_path(ItemKind::Skill, "planning")
            .unwrap();
        assert_eq!(path.as_str(), "skills/planning");
    }

    #[test]
    fn windows_invalid_agent_filename_is_rejected() {
        assert!(validate_agent_filename("bad:name").is_err());
        assert!(validate_agent_filename("team/lead").is_err());
        assert!(validate_agent_filename(r"team\lead").is_err());
        assert!(validate_agent_filename("CON").is_err());
        assert!(validate_agent_filename("com1").is_err());
    }

    #[test]
    fn valid_agent_filename_passes() {
        assert!(validate_agent_filename("coder").is_ok());
        assert!(validate_agent_filename("deep-agent").is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn path_equivalence_normalizes_separators_on_windows() {
        assert!(paths_equivalent(r"agents\coder.md", "agents/coder.md"));
    }

    #[cfg(not(windows))]
    #[test]
    fn path_equivalence_preserves_backslash_on_posix() {
        assert!(!paths_equivalent(r"agents\coder.md", "agents/coder.md"));
    }

    #[test]
    fn dest_path_equivalence_always_normalizes_separators() {
        assert!(dest_paths_equivalent(r"agents\coder.md", "agents/coder.md"));
    }
}
