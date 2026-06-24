# src/discover/ — Source Item Discovery

Discovers package-provided agents, skills, and bootstrap docs before resolve/sync installs them.

## Contract

Discovery is convention-based. A single bounded recursive walk starts at the rooted package directory and scans any directory named, up to `MAX_DISCOVERY_WALK_DEPTH = 5`:

- `agents/` for `*.md` agent profiles
- `skills/` for child directories containing `SKILL.md`
- `bootstrap/` for child directories containing `BOOTSTRAP.md`

The same walk is used for packages with `mars.toml`, packages without `mars.toml`, and local `.mars-src/` roots.

After convention scanning, discovery is globally grounded to the shallowest logical layer that contains convention items. Agents, skills, and bootstrap docs are treated as one package layer: if `skills/foo` exists at the package layer, deeper `examples/skills/bar` or vendored nested containers are ignored. If the only convention items are nested, that nested layer becomes the grounded package layer and is still discovered.

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
