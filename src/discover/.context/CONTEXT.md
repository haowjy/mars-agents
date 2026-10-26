# src/discover/ — Source Item Discovery

Discovers binary-owned hooks and bootstrap documents. `mars-schema` is the authority for Agent and skill selection; the resolver replaces any Agent/skill observations from this compatibility walk with `Prepared.definitions`.

## Contract

Discovery is convention-based. A single bounded recursive walk starts at the rooted package directory and scans any directory named, up to `MAX_DISCOVERY_WALK_DEPTH = 5`:

- `agents/` for `*.md` agent profiles
- `skills/` for child directories containing `SKILL.md`
- `bootstrap/` for child directories containing `BOOTSTRAP.md`

The filesystem walk remains for hook/bootstrap policy and foreign plugin declarations. Do not add Agent or skill semantics here; crate preparation owns those definitions.

Hooks are the exception to the layered walk: only direct children of the
package-root `hooks/` directory are discovered. The compiler and general item
discovery share that directory enumerator, so a hook cannot be listed and then
fail resolution because a second scanner searched a different root.
During target construction, strict hook parsing supplies the corresponding
`DiscoveredItem`s back to the convention walk, preserving grounding and collision
checks without parsing `hook.toml` twice.

After convention scanning, discovery is globally grounded to the shallowest logical layer that contains convention items. Agents, skills, and bootstrap docs are treated as one package layer: if `skills/foo` exists at the package layer, deeper `examples/skills/bar` or vendored nested containers are ignored. If the only convention items are nested, that nested layer becomes the grounded package layer and is still discovered.

## Current-package self inputs

`local_source` opts into this walk only when the project declares `[package]`,
then selects agents/skills after grounding (other convention kinds still affect
the occupied layer). `.mars-src` is discovered separately and wins matching
`(kind, name)` definitions before staging. Removing shallower items can promote
nested distribution items; there is no permanent `cw/` exclusion.

Directories containing a valid `.mars-plugin.json` are generated plugin trees and are
excluded at any depth, including hook and MCP discovery, with one info diagnostic per skipped
path. A valid package-relative `[plugin].dir` is a stronger source boundary: discovery excludes it
by path without reporting it, even before a sentinel exists, so the package's own committed output
never adds noise to `sync` or `check`. Canonical staging applies the sentinel boundary before any
downstream copy or hash; those lower-level primitives remain byte-faithful. The authored-tree
filesystem walk also protects local-source installs that bypass whole-package staging. Other
configured non-hidden output trees remain discoverable; use hidden targets for those authored-root
flows.

## Hidden directories

The walk skips dot-prefixed directories at every descent step. This keeps generated harness outputs and control/cache directories (`.claude/`, `.codex/`, `.cursor/`, `.opencode/`, `.git/`, `.mars/`) out of default package discovery without maintaining a harness blocklist. These directories are local execution or output surfaces, not package discovery sources.

A hidden foreign layout is still importable when the dependency explicitly roots the package there, for example:

```toml
[dependencies.foreign]
path = "../foreign-package"
subpath = ".claude"
dialect = "claude"
```

With that rooting, `agents/` and `skills/` are non-hidden children of the effective package root and are discovered normally.

## Root `SKILL.md`

A package-root `SKILL.md` is a flat-root skill fallback only when no agents or skills were found by convention. Bootstrap docs can still accompany a flat-root skill through explicit plugin-manifest declarations.

## Manifest declarations

Claude plugin manifests (`.claude-plugin/plugin.json` and `marketplace.json`) are explicit declarations, not auto-scanning. Their `./` paths are normalized, checked to stay under the package root, and then scanned/registered according to the declared item kind.

Manifest-declared items and convention items share one duplicate rule: if two discovered items have the same `(kind, name)`, discovery raises `DiscoveryCollision`. There is no precedence order or silent shadowing inside a single source.

## Rationale

The bounded convention walk replaced the earlier two-path discovery model: fixed top-level conventions plus a heuristic BFS with hardcoded harness container roots. The current model treats source layout as an explicit package convention, keeps generated hidden harness directories out of default imports, and requires foreign hidden layouts to be rooted deliberately with `subpath` and `dialect`. The durable decision record is in the Meridian CLI KB: [convention-based source discovery](https://github.com/haowjy/meridian-cli-kb/blob/main/kb/decisions/package-management.md#d87-convention-based-source-discovery-and-explicit-hidden-foreign-import).
