# Compiler

Use the schema's prepared results rather than rediscovering definitions.

Standalone emission writes each agent to each managed harness, omitting an unresolved native model so the harness uses its default. Meridian-managed emission can suppress or select native agent copies. Reconciliation compares prior claims with the exact desired output set, including removed dependencies and renamed agents: current source names alone can't identify stale outputs. See [target ownership](../target_sync/.context/CONTEXT.md).

Build an agent's tool rules only through `effective_tools`: the launch bundle, native files and the plugin build share it, so they can't disagree. Use `EffectiveToolPolicy::project` for native permissions and available tools. Unsupported permissions must not broaden access; bundle rule status records the gap and sync reports author-facing lossiness. A scoped allow grants that scope; it doesn't make an allowlist. See [MCP projection](.context/CONTEXT.md).

Harness overrides cannot replace fields owned by lowering. OpenCode permissions merge with Mars deny rules, which win. Native hooks and MCP config come from the selected target state and are withheld when their required directory wasn't installed; see [sync](../sync/AGENTS.md).

Codex hook trust depends on entry positions: don't rewrite an unchanged merge file, and warn when entries are added or moved. Hook and MCP directory tokens resolve to absolute paths because a relative path can point at the wrong checkout in a git worktree.
