use crate::types::DestPath;
use mars_schema::HarnessId;
use mars_schema::item::ItemKind;
use mars_schema::mcp_servers::{self, Flavor, McpRender};

use super::plugin::PluginTarget;
use super::{KeyedSection, McpDirs, McpFile, McpServerEntry, SectionFormat, TargetAdapter};

/// `agy` reads nothing from `.agy/` itself: the project's `.agents/plugins.json` points at it
/// as a plugin, which carries the skills, agents, hooks and MCP servers.
#[derive(Debug)]
pub struct AgyAdapter;

impl TargetAdapter for AgyAdapter {
    fn harness(&self) -> Option<HarnessId> {
        Some(HarnessId::Agy)
    }

    fn default_dest_path(&self, kind: ItemKind, name: &str) -> Option<DestPath> {
        match kind {
            ItemKind::Skill => Some(DestPath::from(format!("skills/{name}").as_str())),
            _ => None,
        }
    }

    fn mcp_file(&self) -> Option<McpFile> {
        Some(McpFile {
            at: KeyedSection {
                path: ".agy/mcp_config.json",
                format: SectionFormat::JsonSection("mcpServers"),
            },
            render,
        })
    }

    /// `agy` substitutes `${PLUGIN_ROOT}` in `args` and `env`; it was not shown to in `command`,
    /// which gets the installed path as every harness does.
    fn mcp_dirs(&self, native: &str, installed: &str) -> McpDirs {
        McpDirs {
            command: installed.to_owned(),
            args: format!("${{PLUGIN_ROOT}}/{native}"),
        }
    }

    fn plugin(&self) -> Option<PluginTarget> {
        Some(PluginTarget {
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
        })
    }

    /// `agy` loads `.agents/skills/` too, so a skill in both is in front of it twice.
    fn reads_agents_skills(&self) -> bool {
        true
    }
}

fn render(server: &McpServerEntry) -> McpRender {
    mcp_servers::render(&server.transport, Flavor::Agy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hook_file_is_where_the_hook_table_places_keyed_hooks() {
        let adapter = AgyAdapter;
        let file = mars_schema::hooks::targets::target(adapter.name())
            .and_then(|target| target.keyed_file())
            .expect("the hook table has a keyed row for .agy");
        assert_eq!(
            adapter.plugin().unwrap().hooks.path,
            format!("{}/{file}", adapter.name())
        );
    }
}
