# src/discover/ — Source Item Discovery

`mars-schema` is the authority for Agent and skill selection, including Claude plugin and
marketplace manifests and skill trees (`inspect`, `normalize`). Resolver and plugin paths call the
binary-only inventory here for hooks and bootstrap documents, then combine it with
`Prepared.definitions`; they do not run the compatibility all-kind walk.

## Contract

`discover` selects the kinds `mars-schema` does not own: hooks and bootstrap documents
(`discover_binary_owned_items`). `package_items` adds the package's Agents and skills from the
`Prepared` result each caller already holds (`definition_item` maps one definition; a skill's item
path is the parent of its output path). Nothing here walks a tree for Agents or skills, and nothing
may: crate preparation owns those definitions.

Bootstrap discovery is convention-based. It uses `mars_schema::discover::walk_dirs`, the one bounded walk the crate selects Agents and skills with, and scans each directory named `bootstrap/` it reaches for child directories containing `BOOTSTRAP.md`. The walk owns the boundary (depth five, hidden directories, the skip list, `[plugin].dir`, generated plugin trees); nothing here keeps a copy of it.

Hooks are the exception to the layered walk: only direct children of the
package-root `hooks/` directory are discovered. The compiler and general item
discovery share that directory enumerator, so a hook cannot be listed and then
fail resolution because a second scanner searched a different root.
During target construction, strict hook parsing supplies the corresponding
`DiscoveredItem`s, preserving collision checks without parsing `hook.toml` twice.

Bootstrap documents are grounded to the shallowest logical layer that contains any: deeper
vendored `bootstrap/` containers are ignored. Grounding sees only bootstrap documents, not Agents or
skills.

## Current-package self inputs

`local_source` takes the project's Agents and skills from preparation when the project declares
`[package]`. `.mars-src` is discovered separately and wins matching `(kind, name)` definitions
before staging.

Directories containing a valid `.mars-plugin.json` are generated plugin trees and are
excluded wherever the bounded walk meets them (the walk checks the marker of each directory it is
about to enter, and never scans the tree for markers), including hook and bootstrap discovery, with one
info diagnostic per skipped path. A valid package-relative `[plugin].dir`, which the crate reads
from `mars.toml`, is a stronger source boundary: the walk excludes it by path without reporting
it, even before a sentinel exists, so the package's own committed output never adds noise to
`sync` or `check`. Canonical staging applies the sentinel boundary before any
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

With that rooting, the foreign `agents/` and `skills/` are non-hidden children of the effective package root and `normalize` reads them.

## Root `SKILL.md`

A package-root `SKILL.md` is a flat-root skill that preparation reports with an output path of `SKILL.md`; `definition_item` gives it the item path ".". Bootstrap docs can still accompany it through explicit plugin-manifest declarations. A root skill of a manifest-free dependency is relocated to `skills/<slug>/` by `normalize`.

## Manifest declarations

Only `bootstrapDocs` is read from `.claude-plugin/plugin.json` and `marketplace.json` here; the paths are normalized, checked to stay under the package root, and registered as bootstrap docs. Skills and Agents a manifest lists are the schema crate's (`claude::manifest`), where the manifest's lists add to or replace the default folders as Claude does.

Hooks and bootstrap documents share one duplicate rule: if two discovered items have the same `(kind, name)`, discovery raises `DiscoveryCollision`. Two Agents or two skills with one slug are the crate's `import.duplicate-slug` error, reported by preparation. There is no precedence order or silent shadowing inside a single source.

## Rationale

The bounded convention walk replaced the earlier two-path discovery model: fixed top-level conventions plus a heuristic BFS with hardcoded harness container roots. The current model treats source layout as an explicit package convention, keeps generated hidden harness directories out of default imports, and requires foreign hidden layouts to be rooted deliberately with `subpath` and `dialect`. The durable decision record is in the Meridian CLI KB: [convention-based source discovery](https://github.com/haowjy/meridian-cli-kb/blob/main/kb/decisions/package-management.md#d87-convention-based-source-discovery-and-explicit-hidden-foreign-import).
