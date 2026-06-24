# src/discover/ — Source Item Discovery

Discovers package-provided agents, skills, and bootstrap docs before resolve/sync installs them.

## Contract

Discovery is convention-based. A single bounded recursive walk starts at the rooted package directory and scans any directory named:

- `agents/` for `*.md` agent profiles
- `skills/` for child directories containing `SKILL.md`
- `bootstrap/` for child directories containing `BOOTSTRAP.md`

The same walk is used for packages with `mars.toml`, packages without `mars.toml`, and local `.mars-src/` roots.

## Hidden directories

The walk skips dot-prefixed directories at every descent step. This keeps generated harness outputs and control/cache directories (`.claude/`, `.codex/`, `.cursor/`, `.opencode/`, `.git/`, `.mars/`) out of default package discovery without maintaining a harness blocklist.

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
