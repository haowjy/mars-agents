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

Five variants mirroring `compiler::agents::HarnessKind` (Claude, Codex, OpenCode, Cursor, Pi) **minus Pi**, plus `MarsNative` for already-canonical mars-authored sources. The upward dep on `HarnessKind` is tracked in issue #118 (structural inversion — dialect is logically more foundational than compiler harness enumeration).

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

## Conversion to/from HarnessKind

- `from_harness_kind(HarnessKind) -> Option<Dialect>` — returns `None` for `HarnessKind::Pi` (no corresponding inbound dialect).
- `to_harness_kind(self) -> Option<HarnessKind>` — returns `None` for `Dialect::MarsNative` (no foreign harness equivalent).

## Tests

77 lines of tests covering: explicit beats inference, inference from containers, ambiguous containers return default, dialect→harness→dialect roundtrip, MarsNative→to_harness_kind→None.
