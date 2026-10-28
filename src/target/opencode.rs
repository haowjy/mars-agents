/// `.opencode` target adapter.
///
/// Handles MCP server registration for the OpenCode harness.
///
/// OpenCode-native lowering:
/// - MCP: writes to `opencode.json` (`mcp` section), with local command arrays and
///   `{env:VAR}` environment references
///
/// Mars has no OpenCode hook config writer; OpenCode hooks are TypeScript plugins.
use std::path::PathBuf;

use crate::types::DestPath;
use mars_schema::item::ItemKind;
use mars_schema::mcp_server::McpTransport;
use mars_schema::mcp_servers::{McpRender, env_map, header_map};

use super::{McpFile, McpLocation, McpServerEntry, SectionFormat, TargetAdapter};

const MCP_SECTION: &str = "mcp";

/// Render one server as an OpenCode `mcp` entry with `{env:VAR}` references:
/// `local` is a command array, `remote` a URL with headers.
fn render(server: &McpServerEntry) -> McpRender {
    let interpolate = |var: &str| format!("{{env:{var}}}");
    let entry = match &server.transport {
        McpTransport::Stdio { command, args, env } => {
            let mut entry = serde_json::json!({
                "type": "local",
                "command": std::iter::once(command).chain(args).collect::<Vec<_>>(),
            });
            if !env.is_empty() {
                entry["environment"] = env_map(env, interpolate);
            }
            entry
        }
        McpTransport::Http { url, headers } => {
            let mut entry = serde_json::json!({ "type": "remote", "url": url });
            if !headers.is_empty() {
                entry["headers"] = header_map(headers, interpolate);
            }
            entry
        }
    };
    McpRender::exact(entry)
}

/// Warning-only check that a fragment exports a default `{ id, setup }`:
/// `export default …`, `export { x as default }` or `export * as default`.
/// It scans identifier tokens, so a commented-out export satisfies it; the cost
/// of that miss is a warning not shown.
fn has_default_export(content: &str) -> bool {
    let tokens: Vec<&str> = content
        .split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '$')))
        .filter(|token| !token.is_empty())
        .collect();
    tokens
        .windows(2)
        .any(|pair| matches!(pair, ["export", "default"] | ["as", "default"]))
}

#[derive(Debug)]
pub struct OpencodeAdapter;

impl TargetAdapter for OpencodeAdapter {
    fn name(&self) -> &str {
        ".opencode"
    }

    fn hook_file_dest_path(&self, name: &str) -> Option<PathBuf> {
        Some(PathBuf::from(format!("plugins/mars-{name}.ts")))
    }

    fn skill_variant_key(&self) -> Option<&str> {
        Some("opencode")
    }

    fn hook_fragment_problem(&self, content: &str) -> Option<&'static str> {
        (!has_default_export(content))
            .then_some("has no recognizable default export; add a default `{ id, setup }` export")
    }

    fn mcp_file(&self) -> Option<McpFile> {
        Some(McpFile {
            at: McpLocation {
                path: ".opencode/opencode.json",
                format: SectionFormat::JsonSection(MCP_SECTION),
            },
            render,
        })
    }

    fn default_dest_path(&self, kind: ItemKind, name: &str) -> Option<DestPath> {
        match kind {
            ItemKind::Skill => Some(DestPath::from(format!("skills/{name}").as_str())),
            _ => None,
        }
    }
}
