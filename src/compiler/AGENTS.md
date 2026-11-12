# Compiler

Use the schema's prepared results rather than rediscovering definitions.

Standalone emission writes each agent to each managed harness, omitting an unresolved native model so the harness uses its default. Meridian-managed emission can suppress or select native agent copies. Reconciliation compares prior claims with the exact desired output set, including removed dependencies and renamed agents: current source names alone can't identify stale outputs. See [target ownership](../target_sync/.context/CONTEXT.md).

Build an agent's tool rules only through `effective_tools` (`effective_tools.rs`): the launch bundle (`build/mod.rs`), native files (`native_agents.rs`) and the plugin build share it, so they can't disagree. It returns the policy alone and drops no rule; `covered` names the allows and asks a stricter rule covers. Its first argument is the project-wide `[settings.tools]` map: rules that restrict every agent and widen none. A project-wide `allow` or `ask` is shaped by the agent's `'*'` rule (table in the function's doc) because Claude and OpenCode take one flat list; every branch keys on the rule's action, so a project `allow` must never remove or weaken an agent rule. The plugin build and `mars check` pass `ToolRuleMap::default()`, since those rules belong to one project and not to a package.

A caller that stores an agent's rules and later needs them against the current project rules passes them to `mars build launch-bundle --agent-tool-rules` (`effective_tools_for_rules`). It must not copy this shaping, which would drift from the table above.

A projected rule keeps every name it makes available; a stricter rule (coverage, or another rule naming the same native tool) marks the name `beaten_by` and sets `covered_by` once none is left. `native()` (what the permission engine hears) skips beaten names, so a covered rule tells it nothing and a reader that ignores `covered_by` cannot let through what the covering rule refuses; `available()` (Claude's `tools:` line, `tool_loss`) reads all of them, so an allowlist never empties into "every tool". Never clear a binding to say "covered".

Use `EffectiveToolPolicy::project` (the projection of a policy to native permissions and available tools, unrelated to project-wide rules) for native permissions and available tools. A native file never gets a permission the profile did not grant. What a file cannot hold is reported, never dropped silently: bundle rule status records the gap and sync reports author-facing lossiness. Claude files carry denies and a non-empty allowlist; OpenCode's `permission` map carries every rule (see `agents/opencode_permission.rs` for the guarantee and the order that makes OpenCode's last-match agree with Mars's strictest-match). Codex, Pi and Cursor files carry none, so Mars warns and the rules hold only for launches through Meridian. A scoped allow grants that scope; it doesn't make an allowlist. See [MCP projection](.context/CONTEXT.md) for MCP refs.

Three rules a change here can break without a test failing:

- `ToolRule.origin` stays outside `PartialEq` and rule identity. A profile rule and a project rule with the same key and action must fold into one rule; comparing origin would duplicate it.
- `tool_cover::relation` returns `Unknown` for anything it cannot prove. A wrong `Equal` or `Covers` marks an allow as never applying, so the bundle gives it no native name; `Unknown` leaves it to the enforcer, which matches the real call.
- `*` rules take no part in coverage: every enforcer ranks a named key above `*`, so a deny already wins over an agent's `*` rule.

Harness overrides cannot replace fields owned by lowering. The OpenCode `permission` map is never looser than the agent's tool rules or the author's `permission` override, and equals the rules when there is no override; it decides the calls it names over `opencode.json` (see the module doc of `agents/opencode_permission.rs`). Native hooks and MCP config come from the selected target state and are withheld when their required directory wasn't installed; see [sync](../sync/AGENTS.md).

Codex hook trust depends on entry positions: don't rewrite an unchanged merge file, and warn when entries are added or moved. Hook and MCP directory tokens resolve to absolute paths because a relative path can point at the wrong checkout in a git worktree.
