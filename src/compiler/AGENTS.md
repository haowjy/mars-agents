# Compiler

The compiler turns selected package content into canonical and harness-native outputs. Schema parsing and authored vocabulary belong to `mars-schema`; use its prepared results rather than rediscovering definitions.

Standalone emission writes each agent to each managed harness, omitting an unresolved native model so the harness uses its default. Meridian-managed emission can suppress or select native agent copies. Reconciliation compares prior claims with the exact desired output set, including removed dependencies and renamed agents; current source names alone cannot identify stale outputs. See [target ownership](../target_sync/.context/CONTEXT.md).

Use `EffectiveToolPolicy::project` for native permissions and available tools. Unsupported permissions must not broaden access; bundle rule status records the gap and sync reports author-facing lossiness. See [MCP projection](.context/CONTEXT.md).

A scoped allow grants that scope; it does not make an allowlist. Lost restrictions are reported in one line per project by default; other lost fields are available under `--verbose`.

Harness overrides cannot replace fields owned by lowering. OpenCode permissions merge with Mars deny rules, which take precedence. Native hooks and MCP config must come from the selected target state and are withheld when their required directory was not installed; see [sync](../sync/AGENTS.md).

Codex hook trust depends on entry positions. Avoid rewriting an unchanged merge file, and warn when entries are added or moved. Hook and MCP directory tokens resolve to absolute paths because a relative path can point to the wrong checkout in a git worktree.
