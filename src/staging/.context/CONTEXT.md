# src/staging/ — Canonical Source Staging

Lift foreign-dialect package trees into a derived canonical source tree before
discovery, validation, hash, and apply.

## Placement

Staging runs in `resolve/package.rs` immediately after `apply_subpath` (and on
version-override replay / restart). `RootedSourceRef.package_root` is repointed at
the staged tree. Local project items stage in `sync/mod.rs` `build_target`.

```
fetch → ResolvedRef.tree_path (global cache, read-only)
  → apply_subpath → package_root
      → stage_canonical_source → staged_root
  → package_root := staged_root
  → discover / hash / validate / apply
```

## Staging location

Per-consumer under `<project>/.mars/staging/<source-name>/<dialect>/`.

- Never mutates the global fetch cache (`ResolvedRef.tree_path`).
- Dependency- and dialect-scoped paths make restaging deterministic for `--frozen`.
- Same source bytes + same resolved dialect + same overrides ⇒ byte-identical staged tree.

## `.mars/` store semantics

The canonical store (`.mars/`) is installed from the **staged** tree, not raw
fetched bytes. For inferred/default dialects with identity lift, content matches
the source. With explicit non-`mars-native` dialect, foreign frontmatter is lifted to
canonical mars fields per `staging/lift.rs`. Default/inferred `Claude` lift is
idempotent on already-canonical packages.

## Non-canonical allowed-tools handling (MarsNative skills only)

In `process_markdown_file` (mod.rs:271–312), MarsNative skills parse **RAW** frontmatter (before any lift) and call `push_non_canonical_tool_field_diags` → `"skill-schema-warning"` diagnostic. This runs BEFORE `lift_frontmatter_with_change` (lift.rs:41–55) strips the non-canonical aliases via `strip_non_canonical_tool_aliases`. The diagnostic inspects the original pre-mutation frontmatter; the strip operates on a separate clone. Both `.mars/` and `.claude/` end up clean (aliases removed from canonical store).

Non-canonical aliases detected: `allowed-tools`, `allowed_tools` (→ `tools:`), plus `disallowed_tools` (→ `disallowed-tools:`).

## `lift_frontmatter` (B3)

```rust
pub fn lift_frontmatter(
    dialect: Dialect,
    item_kind: ItemKind,
    frontmatter: &Frontmatter,
) -> Frontmatter
```

For `Dialect::MarsNative`, lift is limited to stripping non-canonical tool aliases. All other dialects apply their own key mapping (Claude, Codex, Cursor, OpenCode).

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
Use `tools: [mcp(server)]` instead. Projection to harness-native tokens:
[`src/compiler/mcp_ref.rs`](../compiler/mcp_ref.rs) — documented in
[agent-compilation.md](../../docs/config/agent-compilation.md#mcp-tool-policy-references).

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
