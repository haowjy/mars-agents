use crate::types::DestPath;
use mars_schema::item::ItemKind;
use mars_schema::mcp_server::McpTransport;
use mars_schema::mcp_servers::{McpRender, env_map, header_map};

use super::{McpFile, McpLocation, McpServerEntry, SectionFormat, TargetAdapter};

const MCP_SECTION: &str = "mcp";

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

#[derive(Debug)]
pub struct OpencodeAdapter;

impl TargetAdapter for OpencodeAdapter {
    fn name(&self) -> &str {
        ".opencode"
    }

    fn skill_variant_key(&self) -> Option<&str> {
        Some("opencode")
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
