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
requires-mars = ">=0.19.0"

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

## MCP server files

A server may ship its own files beside `mcp.toml` and name them with
`${MARS_MCP_DIR}`:

```toml
# mcp/echo/mcp.toml
command = "node"
args    = ["${MARS_MCP_DIR}/server.js"]
```

Each tree receives the whole `mcp/echo/` directory. The Claude/Codex/Grok tree
writes `${CLAUDE_PLUGIN_ROOT}/mcp/echo/server.js`, the Cursor tree writes
`${CURSOR_PLUGIN_ROOT}/mcp/echo/server.js`, and the OpenCode tree keeps
`${MARS_MCP_DIR}` in `mcp.json` so its `index.mjs` can resolve the path at
runtime. Entries that do not use the token are written unchanged.

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

- `[package].requires-mars` does not require Mars 0.19.0 or newer;
- plugin-only package or `[plugin]` metadata is invalid, including an unknown harness;
- generated output records a different package version.

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
