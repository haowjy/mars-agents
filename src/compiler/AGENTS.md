# Compiler

Use the schema's prepared results rather than rediscovering definitions.

Standalone emission writes each agent to each managed harness, omitting an unresolved native model so the harness uses its default. Meridian-managed emission can suppress or select native agent copies. Reconciliation compares prior claims with the exact desired output set, including removed dependencies and renamed agents: current source names alone can't identify stale outputs. See [target ownership](../target_sync/.context/CONTEXT.md).

Build an agent's tool rules only through `effective_tools` (`effective_tools.rs`): the launch bundle (`build/mod.rs`), native files (`native_agents.rs`) and the plugin build share it, so they can't disagree. It returns the policy alone and drops no rule; `covered` names the allows and asks a stricter rule covers, and the projection gives them no native name. Its first argument is the project-wide `[settings.tools]` map: rules that restrict every agent and widen none. The plugin build and `mars check` pass `ToolRuleMap::default()`, since those rules belong to one project and not to a package.

Use `EffectiveToolPolicy::project` (the projection of a policy to native permissions and available tools, unrelated to project-wide rules) for native permissions and available tools. Unsupported permissions must not broaden access; bundle rule status records the gap and sync reports author-facing lossiness. A scoped allow grants that scope; it doesn't make an allowlist. See [MCP projection](.context/CONTEXT.md) for MCP refs.

Four rules a change here can break without a test failing:

- `ToolRule.origin` stays outside `PartialEq` and rule identity. A profile rule and a project rule with the same key and action must fold into one rule; comparing origin would duplicate it.
- `tool_cover::relation` returns `Unknown` for anything it cannot prove. A wrong `Equal` or `Covers` marks an allow as never applying, so the bundle gives it no native name; `Unknown` leaves it to the enforcer, which matches the real call.
- `*` rules take no part in coverage: every enforcer ranks a named key above `*`, so a deny already wins over an agent's `*` rule.
- `{ replace }` never clears a project-wide rule or an overlay rule entry. It clears the profile's rules only; otherwise a user's replace would lift a project-wide deny.

Harness overrides cannot replace fields owned by lowering. OpenCode permissions merge with Mars deny rules, which win. Native hooks and MCP config come from the selected target state and are withheld when their required directory wasn't installed; see [sync](../sync/AGENTS.md).

Codex hook trust depends on entry positions: don't rewrite an unchanged merge file, and warn when entries are added or moved. Hook and MCP directory tokens resolve to absolute paths because a relative path can point at the wrong checkout in a git worktree.
