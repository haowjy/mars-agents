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
- **Hooks** are written once in Claude's hook format (`[hooks]` in `hook.toml`)
  and installed natively in Claude, Codex, OpenCode and Pi, or contributed as
  target-native fragments (JSON for Claude, Codex and Cursor; TypeScript for
  OpenCode and Pi). Their complete hook directories install beside the target.
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

**Using `[hooks]` in a package.**

- Set `requires-mars = ">=0.25.0"` in the package's [`mars.toml`](mars-toml.md). A Mars older
  than 0.25 rejects `[hooks]` and fails the whole sync, not just your hook, and
  `mars check` warns when the requirement is missing. A path or branch
  dependency has no version to fall back on, so a too-old Mars hard-errors on it.
- A plugin package that moves an `opencode.ts` fragment into `[hooks]` loses its
  hook in the OpenCode plugin tree built by `mars build plugin`; the tree
  reports `plugin-hook-fragment-dropped`. Keep the `opencode.ts` fragment for
  the plugin tree until the tree can load generated hooks.
- Mars now reads a dependency's `hook.toml` leniently (see
  [Validation by origin](#validation-by-origin)): an unknown key or event in a
  dependency warns instead of failing the sync. Your own project's hooks stay
  strict.
- Claude Code before 2.1.295 ignores `onFailure`, and a guard written with
  `onFailure = "block"` then fails open there.

**From Mars 0.23 or earlier.** Those releases wrote Claude servers to
`.claude/.mcp.json` and Codex servers to `.codex/codex_mcp.json`, which neither harness
read, and recorded MCP servers and old hooks in `mars.lock` by name, without
`emitted_json`. Mars 0.25 no longer reads them or those files. It never fails the
sync over them: it forgets the records, removes nothing, and warns
(`lock-config-entries-dropped`) once, naming each entry. Delete leftovers by hand:
the entries in those two files, `.codex/codex_hooks.json`, and the `hooks` object in
`.opencode/opencode.json`.

**From Mars 0.11 or earlier.** Those releases wrote a version 2 `mars.lock`, which Mars
0.25 reads for its dependencies only (`lock-v2-dropped`): installed files count as local
content, so the first sync warns about each and `mars sync --force` adopts them,
overwriting any local edits at those paths. Syncing once with any Mars from 0.12 to 0.24
first migrates the lock cleanly.

- A Cursor entry an earlier release wrote is now treated as yours. If it differs
  from what Mars writes, Mars keeps it and warns (`mcp-unmanaged-collision`) until
  `mars sync --force` replaces it.
- `.mcp.json` at the project root is new: a project that does not ignore it sees a
  new untracked file.
- When Mars removes its last entry and the MCP file holds nothing else, Mars deletes
  the file. A file with other content, including only comments, stays.

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
| `env` value exactly `${VAR}`, or a name in `env_vars` (a Mars-built plugin's same-name references) | an env reference to `VAR` |
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
| `mcp.credential-blocked` | a header that sends an environment variable Claude does not send to a remote server: its cloud, registry and CI credentials, and their prefix families (Anthropic, Claude Code, AWS, Azure, Google, Vault, OpenTelemetry), matched case-insensitively. Mars refuses a superset of Claude's set, so the server would otherwise connect differently |
| `mcp.default-value` | `${VAR:-default}` |
| `mcp.interpolation-unsupported` | any other `${...}`, such as one inside `command` or `url` |
| `mcp.literal-env` | an `env` value that is not `${VAR}` |
| `mcp.name-invalid` | a name that cannot be a directory: letters, digits, `-`, `_` and `.`, starting with a letter or digit. Sync reports the same code for a server that installs its own directory (`${MARS_MCP_DIR}`) under another name |
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

Write a hook once, in the shape Claude Code uses for `"hooks"` in
`settings.json`, expressed as TOML. Mars installs it natively in Claude Code,
Codex, OpenCode and Pi. A hook directory holds `hook.toml` and the script it
runs:

```
my-package/hooks/protect-tmux-server/
  hook.toml
  run.sh
```

```toml
# hooks/protect-tmux-server/hook.toml
visibility = "exported"       # dependencies must export hooks
order = 0                     # deterministic merge tiebreaker; default 0
skip-targets = []             # optional; targets that get no generated hooks

[[hooks.PreToolUse]]
matcher = "Bash"

[[hooks.PreToolUse.hooks]]
type = "command"
command = 'bash "${MARS_HOOK_DIR}/run.sh"'
if = "Bash(*tmux*)"           # Claude-only filter; see below
onFailure = "block"           # fail closed on Claude; see below
timeout = 10

[[hooks.SessionStart]]

[[hooks.SessionStart.hooks]]
type = "command"
command = 'bash "${MARS_HOOK_DIR}/context.sh"'
timeout = 10
```

```bash
# hooks/protect-tmux-server/run.sh: reads Claude's JSON on stdin
if grep -q 'kill-server' <&0; then
  echo "tmux kill-server would kill every session on this machine" >&2
  exit 2                      # exit 2 = block, with the reason on stderr
fi
```

`[hooks]` is Claude's `"hooks"` object. Each `[[hooks.<Event>]]` is one matcher
group; each `[[hooks.<Event>.hooks]]` is one handler in it. Event and field
names are Claude's, camelCase included (`onFailure`, `statusMessage`). A Claude
hooks JSON converts to this TOML and back mechanically. The one exception is
JSON `null`, which TOML cannot hold: drop it.

A hook needs `[hooks]`, a [target table](#native-fragments), or both. Mars
copies the whole directory to `<target>/hooks/<name>/` for each target that
receives the hook, so the scripts sit beside the generated entry.

**Events.** The event set is Claude's. An event Mars does not know is an error
in your own project's hooks. A hook may set `unchecked = true` at the top level
to let unknown events through to Claude only; the other targets report them
skipped. A dependency's unknown events are skipped with a warning (see
[Validation by origin](#validation-by-origin)).

**Handlers.** Mars knows `type`, `command`, `timeout`, `if`, `onFailure`,
`statusMessage` and `async`, and passes any other handler or matcher-group field
to Claude. The other targets get only what their parser accepts. A handler whose
`type` a target cannot run is left out on that target and reported: Codex runs
`command` and `mcp_tool`; OpenCode and Pi run `command` only.

### Which harness gets what

"Seen" means the row was observed working on that version. "Docs" means read
from the harness's documentation or source, not run. Versions: Claude Code
2.1.312, Codex 0.164.0, OpenCode 2.2.0, Pi 1.4.0.

| Event | Claude Code | Codex | OpenCode | Pi |
|---|---|---|---|---|
| `PreToolUse` | seen | seen | seen (`execute.before`) | seen (`tool_call`) |
| `PostToolUse` | docs | docs | seen (`execute.after`) | seen (`tool_result`) |
| `PostToolUseFailure` | docs | not rendered: Codex fires `PostToolUse` instead | partial: a shell that exits non-zero reports success, so the hook runs for tool errors only | seen, including a failing shell |
| `UserPromptSubmit` | docs | docs | seen; a block shows a generic error, not your reason | seen; a block is silent in print mode |
| `SessionStart` | docs | docs | skipped: no per-session hook | seen; `startup`, `resume`, and `new` (reported to your script as `clear`) |
| `Stop` | docs | docs | skipped | seen (keep going) |
| `PreCompact` | docs | docs | runs, its answer ignored (seen); a group with a matcher is skipped | seen (block cancels) |
| `PostCompact` | docs | docs | skipped | seen |
| `SessionEnd` | docs | seen | skipped | seen |
| `PermissionRequest` | docs | docs | seen, but only when the permission is `ask` (see below) | skipped: no permission system |
| `SubagentStart`, `SubagentStop` | docs | docs | skipped | skipped |
| `Notification` and the other Claude events | docs | skipped | skipped | skipped |

OpenCode's `PermissionRequest` hook runs only when the permission is `ask`, as
Claude shows the hook only before a dialog. Set `permission: {shell: "ask"}` in
`opencode.json` to see it; under `--auto` or an `allow` rule OpenCode never asks,
so the hook does not run. Pi reports `new` to your script as `clear`; its `reload`
and `fork` reasons pass through unchanged and match no Claude `source`. Pi can emit
`session_start` twice when a session is replaced; Mars runs your script once per
session id. An OpenCode `PreCompact` event carries no `trigger`, so a group with a
real matcher cannot be tested and is left out.

A target that cannot render an event reports `hook-event-skipped` at sync.
Cursor receives no generated entries (Mars renders none for it before 0.26) and
reports that once per hook; a native Cursor fragment still works (see
[Native fragments](#native-fragments)). Codex leaves out a matcher group whose
matcher its regex engine cannot compile, such as one with look-around
(`(?!…)`), and reports it; Claude's JavaScript regex accepts those.

On OpenCode and Pi the matcher tests the tool name in Claude's spelling. Mars
translates the native names and inputs:

| Claude | OpenCode 2.2.0 | Pi 1.4.0 | Input your script sees |
|---|---|---|---|
| `Bash` | `shell` | `bash` | `tool_input.command` |
| `Read` | `read` | `read` | `tool_input.file_path` (absolute) |
| `Write` | `write` | `write` | `file_path`, `content` |
| `Edit` | `edit` | `edit` | `file_path`, `old_string`, `new_string` |
| `Edit` | `patch` | — | `command` (the patch text, no `file_path`) |

Other tools keep their native name, so a `*` matcher still sees them. Pi's
`powershell` tool is not mapped. On Codex, `apply_patch` is reported as
`Write`/`Edit`, and `tool_input.command` holds the patch text, not a
`file_path` (Codex 0.164.0); OpenCode's `patch` tool arrives the same way, as
`Edit`. An `Edit|Write` hook on Codex or OpenCode must parse the patch.

### What a script may answer

A script reads Claude's JSON on stdin: `hook_event_name`, `session_id`, `cwd`
and the event's own fields (`tool_name`, `tool_input`, `prompt`, `source`,
`stop_hook_active`). Exit 0 with no output means no decision. Exit 2 with a
reason on stderr is the event's blocking answer.

Anything printed as JSON on stdout (with exit 0) must use only these fields,
which every target understands:

| Event | Portable answers |
|---|---|
| `PreToolUse` | exit 2, or `hookSpecificOutput.permissionDecision: "deny"` with `permissionDecisionReason` |
| `PostToolUse`, `PostToolUseFailure` | `hookSpecificOutput.additionalContext` |
| `UserPromptSubmit` | exit 2, or `decision: "block"` with `reason`; `hookSpecificOutput.additionalContext` |
| `SessionStart` | `hookSpecificOutput.additionalContext` |
| `Stop` | exit 2, or `decision: "block"` with `reason` (keep going) |
| `PreCompact` | exit 2 (block) |
| `PermissionRequest` | `hookSpecificOutput.decision.behavior: "allow"` or `"deny"` with `message` |
| `SessionEnd`, `PostCompact`, others | none; the hook runs only |

Stay inside this table. Codex parses every answer strictly: one field it does
not know (`sessionTitle`, `updatedToolOutput`, `additionalContext` on `Stop`)
makes it drop the whole answer and run the action (Codex 0.164.0). Mars cannot
check what a script prints at runtime. Answers outside the table work on Claude,
and on Codex where Codex has them, but Mars does not translate them for OpenCode
and Pi.

### `if`, `onFailure`, and failing open

`if` is a best-effort filter in Claude's permission-rule syntax
(`Bash(git *)`). Claude applies it to tool events only; on any other event,
Claude never runs a handler that has `if`, and `mars check` warns. Codex ignores
`if` and runs the script on every `matcher` match, so the script must decide for
itself. OpenCode and Pi leave out an `if` on a non-tool event, as Claude never
runs such a handler, and report it. On tool events they evaluate a superset of Claude's check: the tool must
match and every literal part of the glob must appear in the command or path, so
the script never runs less often than on Claude. Their default `timeout` is 600
seconds for every event; set `timeout` explicitly on a guard.

`onFailure = "block"` turns a failure (cannot start, timeout, an exit other than
0 or 2) into the event's blocking answer. The default, `"continue"`, lets the
action run. Where it works:

| Harness | A script that crashes or times out |
|---|---|
| Claude Code 2.1.312 | blocks with `onFailure = "block"`, otherwise runs the action. Claude before 2.1.295 ignores the field and fails open |
| Codex 0.164.0 | always runs the action: no `onFailure`, and a timeout fails open |
| OpenCode 2.2.0, Pi 1.4.0 | blocks with `onFailure = "block"`, including a timeout |

If a guard must hold, do not rely on a crash to stop the action: on Codex, only
an explicit exit 2 or a `deny` answer blocks.

### Tokens

Mars replaces these in every command:

- `${MARS_HOOK_DIR}`: the absolute installed hook directory.
- `${MARS_HOOK_TARGET}`: the target root the entry is written for: `.claude`,
  `.codex`, `.opencode` or `.pi`. In a `mars build plugin` Claude tree, which
  several harnesses read, it is `plugin`.

Quote the path in the command (`bash "${MARS_HOOK_DIR}/run.sh"`) so a project
path with spaces works. Mars adds no shell or quoting wrapper.

### Choosing targets: `skip-targets`

A hook with `[hooks]` goes to every managed target that can render at least one
of its events, except the targets in `skip-targets`. Entries must be target
roots such as `".codex"`. A skipped target gets no generated entries and no
`hook-event-skipped` reports. `skip-targets` turns off only the generated
entries: a [native fragment](#native-fragments) for that target still installs.

```toml
skip-targets = [".codex"]   # [hooks] generates nothing for Codex
```

### Native fragments

`[hooks]` and native fragments can live in one hook. A fragment is the
harness-native file for one target, declared in a target table:

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

```toml
# hooks/audit/hook.toml
name = "audit"              # optional; defaults to the directory name
visibility = "exported"
order = 10

[targets.".claude"]
fragment = "claude.json"    # default filename when the hook has no [hooks]

[targets.".codex"]
fragment = "codex.json"
```

How fragments combine with `[hooks]`:

- **Claude and Codex (merge targets).** A fragment replaces the generated
  entries only for the events it declares; the other events still render from
  `[hooks]`. A fragment that declares an event as an empty array
  (`"PreToolUse": []`) turns that event off on that target. Sync reports
  `hook-fragment-overrides`.
- **OpenCode and Pi (file targets).** Mars cannot read events out of a
  TypeScript file, so the fragment installs beside the generated file
  (`mars-<name>.ts` next to `mars-<name>.hooks.ts`) and sync reports
  `hook-fragment-beside-generated`. To replace the generated hook there, put
  that target in `skip-targets`. Hook names may not end in `.hooks`.
- When a hook has `[hooks]`, every target table must name `fragment`
  explicitly; an empty table is an error, because it would pick up a leftover
  `claude.json`. A hook without `[hooks]` keeps the old rule: the tables select
  targets, and an empty table on a merge target means `<target>.json`. An OpenCode
  or Pi table always names its `fragment`: it is a TypeScript file, and
  `opencode.json` would otherwise be installed as a plugin.

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

A hook with no `[hooks]` goes only to the targets with a declared table. Mars
copies the whole authored
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
unchanged in a fragment. Claude currently has 29 known events and Cursor has
21. Codex's allowlist holds the 11 events it shares with Claude (`SessionStart`,
`UserPromptSubmit`, `PreToolUse`, `PermissionRequest`, `PostToolUse`,
`PreCompact`, `PostCompact`, `SubagentStart`, `SubagentStop`, `Stop`,
`SessionEnd`) plus its own `PreModelRequest`, `PostModelRequest` and
`Interrupt`, which pass in a Codex fragment only with `unchecked = true`. `SessionEnd` fires on Codex 0.164.0;
it did not fire on 0.144.4.
To use a newly shipped native event before Mars updates its
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

Mars places it at `.pi/extensions/mars-<name>.ts`. Generated hooks from `[hooks]`
go to `mars-<name>.hooks.ts` in the same directories. File fragments receive no
content or event validation; only file existence and substitution are checked
in preflight. `unchecked` is therefore meaningless and rejected for file-mode
targets, even when set to `false`.

### Validation by origin

Before 0.25, `hook.toml` rejected unknown keys, so a Mars older than 0.25 fails
the whole sync on `[hooks]`. From 0.25 on, validation depends on who wrote the
hook:

- **Your project's own hooks are strict.** An unknown key, unknown event or
  invalid table is an error, because you can fix it. `unchecked = true` on a
  file target is an error whichever targets are selected.
- **A dependency's hooks never fail your sync.**
  - An unknown top-level key is ignored with a warning.
  - An unknown event is skipped with a warning; the hook's other events still
    render.
  - Any other validation failure keeps that hook's installed entries and files
    as they were (the last good install) and warns, naming the package and
    version (`hook-invalid-kept`).

A guard is therefore never removed because a newer package met an older Mars.
Fields inside `[hooks]` handlers and matcher groups always pass through.

### Ordering and Codex trust

**Codex runs a project's `.codex/hooks.json` only in a trusted project.** Mars
writes the file but cannot trust the project; trust it once in Codex. For a
scripted run (Codex 0.164.0), pass the project as an inline table:

```bash
codex exec -c 'projects={"/path/to/project"={trust_level="trusted"}}' "…"
```

The dotted form `-c 'projects."/path".trust_level="trusted"'` trusts nothing on
0.164.0: Codex splits the override at every `.`, including the ones inside the
quoted path. `--dangerously-bypass-hook-trust` skips only the handler review
below, after Codex has found the hooks, so it does not make an untrusted
project load them.

Mars sorts contributions by package depth, dependency declaration order,
`order`, and hook name, preserving each fragment array's internal order. It
appends the managed block after user-authored entries. This guarantees stable
placement, not execution order—Claude runs matching hooks in parallel.

Within a trusted project, Codex also reviews each new or moved handler in its
`/hooks` screen and skips it until you approve (`codex-hooks-need-trust`, below).
Trust is indexed by hook file, event, and array position. Adding or
reordering managed hooks can shift indices, so Codex may silently skip affected
hooks until you re-trust them with `/hooks`. This re-trust churn is an accepted
Codex behavior; deterministic ordering minimizes it but cannot prevent it when
a new hook sorts ahead of an existing one. Seen on Codex 0.164.0: inserting a
new `PreToolUse` entry ahead of a trusted one left both skipped, silently, until
reviewed again. Mars cannot grant trust. After a sync that changes the position
or content of anything in `.codex/hooks.json`, user entries included, it warns
(`codex-hooks-need-trust`); open `/hooks` in Codex and review. A hook skipped
for lack of trust does not run, so a Codex guard is not enforced until then.

The trust hash covers the hook definition, not the referenced script contents.
Editing a script therefore runs the new code under the existing approval,
without another trust prompt. Review script changes with the same care as hook
definition changes even though Codex does not ask again.

File-mode placement ignores `order`: OpenCode and Pi load discovered filenames,
so managed plugin ordering is alphabetical by `mars-<name>.ts`. An `order`
value still affects the same hook's merge-mode targets but does not rename or
reorder its file-mode artifact.

### What sync reports

| Code | Level | When |
|---|---|---|
| `hook-event-skipped` | warning for your own hooks and in `mars check`; info (one summary per sync) for dependencies' hooks | an event, handler type or matcher field has no counterpart on a managed target, whether or not the hook was selected there |
| `hook-answer-unsupported` | info | the event renders, but some portable answers do not carry over to that target |
| `hook-fragment-overrides` | info | a Claude or Codex fragment declares an event that `[hooks]` also declares |
| `hook-fragment-beside-generated` | info | an OpenCode or Pi fragment installs next to the generated file for the same hook |
| `hook-event-unchecked` | warning | `unchecked = true` passes an unknown event through to a target |
| `hook-fragment-contract` | warning | an OpenCode or Pi fragment lacks the expected default export; it installs anyway |
| `hook-fragment` | warning or error | a fragment could not be read or rendered in a plugin build (an error for the project's own hook, a warning for a dependency's) |
| `hook-invalid-kept` | warning | a dependency's hook failed validation; its last good install stays, or nothing installs if none exists |
| `codex-hooks-need-trust` | warning | `.codex/hooks.json` has new or moved handlers Codex will want reviewed |
| `plugin-hook-fragment-dropped` | warning | `mars build plugin`: a hook's entries do not reach a tree |

With `--json`, these appear in `diagnostics[]`. A hook whose directory was not
installed gets no entries (`hook-not-installed`).

### Plugin builds

`mars build plugin` renders `[hooks]` through the same code. The Claude tree,
which Claude, Codex and Grok read, gets a `hooks/hooks.json` reduced to what
Codex can parse, with drops reported. The OpenCode and
Cursor trees drop `[hooks]`: for each hook that has no fragment for that tree, sync
reports `plugin-hook-fragment-dropped`, naming the harnesses that won't get it.
A fragment for another target also reports it. A hook generated for the OpenCode
tree is tracked in [mars-agents#290](https://github.com/haowjy/mars-agents/issues/290).

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

Removal matches recorded entries only. Command-path sweeps for hook records that
carry no emitted entry (before v0.12.0) are gone; see
[Upgrading](#upgrading) for what Mars does with such records.

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
