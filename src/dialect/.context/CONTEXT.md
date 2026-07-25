# src/dialect/ — Inbound Dialect Resolution

Single-file module (224 lines, `mod.rs`). Resolves which inbound dialect applies to a package or local item before frontmatter lifting in staging.

## Dialect Enum

```rust
pub enum Dialect {
    Claude,
    Codex,
    OpenCode,
    Cursor,
    #[serde(rename = "mars-native")]
    MarsNative,
}
```

Five variants mirroring `harness::registry::HarnessId` (Claude, Codex, OpenCode, Cursor, Pi) **minus Pi**, plus `MarsNative` for already-canonical mars-authored sources. Dialect depends downward on `HarnessId` only.

## Resolution

### `Dialect::resolve(explicit: Option<Self>, package_root: &Path) -> Self`

For **dependencies** (foreign packages). Three-step chain:

1. **Explicit `dialect` key** — if provided (`Some(dialect)`), return immediately.
2. **Foreign container path inference** — scan `package_root/<container>/{skills,agents}/` for exactly one non-empty subdirectory among `.claude`, `.codex`, `.opencode`, `.cursor`. Ambiguous (0 or >1) yields `None`.
3. **Default** — `Dialect::Claude`.

### `Dialect::resolve_local(explicit: Option<Self>, package_root: &Path) -> Self`

For **local project items**. Same three-step chain as `resolve`, but default is `Dialect::MarsNative` instead of `Claude`.

### Helper

`resolve_with_default(explicit, package_root, default)` implements the shared logic. Both public functions delegate to it with their respective default.

## Conversion to/from HarnessId

- `from_harness_id(HarnessId) -> Option<Dialect>` — returns `None` for `HarnessId::Pi` (no corresponding inbound dialect).
- `to_harness_id(self) -> Option<HarnessId>` — returns `None` for `Dialect::MarsNative` (no foreign harness equivalent).

Callers convert directly through `Dialect::from_harness_id` and
`Dialect::to_harness_id`.

## Tests

77 lines of tests covering: explicit beats inference, inference from containers, ambiguous containers return default, dialect→harness_id→dialect roundtrip, MarsNative→to_harness_id→None.
