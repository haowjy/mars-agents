# MCP Servers and Hooks

Packages can ship MCP server registrations and lifecycle hooks alongside their
agents and skills. Mars compiles these into target-specific config files during
`mars sync`.

- **MCP servers** are registered per harness target (`.claude/.mcp.json`,
  `.codex/mcp.json`, etc.) from `mcp/<name>/mcp.toml` package definitions.
- **MCP tool-policy refs** (`mcp(...)` in agent/skill `tools:` / `disallowed-tools:`)
  gate which MCP tools an agent or skill may use — separate from server registration.
  Per-harness projection: [agent-compilation.md](agent-compilation.md#mcp-tool-policy-references).
- **Hooks** run scripts in response to harness lifecycle events
  (`session.start`, `tool.pre`, etc.) and are registered in each target's
  hook config file.

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

Place one directory per hook under `hooks/` at the package root:

```
my-package/
  hooks/
    audit/
      hook.toml
      run.sh
    cleanup/
      hook.toml
      run.sh
```

Each `hook.toml` specifies the hook:

```toml
# hooks/audit/hook.toml
name  = "audit"
event = "tool.pre"

[action]
kind = "script"
path = "./run.sh"   # relative to the hook directory, must not traverse outside package
```

**Supported events (V0):**

| Event | Fires when |
|---|---|
| `session.start` | Agent session starts |
| `session.end` | Agent session ends |
| `tool.pre` | Before a tool call executes |
| `tool.post` | After a tool call executes |

Non-V0 events are rejected with an error at sync time.

**Target restriction and ordering:**

```toml
name  = "claude-only-hook"
event = "session.start"

# Restrict to specific targets (default: all targets)
targets = [".claude"]

# Explicit ordering hint — lower runs earlier (default: 0)
order = 10

[action]
kind = "script"
path = "./run.sh"
```

**Script path constraints** — paths must be relative to the hook directory and
must not escape the package root with `..` components or absolute paths. Mars
rejects invalid paths at discovery time.

## Hook Event Mapping Per Target

Mars translates universal events to native equivalents. Some targets don't
support all events:

| Universal event | `.claude` | `.codex` | `.opencode` | `.cursor` | `.pi` |
|---|---|---|---|---|---|
| `session.start` | `SessionStart` (exact) | `SessionStart` (approx) | `session:start` (approx) | — dropped | — dropped |
| `session.end` | `SessionEnd` (exact) | `Stop` (approx) | `session:end` (approx) | — dropped | — dropped |
| `tool.pre` | `PreToolUse` (exact) | `PreToolUse` (approx) | `tool:before` (approx) | — dropped | — dropped |
| `tool.post` | `PostToolUse` (exact) | `PostToolUse` (approx) | `tool:after` (approx) | — dropped | — dropped |

- **Exact** — semantics match the universal definition precisely.
- **Approx** — closest available native event; semantics may differ slightly.
  Mars emits an `info`-level diagnostic.
- **Dropped** — no native hook surface; the hook is skipped for this target
  with a `warning`-level diagnostic. The hook still runs on other targets.

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

## Hook Ordering

Within a target, hooks are ordered deterministically:

1. **Package depth** — hooks from the root consumer project (depth 0) run
   before hooks from direct dependencies (depth 1), which run before transitive
   dependencies (depth 2+).
2. **Declaration order** — within the same depth, hooks from earlier `[dependencies]`
   entries run before later ones. Transitive packages inherit the declaration
   order of the earliest direct dependency that reaches them.
3. **`order` field** — lower values run earlier (default `0`). Use this to fine-tune
   ordering within a single package.
4. **Hook name** — lexicographic tiebreaker.

## Windows Compatibility

**Hook script invocation** — Mars generates `bash` invocations for all targets.
On POSIX, single-quoting is used; on Windows, double-quoting with forward-slash
normalization ensures Git for Windows bash compatibility:

```
# POSIX
bash '/abs/path/to/hooks/audit/run.sh'

# Windows
bash "C:/abs/path/to/hooks/audit/run.sh"
```

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
