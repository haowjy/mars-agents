# MCP Servers and Hooks

Packages can ship MCP server registrations and lifecycle hooks alongside their
agents and skills. Mars compiles these into target-specific config files during
`mars sync`.

- **MCP servers** are registered per harness target, in the file that harness
  reads (see [Where servers are written](#where-servers-are-written)), from
  `mcp/<name>/mcp.toml` package definitions.
- **MCP tool-policy refs** (`mcp(...)` entries in an Agent's `tools` map)
  gate which MCP tools an agent may use — separate from server registration.
  Per-harness projection: [agent-compilation.md](agent-compilation.md#mcp-tool-policy-references).
- **Hooks** contribute target-native JSON fragments to Claude, Codex, and
  Cursor config, or whole TypeScript plugins to OpenCode and Pi, and install
  their complete hook directories beside the target.
- **Server files** shipped beside `mcp.toml` are installed with their server
  directory when `command` or `args` names it with `${MARS_MCP_DIR}`.

Config entries are tracked in `mars.lock` so Mars can clean them up
automatically when a package is removed or updated.

## Where servers are written

Each target gets its servers in the file its harness reads, relative to the
project root:

| Target | File | Section | Env reference |
|---|---|---|---|
| `.claude` | `.mcp.json` | `mcpServers` | `${VAR}` |
| `.codex` | `.codex/config.toml` | `[mcp_servers.<name>]` | `env_vars = ["VAR"]` |
| `.opencode` | `.opencode/opencode.json` | `mcp` | `{env:VAR}` |
| `.pi` | `.pi/mcp.json` | `mcpServers` | `${VAR}` |
| `.cursor` | `.cursor/mcp.json` | `mcpServers` | `${env:VAR}` |

Mars edits these files in place. Other keys, other servers, and (in
`.codex/config.toml`) comments and key order survive. Stdio servers are written as
`{ "command", "args", "env" }`; OpenCode as
`{ "type": "local", "command": [...], "environment": {...} }`. Remote servers are
covered under [Remote servers](#remote-servers).

**Codex** forwards an environment variable to the server by name, so
`env_vars = ["VAR"]` covers an `env` entry whose key equals its variable. A
renamed reference (`KEY = { var = "OTHER" }`) cannot be written; sync warns
(`mcp-lossy`) and leaves that variable out.

**Approval and trust are the user's.** Mars writes the entry but does not
grant trust: Claude lists project `.mcp.json` servers as pending until you
approve them, and Codex and Pi read a project's MCP config only for projects you
have trusted.

**OpenCode** keeps the `mcp.<name>` form. OpenCode 2 documents `mcp.servers.<name>`
as its native shape and says the 1.x form "remains supported", while OpenCode 1
reads only `mcp.<name>`, so the older form works on both.

### Ownership

An entry is Mars's only while its content still equals what Mars last wrote
(`mars.lock` records it as `emitted_json`):

- A server the lock does not own with the same name as a package server is
  **kept**, with an `mcp-unmanaged-collision` warning. `mars sync --force` adopts it.
  An entry identical to what Mars would write is adopted without a warning, and
  `mars remove` deletes it.
- An entry Mars wrote and you then edited is kept with `mcp-divergent`; `--force`
  overwrites it.
- Removing a package (or a server) deletes only entries that still equal what
  Mars wrote; an edited one stays, with a warning.

### Upgrading

Earlier releases wrote `.claude/.mcp.json` and `.codex/codex_mcp.json`, which
neither harness read. The next sync removes Mars's entries from those files,
leaves anything else in them, and deletes a file only if it ends up empty.
OpenCode servers a still-earlier release wrote under `mcpServers` are migrated
to `mcp` the same way. The legacy locations are read for one release.

- The first sync overwrites any edit you made to a Cursor entry, or to an OpenCode
  entry still under `mcpServers`, that an earlier release wrote (those releases
  rewrote edited entries on every sync); from then on edits are kept.
- `.mcp.json` at the project root is new: a project that does not ignore it sees a
  new untracked file.
- A file Mars created that ends up empty (`.mcp.json` as `{}`, a 0-byte
  `.codex/config.toml`) is left in place, since nothing records that Mars created it.

## Tool-policy MCP references vs server definitions

Two lanes work together but serve different purposes:

| Lane | Authoring | What it does |
|---|---|---|
| **Server definitions** | `mcp/<name>/mcp.toml` in a package | Registers how to launch an MCP server in the harness's MCP config |
| **Tool-policy refs** | `tools: {mcp(server/tool): allow}` on agents | Grants or denies MCP tool access in the agent's tool policy |

Whole-server **enablement** on Codex is governed by the server-definition lane (and
`mcp_servers.enabled_tools` in harness config), not by per-tool entries in `tools:`.
Per-tool `mcp(server/tool)` grants in frontmatter still record lossiness on Codex because
MCP gating there is server-config based, not a tool-list form.

Claude agents emit projected `mcp__…` tokens in the native `tools:` allowlist and
`disallowed-tools:` list; skills carry no tool policy in schema v1. Non-Claude native agent
files do not emit tool lists today. Launchers gate MCP tools from launch-bundle `tools.rules`;
its flat `tools.mcp` list contains permission names only, never the server definitions
installed by this sync lane.

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

**Dependency filters** — a dependency's include filter (`agents`, `skills`,
`only_skills`, `only_agents`) narrows its agents, skills and hooks, never its
MCP servers: every server a dependency makes visible to you is registered
(all of a direct dependency's servers, and only the `exported` ones of a
transitive dependency). `exclude` matches agents and skills only, so there is
currently no way to drop one server from a dependency.

### Remote servers

A server with `url` instead of `command` is a remote server over Streamable HTTP.
Set exactly one of the two; both or neither is a parse error naming the file.

```toml
# mcp/cloudflare/mcp.toml
url = "https://mcp.cloudflare.com/mcp"

# Optional headers: a literal, or a value read from the environment.
# `prefix` (optional, literal) is written before the variable's value.
# A literal containing `${…}` or `{env:…}` is written as-is; Claude, Cursor, Pi and OpenCode
# expand it at runtime, Codex does not.
headers = { "X-Region" = "us", Authorization = { from = "env", var = "DOCS_TOKEN", prefix = "Bearer " } }
```

`args` and `env` belong to `command` servers and `headers` to `url` servers; the
wrong one is ignored with an `mcp-ignored-field` warning. `${MARS_MCP_DIR}` is for `command` servers only: anywhere in a
remote server's `url` or headers (including a `prefix`) it is a parse error, because a remote server has no
directory and the token would reach the harness unreplaced. Header env refs get
the same `mcp-env-missing` check as `env`.

| Target | Remote entry | Env header |
|---|---|---|
| `.claude` | `{"type": "http", "url", "headers"}` | `prefix${VAR}` |
| `.cursor` | `{"url", "headers"}` | `prefix${env:VAR}` |
| `.opencode` | `{"type": "remote", "url", "headers"}` | `prefix{env:VAR}` |
| `.pi` | `{"type": "http", "url", "headers"}` | `prefix${VAR}` |
| `.codex` | `url`, `http_headers`, `env_http_headers`, `bearer_token_env_var` | see below |

**Codex** reads a header from the environment whole (`env_http_headers`, no
prefix). The one prefixed form it has is `bearer_token_env_var`, which Mars writes
for `Authorization` with `prefix = "Bearer "`. Any other header with a prefix has
no Codex spelling: sync warns (`mcp-lossy`) and leaves that header out.

There is no SSE: Codex lists `type = "sse"` as Streamable HTTP, Pi rejects it,
OpenCode has no SSE transport and Claude deprecates it, so `mcp.toml` cannot ask
for it. **OAuth is the harness's**: Mars writes nothing for it, and each harness
starts its own flow the first time a server asks (Claude and Codex also need the
user to approve or log in, as for any project server).

### Importing a Claude plugin's MCP servers

A Claude plugin's servers install like any other: Mars translates the plugin's `.mcp.json`
(with or without the `mcpServers` wrapper) and `plugin.json` `mcpServers` (an inline map, a
path to a JSON file inside the plugin, or a list of those; a `plugin.json` server replaces a
`.mcp.json` server of the same name; manifest entries apply in array order) into
`mcp/<name>/mcp.toml` with `visibility = "local"`. Only a Claude plugin (one with
`.claude-plugin/plugin.json`) is imported: a bare skill tree's `.mcp.json` is usually its
contributors' own, so it is left out with one `import.mcp-not-imported` warning.

| Claude | Imported as |
|---|---|
| `command`, `args` | the same, stdio |
| `env` value exactly `${VAR}` | an env reference to `VAR` |
| `type` `http` or `streamable-http`, literal `url` | a remote server |
| header that is literal, exactly `${VAR}`, or `<text>${VAR}` | a literal header, or an env header with that `prefix` |

A server Mars cannot represent is left out whole, with one warning per offending field, rather than
imported to connect differently from what the plugin declares:

| Warning | Cause |
|---|---|
| `mcp.transport-unsupported` | `sse`, `ws`, `sdk` (or any other `type`) |
| `mcp.url-without-type` | a `url` with no `type`, which Claude treats as invalid |
| `mcp.field-unsupported` | `headersHelper`, `oauth` |
| `mcp.plugin-variable` | `${CLAUDE_PLUGIN_ROOT}`, `${CLAUDE_PLUGIN_DATA}`, `${CLAUDE_PROJECT_DIR}`, `${user_config.*}` |
| `mcp.credential-blocked` | a header that sends `${ANTHROPIC_API_KEY}`, `${ANTHROPIC_AUTH_TOKEN}`, `${AWS_BEARER_TOKEN_BEDROCK}`, `${HTTPS_PROXY}` or `${NPM_TOKEN}`, which Claude reads as empty toward a remote server |
| `mcp.default-value` | `${VAR:-default}` |
| `mcp.interpolation-unsupported` | any other `${...}`, such as one inside `command` or `url` |
| `mcp.literal-env` | an `env` value that is not `${VAR}` |
| `mcp.name-invalid` | a name that cannot be a directory |
| `mcp.invalid-server` | a malformed server |

`mcp.no-servers` marks a valid JSON file with no map of servers; `mcp.manifest-entry-unsupported` skips a `plugin.json` entry that is a `.mcpb` bundle, a URL or
a missing file, and a marketplace entry's own `mcpServers`; `mcp.field-dropped` notes a field
that does not affect how the server connects (`timeout`), and the server still imports. If the
plugin also ships an `mcp/<name>/` directory, its `mcp.toml` is used and the Claude declaration is left
out (`mcp.shadowed-by-authored`). Servers declared in an agent's frontmatter `mcpServers` are
not imported; only their names carry over (`agent.mcp-not-converted`).

### Server files (`${MARS_MCP_DIR}`)

An MCP server directory may ship the program it launches, not just `mcp.toml`:

```
mcp/echo/
  mcp.toml
  server.js
```

Name those files with `${MARS_MCP_DIR}` instead of an absolute path:

```toml
command = "node"
args    = ["${MARS_MCP_DIR}/server.js"]
```

`mars sync` installs the whole `mcp/<name>/` directory inside each target
that takes MCP servers (`.claude/mcp/echo/`, `.codex/mcp/echo/`, `.pi/mcp/echo/`,
and so on) and replaces the token in `command` and `args` with that directory's
absolute path (`/home/you/project/.claude/mcp/echo`), which works from any
subdirectory or git worktree on the machine. A target that takes no MCP servers
(`.agents`) receives no directory. The server registered under a `name` override installs
under the registered name. Executable bits are preserved. Removing the server
from the package prunes the installed directories on the next sync.

Only a server that names the token gets a directory; one whose `command` and
`args` never mention `${MARS_MCP_DIR}` registers exactly as it did before and
copies nothing. Because the registered name becomes one directory under each
target, a token-using server's name must be a single path component: a name
like `github/copilot` is skipped with a warning. A token-free server may use
any name.

A token server's entry therefore holds a machine path. `.claude/`, `.codex/`,
`.pi/` and `.opencode/` are usually gitignored, but `.mcp.json` at the project root
is not: a team that commits it should expect per-machine diffs for such
servers, or gitignore `.mcp.json`.

If something Mars does not own already occupies an installed path, sync warns,
leaves that content untouched, and withholds the target's config entry so
nothing points at files Mars did not install. `mars sync --force` overwrites
instead. A directory Mars installed but which was edited afterwards is kept
with a `target-divergent` warning and also withholds the entry until
`mars sync --force` or `mars repair` resets it.

`mars build plugin` copies a token-using server's `mcp/<name>/` into each
generated tree and maps the token to the tree's own root:
`${CLAUDE_PLUGIN_ROOT}/mcp/<name>` for the shared Claude/Codex/Grok tree,
`${CURSOR_PLUGIN_ROOT}/mcp/<name>` for Cursor, and a runtime-resolved path under
the OpenCode package for OpenCode. An entry without the token is unchanged and
ships no directory.

The token applies to `command` and `args`. `env` values remain symbolic
`{ from = "env", var = "…" }` references; Mars never resolves their values.

## Declaring Hooks in a Package

A hook directory contains `hook.toml`, one native fragment per declared target,
and any scripts or assets it needs:

```
my-package/hooks/audit/
  hook.toml
  claude.json
  codex.json
  cursor.json
  opencode.ts
  pi.ts
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

[targets.".cursor"]
fragment = "cursor.json"    # optional; this is the default filename
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

Cursor uses a flat native entry shape. This is a complete, copy-pasteable
Cursor fragment—do not put a Claude-style nested `hooks` array inside an entry:

```json
{
  "version": 1,
  "hooks": {
    "beforeShellExecution": [
      {
        "command": "bash \"${MARS_HOOK_DIR}/run.sh\"",
        "matcher": "^git ",
        "timeout": 5
      }
    ]
  }
}
```

Mars owns the emitted `{"version": 1, "hooks": {...}}` wrapper in
`.cursor/hooks.json`. The schema and 21 camelCase event names were confirmed
against the `cursor-agent` 2026.07.16 validator. Runtime firing remains
unverified because the probe could not authenticate.

Only declared target tables receive the hook. Mars copies the whole authored
directory to `<target>/hooks/<name>/`; `hook.toml` and fragment files remain
there as inert package metadata. Merge-mode destinations are Claude
`settings.local.json`, Codex `hooks.json`, and Cursor `hooks.json`.

If something Mars does not own already occupies `<target>/hooks/<name>/`, sync
warns, leaves that content untouched, and withholds the target's hook entry so
nothing points at files Mars did not install. `mars sync --force` overwrites
instead. A directory Mars installed but which was edited afterwards is kept
with a `target-divergent` warning and also withholds the entry until
`mars sync --force` or `mars repair` resets it.

Mars validates only the merge contract:

1. the fragment is valid JSON;
2. its top-level event names appear in the target allowlist;
3. every event value is an array.

Nested matcher, handler, timeout, and harness-specific fields pass through
unchanged. Claude currently has 29 known events, Codex has 10, and Cursor has
21. Codex
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

### OpenCode and Pi file fragments

OpenCode and Pi consume opaque files rather than JSON event maps. Point their
target table at any fragment filename (a `.ts` file is expected); Mars performs
textual `${MARS_HOOK_DIR}` substitution and places it under a managed name:

```toml
[targets.".opencode"]
fragment = "opencode.ts"

[targets.".pi"]
fragment = "pi.ts"
```

Self-contained OpenCode plugin fragment for `mars build plugin`:

```ts
import { spawnSync } from "node:child_process"

const SCRIPT = "${MARS_HOOK_DIR}/run.sh"

export default {
  id: "audit",
  setup(ctx) {
    ;(async () => {
      for await (const event of ctx.event.subscribe()) {
        if (event.type === "session.execution.succeeded") {
          spawnSync("bash", [SCRIPT], { cwd: ctx.location.directory })
        }
      }
    })()
  },
}
```

OpenCode fragments must use a default `{ id, setup }` export. Mars warns when a fragment has no
recognizable default export but still installs it; plugin builds isolate each fragment so one load
or setup failure does not block later hooks. Mars places synced fragments at
`.opencode/plugins/mars-<name>.ts`.

These placed fragments are managed target outputs. Mars preserves an existing
file at that path during ordinary sync when the lock holds no installed-content
claim for it; use `mars sync --force` to adopt it. Mars also preserves later edits during ordinary sync and removes a
fragment only when the lock owns that exact target path.

Self-contained Pi extension:

```ts
import type { ExtensionAPI } from "@mariozechner/pi-coding-agent"

const SCRIPT = "${MARS_HOOK_DIR}/run.sh"

export default function (pi: ExtensionAPI) {
  pi.on("tool_call", async () => {
    await pi.exec("bash", [SCRIPT])
  })
}
```

Mars places it at `.pi/extensions/mars-<name>.ts`. File fragments receive no
content or event validation; only file existence and substitution are checked
in preflight. `unchecked` is therefore meaningless and rejected for file-mode
targets, even when set to `false`.

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

The trust hash covers the hook definition, not the referenced script contents.
Editing a script therefore runs the new code under the existing approval,
without another trust prompt. Review script changes with the same care as hook
definition changes even though Codex does not ask again.

File-mode placement ignores `order`: OpenCode and Pi load discovered filenames,
so managed plugin ordering is alphabetical by `mars-<name>.ts`. An `order`
value still affects the same hook's merge-mode targets but does not rename or
reorder its file-mode artifact.

### Ownership and removal

For each `hook:<Event>:<name>` key, `mars.lock` stores the exact emitted entry
array after path substitution. Before writing replacements, Mars removes
structurally equal entries from the current config. User-edited entries no
longer match and are preserved; user-authored entries and unrelated config are
always untouched. Event keys emptied by Mars are pruned.

For file-mode targets, the hook item's output list records the exact target
root, placed `mars-<name>.ts` path, and installed checksum. Removal deletes
only that exact lock-owned output and leaves every other plugin or extension
untouched.

For one release, removal also recognizes v0.11.0 command-path entries containing
`/hooks/<name>/`, including staging-path commands. This migration sweep runs
before fragment writes and shares the next-release deletion ledger with the
existing OpenCode and legacy-Codex residue sweeps.

## Collision Resolution

When two packages declare an MCP server or hook with the same name for the same
target, Mars resolves the collision deterministically:

**For MCP servers**, collision identity is the server name + target root.
**For merge-mode hooks**, collision identity is `(event, name)` + target root —
hooks with the same name on different events are distinct and both install.
**For file-mode hooks**, collision identity is the hook name + target root.

**Precedence rules (highest to lowest):**

1. **Shallower package wins** — the local package (`_self`) beats direct dependencies,
   and a direct dependency beats its transitive dependencies. A local winner is silent;
   dependency collisions emit a warning naming both sources.

2. **Earlier declaration order wins at equal depth** — when two dependencies at
   the same depth declare the same name, the one reached through the earlier
   `[dependencies]` entry wins and the later one is dropped.

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
string, including the shell and quoting. Mars substitutes the absolute
`${MARS_HOOK_DIR}` value and normalizes that substituted directory's separators
to `/`, keeping JSON and TypeScript string literals escape-safe. Author commands
for every platform the package claims to support; Mars does not synthesize a
Windows command branch.

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
