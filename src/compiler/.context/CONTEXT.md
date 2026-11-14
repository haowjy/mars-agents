# MCP tool policy

The schema crate owns canonical MCP reference parsing; `mars_schema::tool_project` projects those references to harness permissions. An unsupported reference produces no native permission and records lossiness, rather than granting broader access. Codex cannot express per-tool MCP permissions through its agent tool list, and Pi drops MCP permissions. Claude cannot express a named tool across all servers.

`mars_schema::tool_project::project` retains permission-to-tool bindings through conflict resolution. Bundle consumers must use the resulting native permission and tool availability fields separately; deriving one from the other can restore a tool whose permission was removed.

`tools.mcp` in a launch bundle is a legacy list of allowed permission names, not MCP server configuration. `tools.rules` is authoritative for spawn-time tool policy.
