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

## `lift_frontmatter` (B3)

```rust
pub fn lift_frontmatter(
    dialect: Dialect,
    item_kind: ItemKind,
    frontmatter: &Frontmatter,
) -> Frontmatter
```

C-skills applies `[skills.<name>]` overrides after lift in the same staging hook
(`staging/overlay.rs`). Tool overlays project into canonical `tools:` /
`disallowed-tools:` / `mcp-tools:` (not legacy `allowed-tools`). Lookup key is the **installed** skill name (after
explicit rename), matching `[skills.<name>]` in mars.toml — not the source
directory basename alone. Flat/root `SKILL.md` skills use the discovered item
name (dependency source name or configured fallback).

## Threading

- `EffectiveDependency.dialect` — explicit `[dependencies.<dep>].dialect`
- `EffectiveDependency.rename` — overlay lookup uses installed names after rename
- `EffectiveConfig.skills` — `[skills.<name>]` overlays applied in `process_markdown_file`
- `ResolveOptions.staging_root` — set by sync to `.mars/staging`

Dialect resolution (`crate::dialect::Dialect::resolve`):

1. explicit `dialect` key
2. foreign container path (`.claude`, `.codex`, `.opencode`, `.cursor`)
3. default `Claude` (including bare `skills/` / `agents/`)
