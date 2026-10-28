/// `.codex` target adapter.
///
/// Handles MCP server registration and hook binding for the Codex harness.
///
/// Codex-native lowering:
/// - MCP: edits `[mcp_servers.<name>]` in `.codex/config.toml`, env vars forwarded by name
/// - Hooks: appended to `hooks.json` as Codex command hook entries
use crate::types::DestPath;
use indexmap::IndexMap;
use mars_schema::item::ItemKind;
use mars_schema::mcp_server::{HeaderValue, McpTransport};
use mars_schema::mcp_servers::{McpRender, partition_env};

use super::{HookPlacement, McpFile, McpLocation, McpServerEntry, SectionFormat, TargetAdapter};

#[derive(Debug)]
pub struct CodexAdapter;

impl TargetAdapter for CodexAdapter {
    fn name(&self) -> &str {
        ".codex"
    }

    fn skill_variant_key(&self) -> Option<&str> {
        Some("codex")
    }

    fn default_dest_path(&self, kind: ItemKind, name: &str) -> Option<DestPath> {
        match kind {
            ItemKind::Skill => Some(DestPath::from(format!("skills/{name}").as_str())),
            _ => None,
        }
    }

    fn mcp_file(&self) -> Option<McpFile> {
        Some(McpFile {
            // Codex reads `[mcp_servers.<name>]` from the project's
            // `.codex/config.toml`, for trusted projects only.
            at: McpLocation {
                path: ".codex/config.toml",
                format: SectionFormat::TomlTable("mcp_servers"),
            },
            render,
        })
    }

    fn hook_placement(&self) -> Option<HookPlacement> {
        Some(HookPlacement::Merge {
            file: "hooks.json",
            top_level: &[],
            trust_gated: true,
        })
    }
}

// ---------------------------------------------------------------------------
// Codex MCP — `[mcp_servers.<name>]` entry shape
// ---------------------------------------------------------------------------
//
// Codex forwards an environment variable to the server by name, in
// `env_vars = ["VAR"]`. A mapping from a differently named key cannot be
// written, so a renamed reference is reported as lossy and dropped.

fn render(server: &McpServerEntry) -> McpRender {
    match &server.transport {
        McpTransport::Stdio { command, args, env } => {
            let mut entry = serde_json::json!({ "command": command, "args": args });
            let (forwarded, renamed) = partition_env(env);
            if !forwarded.is_empty() {
                entry["env_vars"] = serde_json::json!(forwarded);
            }
            McpRender {
                value: entry,
                lossy: renamed
                    .into_iter()
                    .map(|(key, variable)| {
                        format!(
                            "Codex forwards `{variable}` by name and cannot set it as `{key}`; `{key}` is not set for the server."
                        )
                    })
                    .collect(),
            }
        }
        McpTransport::Http { url, headers } => render_remote(url, headers),
    }
}

/// A Streamable HTTP server. Codex reads a header from the environment in
/// whole (`env_http_headers`, no prefix) and has one prefixed form,
/// `bearer_token_env_var` for `Authorization: Bearer <value>`; any other
/// prefixed env header has no spelling and is reported and dropped.
fn render_remote(url: &str, headers: &IndexMap<String, HeaderValue>) -> McpRender {
    let mut entry = serde_json::json!({ "url": url });
    let mut literals = serde_json::Map::new();
    let mut from_env = serde_json::Map::new();
    let mut lossy = Vec::new();
    for (name, value) in headers {
        match value {
            HeaderValue::Literal(text) => {
                literals.insert(name.clone(), text.as_str().into());
            }
            HeaderValue::Env { env, prefix } if prefix.is_empty() => {
                from_env.insert(name.clone(), env.var_name().into());
            }
            HeaderValue::Env { env, prefix }
                if name.eq_ignore_ascii_case("authorization") && prefix == "Bearer " =>
            {
                entry["bearer_token_env_var"] = env.var_name().into();
            }
            HeaderValue::Env { env, prefix } => lossy.push(format!(
                "Codex reads header `{name}` from `{}` whole and cannot prepend `{prefix}`; `{name}` is not set for the server.",
                env.var_name()
            )),
        }
    }
    if !literals.is_empty() {
        entry["http_headers"] = literals.into();
    }
    if !from_env.is_empty() {
        entry["env_http_headers"] = from_env.into();
    }
    McpRender {
        value: entry,
        lossy,
    }
}
