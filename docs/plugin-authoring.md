# Authoring Harness Plugins

Mars can turn a publishable agent package into committed plugin artifacts for Claude,
Codex, Grok, Cursor, and OpenCode. The generated files contain the package's published
items plus the dependency graph selected by its filters, renames, and visibility rules.

The README records the generated [component support](../README.md#component-support),
[verification levels](../README.md#verification-level), and hand-written
[install commands](../README.md#install-commands).

## Configure the plugin

A plugin package needs `[package]` and `[plugin]` tables:

```toml
[package]
name = "example-agents"
version = "1.0.0"
description = "Example agents and skills"
authors = ["Example Maintainer <maintainer@example.com>"]
homepage = "https://github.com/example/example-agents"
license = "MIT"
schema = 1
requires-mars = ">=0.20.0"

[plugin]
dir = "plugin"
name = "example-agents"
marketplace = "example-agents"
description = "Example agents and skills"
harnesses = ["claude", "codex", "grok", "cursor", "opencode"]
```

All `[plugin]` fields are optional. The output directory defaults to `plugin`, identity
and marketplace default to `[package].name`, description defaults to the package
description, and every harness Mars supports is selected by default. A harness added in a later Mars release joins the default, so a package without `harnesses` starts building for it on its next release; list `harnesses` to opt out. See the
[`mars.toml` reference](config/mars-toml.md#plugin-optional) for field and naming rules.

Build the configured output with:

```bash
mars build plugin
```

Mars writes up to three trees under the configured directory:

```text
plugin/
  claude/    # Claude, Codex, Grok
  cursor/    # Cursor
  opencode/  # OpenCode
  .mars-plugin.json
```

A configured build also writes the Claude and Cursor marketplace catalogs and the root
OpenCode `package.json`. Mars records ownership and file hashes in the sentinel; it
refuses to replace an unowned non-empty output or conflicting root file.

For CI or inspection, `mars build plugin PATH --out <dir>` writes only the owned nested
tree and sentinel. It does not write package-root catalogs or `package.json`, and it can
build a package without `[plugin]`.

## OpenCode agents

The OpenCode tree registers each published agent through the plugin `ctx.agent` API as
`<plugin>:<agent>`, lowered the way `mars sync` lowers `.opencode/agents/<name>.md`. OpenCode's
agent record has no skills field, so an agent's `skills.load` list reaches it as body guidance; the plugin's skills are available to it by
OpenCode's default allow, unless the agent's own `permission` denies them. Permission keys are
translated to the plugin API's spellings (`bash` → `shell`, `task` → `subagent`, `write` and
`patch` → `edit`). The agent's `mode` defaults to `all` so the `subagent` tool can spawn it.

## MCP server files

A server may ship its own files beside `mcp.toml` and name them with
`${MARS_MCP_DIR}`:

```toml
# mcp/echo/mcp.toml
visibility = "exported"
command = "node"
args    = ["${MARS_MCP_DIR}/server.js"]
```

As with hooks, only an `exported` server reaches consumers, so a plugin build
ships nothing for a `local` one.

Each tree receives the whole `mcp/echo/` directory for a server that names the
token. The Claude/Codex/Grok tree writes
`${CLAUDE_PLUGIN_ROOT}/mcp/echo/server.js`, the Cursor tree writes
`${CURSOR_PLUGIN_ROOT}/mcp/echo/server.js`, and the OpenCode tree keeps
`${MARS_MCP_DIR}` in `mcp.json` so its `index.mjs` can resolve the path at
runtime. Entries that do not use the token are written unchanged and ship no
directory.

## Remote MCP servers

A server with `url` (and optional `headers`) instead of `command` ships as a remote
entry: `type: http` in the Claude/Codex/Grok tree, `url` and `headers` in the Cursor
tree, `type: remote` in the OpenCode `mcp.json`. It has no directory to install.
An env header becomes `prefix${VAR}` (`prefix{env:VAR}` for OpenCode, whose
`index.mjs` resolves it at runtime and drops a header whose variable is unset).
Codex 0.164.0 passes `${VAR}` in the shared Claude tree literally, so the build warns
once per env header when Codex is a target.

## MCP environment variables

The shared Claude tree is consumed by Claude, Codex, and Grok. A same-name pass-through:

```toml
[env]
TOKEN = { from = "env", var = "TOKEN" }
```

becomes `"env_vars": ["TOKEN"]`, which all three verified binaries accept. A renamed
mapping such as `TOKEN = { from = "env", var = "OTHER_TOKEN" }` remains
`"env": {"TOKEN": "${OTHER_TOKEN}"}` for Claude compatibility. Codex 0.164.0 passes
that value literally, so the plugin build warns once for the affected server.

## Check before release

Run `mars check` before building or releasing. In addition to source validation, it
reports configured plugin output that is missing, stale, hand-edited, skipped, or in
conflict with a package-root generated file. It also warns when:

- `[package].requires-mars` allows a Mars older than 0.20.0, the first that reads
  `schema = 1` (0.19.0 for a plugin package with no `schema`, the first that keeps
  `[plugin]`);
- plugin-only package or `[plugin]` metadata is invalid, including an unknown harness;
- generated output records a different package version;
- generated output was built by a different Mars version, which may write different files
  for the same inputs.

`mars check` does not rebuild output. Use `mars build plugin`, review the resulting diff,
and run the check again.

## Release

For package repositories, release with:

```bash
mars version patch
# or: mars version minor | major | X.Y.Z
```

When `[plugin]` is configured, `mars version` stages the plugin at the new version,
updates `mars.toml` and `CHANGELOG.md`, publishes the generated directory and root files,
and commits them together. A failed build or release step rolls the touched paths back.
Use `--no-plugins` only as an escape hatch when intentionally releasing without rebuilding
configured plugin output; `mars check` will continue to report stale or skipped output.

After pushing a plugin-carrying release, use the README's
[install commands](../README.md#install-commands) to verify remote installation.
