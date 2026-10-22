# src/compiler/ — MCP reference grammar and projection

Single source of truth for canonical `mcp(...)` parsing and per-harness projection.
User-facing summary: [docs/config/agent-compilation.md](../../../docs/config/agent-compilation.md#mcp-tool-policy-references).

## Module

`mcp_ref.rs` — `McpRef`, `parse_mcp_ref`, `try_parse_mcp_tool_name`, `parse_foreign_mcp_token`,
`project_mcp_ref`.

`tool_policy.rs` — turns `tools:` / `disallowed-tools:` into typed [`ToolRule`]s
(`EffectiveToolPolicy`); an MCP entry is a rule whose `Capability::Mcp` holds the parsed
[`McpRef`] (the bundle spells it capability `mcp`, pattern `server/tool`).
`EffectiveToolPolicy::project` is the one place a rule becomes native permission names and
unscoped harness tool names. It retains permission→tool bindings through conflict
resolution; bundle consumers read `native` and `native_tools` instead of deriving either.

## Canonical grammar

Entries in `tools:` or `disallowed-tools:` use scoped head `mcp(...)`:

| Form | `McpRef` |
|---|---|
| `mcp(server)` | `Named(server)`, `Any` (tool) — normalizes to `mcp(server/*)` |
| `mcp(server/tool)` | `Named(server)`, `Named(tool)` |
| `mcp(server/*)` | `Named(server)`, `Any` |
| `mcp(*/tool)` | `Any` (server), `Named(tool)` |
| `mcp(*/*)` | `Any`, `Any` |

`*` is the only wildcard. Server and tool segments are preserved **verbatim** (no case
change). At most one `/` separator in the inner payload.

## Projection (`project_mcp_ref`)

Harness id is lowercase (`claude`, `cursor`, `opencode`, `codex`, `pi`). Unsupported refs
return `McpProjection::Unsupported` — callers omit the token and record lossiness.

| Harness | Native token shape | Unsupported |
|---|---|---|
| Claude | `mcp__server__tool`, `mcp__server__*`, `mcp__*` | `mcp(*/tool)` → `CrossServerTool` |
| Cursor | `Mcp(server:tool)` | — |
| OpenCode | `server_tool` | — |
| Codex | — | all refs → `PerToolNeedsServerConfig` |
| Pi | — | all refs → `HarnessDropsMcp` |

An unsupported ref projects to no native token and `native_status: unknown`; callers record
the reason as lossiness or a bundle rule `note` and never broaden permissions.

## Emission call sites

- **Claude agents** (`agents/lower.rs`) — merge projected tokens into `tools:` /
  `disallowed-tools:` frontmatter.
- **Claude skills** (`skills/lower_policy.rs`) — merge allowed MCP into `allowed-tools:`
  (grant semantics; always approximate lossiness note).
- **Launch bundle** (`build/mod.rs`) — `tools.rules` is the contract for every tool rule,
  including MCP. `tools.mcp` is a legacy flat list of allowed MCP permission names derived
  from the rules, never MCP server config; denied MCP names appear in `tools.disallowed`.

Non-Claude native agent artifacts do not emit tool lists; MCP lossiness is recorded at
lower time. The launch bundle's `tools.rules` is authoritative for spawn-time tool policy.

## Inbound lift

`staging/lift.rs` calls `parse_foreign_mcp_token` to convert Claude `mcp__…` and Cursor
`Mcp(...)` wire forms to canonical `mcp(...)` before compilation. Claude `mcpServers`
appends whole-server `mcp(server)` via `append_mcp_server_entries_to_tools`.
