# MCP Servers and Hooks

Packages can ship MCP server registrations and lifecycle hooks alongside their
agents and skills. Mars compiles these into target-specific config files during
`mars sync`.

- **MCP servers** are registered per harness target (`.claude/.mcp.json`,
  `.codex/mcp.json`, etc.) from `mcp/<name>/mcp.toml` package definitions.
- **MCP tool-policy refs** (`mcp(...)` in agent/skill `tools:` / `disallowed-tools:`)
  gate which MCP tools an agent or skill may use — separate from server registration.
  Per-harness projection: [agent-compilation.md](agent-compilation.md#mcp-tool-policy-references).
- **Hooks** contribute target-native JSON fragments to Claude and Codex hook
  config and install their complete hook directories beside the target.

Config entries are tracked in `mars.lock` so Mars can clean them up
automatically when a package is removed or updated.

## Tool-policy MCP references vs server definitions

Two lanes work together but serve different purposes:

| Lane | Authoring | What it does |
|---|---|---|
| **Server definitions** | `mcp/<name>/mcp.toml` in a package | Registers how to launch an MCP server in target config (`.mcp.json`, etc.) |
| **Tool-policy refs** | `tools: [mcp(server/tool)]` on agents/skills | Grants or denies MCP tool access in the agent/skill tool policy |

Whole-server **enablement** on Codex is governed by the server-definition lane (and
`mcp_servers.enabled_tools` in harness config), not by per-tool entries in `tools:`.
Per-tool `mcp(server/tool)` grants in frontmatter still record lossiness on Codex because
MCP gating there is server-config based, not a tool-list form.

Claude agents emit projected `mcp__…` tokens in `tools:` / `disallowed-tools:`; Claude
skills grant allowed MCP into `allowed-tools:`. Non-Claude native agent files do not
emit tool lists today — the launch bundle (`ToolsSpec.mcp`) carries the real per-harness
projection at spawn time.

## Declaring MCP Servers in a Package

Place one directory per server under `mcp/` at the package root:

```
my-package/
  mcp/
    context7/
      mcp.toml
    memory-bank/
      mcp.toml
```

Each `mcp.toml` specifies the server:

```toml
# mcp/context7/mcp.toml
command = "npx"
args    = ["-y", "@upstash/context7-mcp@latest"]

# Optional: restrict to specific targets (default: all targets)
targets = [".claude", ".codex"]

# Optional: control propagation to transitive consumers
# "local" (default) = only direct consumers get this server
# "exported" = propagates to transitive consumers too
visibility = "local"
```

**Env references** — if the server needs secrets, declare them symbolically.
Mars never resolves the values; harnesses substitute them at runtime:

```toml
command = "node"
args    = ["server.js"]

[env]
API_KEY   = { from = "env", var = "MY_API_KEY" }
API_TOKEN = { from = "env", var = "MY_API_TOKEN" }
```

The `from = "env"` field is the only supported kind (V0). Mars warns at sync
time when the named variable isn't present in the environment, but sync still
proceeds.

**Name override** — by default the server name matches the directory name. To
use a different name:

```toml
# Directory is "my-dir", but server is registered as "custom-name"
name    = "custom-name"
command = "node"
```

## Declaring Hooks in a Package

A hook directory contains `hook.toml`, one native JSON fragment per declared
target, and any scripts or assets it needs:

```
my-package/hooks/audit/
  hook.toml
  claude.json
  codex.json
  run.sh
```

`hook.toml` controls identity, propagation, deterministic placement, and which
fragments ship. The hook name defaults to the directory name.

```toml
# hooks/audit/hook.toml
name = "audit"              # optional; defaults to "audit"
visibility = "exported"     # dependencies must export hooks
order = 10                  # deterministic merge tiebreaker; default 0

[targets.".claude"]
fragment = "claude.json"    # optional; this is the default filename

[targets.".codex"]
fragment = "codex.json"     # optional; this is the default filename
```

Each fragment is the harness's native event-keyed object—the value that its
documentation places under `"hooks"`. Mars preserves every entry field and
only merges arrays by event:

```json
{
  "PreToolUse": [
    {
      "matcher": "Bash|Agent",
      "hooks": [
        {
          "type": "command",
          "command": "bash \"${MARS_HOOK_DIR}/run.sh\"",
          "timeout": 30
        }
      ]
    }
  ]
}
```

The quoted command form matters when a project path contains spaces. Authors
own the complete command string; Mars does not add a shell or quoting wrapper.
It textually replaces every `${MARS_HOOK_DIR}` occurrence in every JSON string
with the absolute installed directory, such as
`/work/project/.claude/hooks/audit`. A fragment may omit the placeholder.

For copy-paste convenience, Mars also accepts a full documented wrapper and
unwraps `hooks` when the only sibling keys are `version` and/or `description`:

```json
{
  "description": "audit hooks",
  "hooks": { "SessionStart": [] }
}
```

Only declared target tables receive the hook. Mars copies the whole authored
directory to `<target>/hooks/<name>/`; `hook.toml` and fragment files remain
there as inert package metadata. Phase A supports merge-mode fragments for
Claude (`settings.local.json`) and Codex (`hooks.json`). Cursor and the
OpenCode/Pi TypeScript file modes are not yet enabled and fail rather than
silently dropping a declaration.

Mars validates only the merge contract:

1. the fragment is valid JSON;
2. its top-level event names appear in the target allowlist;
3. every event value is an array.

Nested matcher, handler, timeout, and harness-specific fields pass through
unchanged. Claude currently has 29 known events; Codex has 10. Codex
`SessionEnd` remains excluded because it was runtime-verified not to fire in
Codex 0.144.4. To use a newly shipped native event before Mars updates its
allowlist, opt in on that target:

```toml
[targets.".claude"]
unchecked = true
```

Mars warns and passes unknown keys when `unchecked = true`; without it, the
error lists valid events. All fragment parsing and validation happens before
Mars mutates the canonical store or any target config.

### Ordering and Codex trust

Mars sorts contributions by package depth, dependency declaration order,
`order`, and hook name, preserving each fragment array's internal order. It
appends the managed block after user-authored entries. This guarantees stable
placement, not execution order—Claude runs matching hooks in parallel.

Codex trust is indexed by hook file, event, and array position. Adding or
reordering managed hooks can shift indices, so Codex may silently skip affected
hooks until you re-trust them with `/hooks`. This re-trust churn is an accepted
Codex behavior; deterministic ordering minimizes it but cannot prevent it when
a new hook sorts ahead of an existing one.

### Ownership and removal

For each `hook:<Event>:<name>` key, `mars.lock` stores the exact emitted entry
array after path substitution. Before writing replacements, Mars removes
structurally equal entries from the current config. User-edited entries no
longer match and are preserved; user-authored entries and unrelated config are
always untouched. Event keys emptied by Mars are pruned.

For one release, removal also recognizes v0.11.0 command-path entries containing
`/hooks/<name>/`, including staging-path commands. This migration sweep runs
before fragment writes and shares the next-release deletion ledger with the
existing OpenCode and legacy-Codex residue sweeps.

## Collision Resolution

When two packages declare an MCP server or hook with the same name for the same
target, Mars resolves the collision deterministically:

**For MCP servers**, collision identity is the server name + target root.
**For hooks**, collision identity is `(event, name)` + target root — hooks with
the same name on different events are distinct and both install.

**Precedence rules (highest to lowest):**

1. **Local package (`_self`) always wins** — an MCP server or hook declared in
   your project's local `mcp/` or `hooks/` directory silently overrides any
   dependency that declares the same name. No warning is emitted.

2. **Earlier declaration order wins** — when two dependencies declare the same
   name, the one that appears earlier in `[dependencies]` in `mars.toml` wins
   and the later one is dropped. A warning is emitted naming both sources.

3. **Alphabetical tiebreak** — when two sources have the same declaration order
   (e.g., both are transitive at the same depth), the alphabetically-first
   package name wins. A warning is emitted naming both sources.

Collision resolution is per target root. A collision in one target does not
affect what gets installed in other targets.

**Example output when two dependencies collide:**

```
warning[config-entry-collision]: MCP server `context7` collision in target `.claude`:
  `meridian-base` wins over `acme-agents`
```

**Suppressing a dependency's server with a local override:**

```
mcp/context7/mcp.toml  ← your local version wins silently
```

## Stale Config Entry Cleanup

Mars tracks which config entries it installed, attributed to their source
package, in `mars.lock`. On every `mars sync`:

- If a package is removed from `mars.toml`, its MCP servers and hooks are
  removed from all target config files.
- If a package is updated and no longer declares a server or hook that was
  previously present, that entry is removed.
- If a local (`_self`) entry is removed from `mcp/` or `hooks/`, it is removed
  from target config files.

**Dry run** — `mars sync --diff` reports stale entries as warnings but does not
remove them:

```
warning[stale-config-entry]: target `.claude` has stale config entries:
  mcp:context7, hook:tool.pre:audit
```

On a normal `mars sync`, successful removal emits an info diagnostic:

```
info[stale-config-entry]: removed stale config entries from `.claude`:
  mcp:context7, hook:tool.pre:audit
```

## Windows Compatibility

**Hook script invocation** — hook fragments own the entire native command
string, including the shell and quoting. Mars only substitutes the absolute
`${MARS_HOOK_DIR}` value. Author commands for every platform the package claims
to support; Mars does not normalize separators or synthesize a Windows branch.

**Agent filename validation** — Mars validates agent names against Windows
filename constraints at compile time, on all platforms. An agent named with
characters invalid on Windows (`: * ? < > | " / \`) or matching a reserved
device name (`CON`, `PRN`, `NUL`, `COM1`–`COM9`, `LPT1`–`LPT9`) is skipped
with a diagnostic error. This ensures agent packages stay portable regardless
of the authoring platform.

**Path separator matching** — when Mars matches config entries and lock file
provenance records against paths, it treats `/` and `\` as equivalent. Filters
and stale-cleanup logic authored on one platform work correctly on another.

**`mars cache info --json`** — on Windows, backslashes in path values are
properly escaped in JSON output so the JSON is always valid.
