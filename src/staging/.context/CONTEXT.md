# src/staging/ — Canonical Source Staging

Materialize classified package trees as schema-v1 content before discovery,
validation, hash, and apply.

## Placement

`package_prepare::classify` runs immediately after `apply_subpath`, before staging and
discovery. Staging copies schema-v1 packages without schema rewrites, runs
`mars_schema::upgrade` for legacy packages, or lifts manifest-free foreign content.
`RootedSourceRef.package_root` is repointed at the staged tree.

Local discovery follows the same ordering. Declared content is classified first and legacy
content is staged before discovery. Undeclared `.mars-src` always passes through the legacy
upgrade and warns only when a definition changed. Items from either prepared tree are schema-v1
when `sync/mod.rs` later stages each selected item.

Flat package skills filter control files, `.mars-src`, standard native roots, and configured or previously owned target paths
(including existing resolved paths for aliases/absolute/dot-segment spellings)
before traversal, because their source root contains `.mars/staging`. Filtering
at install/hash time alone would recurse into the staging destination. This
resource filter is combined with the shared authored-tree walk, which excludes
valid generated plugin trees before any bytes reach staging.
For a flat `.mars-src` skill, project output paths are rebased to the actual
source root; similarly named authored resources are preserved.

```
fetch → ResolvedRef.tree_path (global cache, read-only)
  → apply_subpath → package_root
      → stage_canonical_source → staged_root
  → package_root := staged_root
  → discover / hash / validate / apply
```

Discovery runs from the staged/rooted package directory. Its convention walk skips dot-prefixed child directories during default discovery; explicit dependency `subpath` can still make a hidden foreign container (for example `.claude`) the effective root, where its inner `agents/` and `skills/` directories are discovered normally. Foreign-container dialect inference chooses lift behavior only; it does not make hidden containers discovery roots.

## Staging location

Per-consumer under `<project>/.mars/staging/<source-name>/<preparation>/`, where preparation
is `schema-1`, `legacy`, or a foreign dialect. Local prepared roots live under
`.mars/staging/_authored/`.

- Never mutates the global fetch cache (`ResolvedRef.tree_path`).
- Dependency- and preparation-scoped paths make restaging deterministic for `--frozen`.
- Same source bytes + same preparation + same overrides ⇒ byte-identical staged tree.

## `.mars/` store semantics

The canonical store (`.mars/`) is installed from the **prepared** tree, never raw legacy
bytes. Schema-v1 definitions stay byte-identical unless a consumer overlay changes them.
Legacy definitions use the same pure conversion as `mars migrate`. Foreign frontmatter is
lifted through `staging/lift.rs`.

## Non-canonical skill tool handling

Legacy skills parse raw frontmatter before upgrade and emit the existing schema diagnostics.
The pure upgrade then drops skill tool keys because schema v1 gives them no meaning. Schema-v1
content is not rewritten here; strict rejection belongs to its parser.

Non-canonical aliases detected: `allowed-tools`, `allowed_tools` (→ `tools:`), plus `disallowed_tools` (→ `disallowed-tools:`).

## `lift_frontmatter` (B3)

```rust
pub fn lift_frontmatter(
    dialect: Dialect,
    item_kind: ItemKind,
    frontmatter: &Frontmatter,
) -> Frontmatter
```

`lift_frontmatter` now serves only manifest-free foreign inputs. Schema-v1 and legacy Mars
packages bypass it; legacy conversion belongs to `mars_schema::upgrade`.

Foreign scalar/list tool fields become canonical maps so imports never author deprecated
list-form `tools:`. Claude agent `tools:` is restrictive, so it becomes `'*': deny` plus an
`allow` entry per tool. Skill tool fields and `allowed-tools` / `allowed_tools` are grants, so
they become `allow` entries without the wildcard. Existing map actions stay intact. Codex
agent tool fields are dropped because the Codex native agent format has no such policy field;
Codex skill tool grants follow the skill rule.

C-skills applies `[skills.<name>]` overrides after lift in the same staging hook
(`staging/overlay.rs`). Tool overlays project into canonical `tools:` /
`disallowed-tools:` (including inline `mcp(...)` grants). Lookup key is the **installed**
skill name (after explicit rename), matching `[skills.<name>]` in mars.toml — not the
source directory basename alone. Flat/root `SKILL.md` skills use the discovered item
name (dependency source name or configured fallback).

## Inbound MCP lift

Foreign MCP permission tokens in `tools:` / `disallowed-tools:` (and skill
`allowed-tools` / `disallowedTools`) lift to canonical `mcp(...)` during dialect staging
(`staging/lift.rs`):

| Foreign form | Dialect | Lifts to |
|---|---|---|
| `mcp__server__tool`, `mcp__server__*`, `mcp__server`, `mcp__*` | Claude | `mcp(server/tool)`, `mcp(server/*)`, `mcp(*/*)` (segments verbatim) |
| `Mcp(server:tool)` | Cursor | `mcp(server/tool)` (last `:` separates tool; namespaced server ids preserved) |
| `mcpServers` | Claude | Appends `mcp(server)` entries to `tools:` (whole-server grant) |

The removed `mcp-tools:` / `mcp_tools` field is rejected at parse time (`RemovedField`).
Use a `tools:` map with `mcp(server): allow` instead. Projection to harness-native tokens:
[`src/compiler/mcp_ref.rs`](../../compiler/mcp_ref.rs) — documented in
[agent-compilation.md](../../../docs/config/agent-compilation.md#mcp-tool-policy-references).

## Poor Man's module — `skill_source_name`

Flat-root skill naming (`skill_source_name` in `src/skill_source_name.rs`) is the single canonical rule shared by discovery and staging overlay lookup. `staging/overlay.rs::skill_source_name` delegates to `flat_root_skill_source_name` for `SKILL.md` at package root, using the explicit `fallback_skill_name` when provided (dependency source name) or falling back to the package directory basename.

## Threading

- `EffectiveDependency.dialect` — explicit `[dependencies.<dep>].dialect`
- `EffectiveDependency.rename` — overlay lookup uses installed names after rename
- `EffectiveConfig.skills` — `[skills.<name>]` overlays applied in `process_markdown_file`
- `ResolveOptions.staging_root` — set by sync to `.mars/staging`
- `fallback_skill_name` — threaded into `process_markdown_file` via `StageOverlayContext`; used for flat-root skill overlay lookup and diagnostic messages

Dialect resolution (`crate::dialect::Dialect::resolve`):

1. explicit `dialect` key
2. foreign container path (`.claude`, `.codex`, `.opencode`, `.cursor`)
3. default `Claude` (including bare `skills/` / `agents/`)
