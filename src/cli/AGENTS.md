# src/cli/ — Policy Layer

CLI command definitions, project root discovery, and dispatch to library functions. 26 subcommand modules, ~9400 lines.

## Mental Model

```
 clap args → Command enum → dispatch_result() → find_agents_root() → dispatch_with_root()
                                                                    ↓
                                                         cli/<cmd>.run() → lib functions
```

The CLI layer is **policy** — what to do, which model, what output. Library modules are **mechanism** — how to do it.

## Root Discovery

`find_agents_root()` walks up from cwd (or `--root`) to filesystem root looking for `mars.toml`. Git boundaries do NOT stop the walk. `Path::parent()` returns `None` at filesystem root on all platforms.

Three bypass conditions:
1. **Root-free**: `init`, `check`, `cache` — no project needed
2. **Auto-init**: `add`, `link` — creates mars.toml if missing
3. **Ad-hoc launch**: `build launch-bundle` (no `--agent`, optional `--model <x>`) — works in plain directory

## Command Categories

| Category | Commands | Notes |
|---|---|---|
| Package mgmt | `add`, `remove`, `upgrade`, `outdated`, `why` | Mutate mars.toml + lock |
| Sync | `sync`, `repair` | Make reality match config |
| Build | `validate`, `export`, `build` | Dry-run or produce artifacts |
| Config | `link`, `unlink`, `override`, `resolve` | Target/dev settings; `link --force` adopts unmanaged collisions and persists lock |
| Models | `models` | Alias resolution, catalog cache |
| Diagnostics | `doctor`, `check`, `list`, `version` | Read-only inspection |
| Init | `init` | Bootstrap project |

## Model Command Boundaries

Resolve alias identity statically, then apply effective target scope before routing
or probe collection. Availability output is limited to permitted installed routes;
keep physical installation facts intact for routing diagnostics. Live model availability
must honor route rejection and describe only the selected route, not other installed
harnesses. Unverified routes report unknown availability with no asserted runnable
paths. Live inventory and resolution share one NativeAuthCache per command. Exact/prefix
resolution with no selected route exits nonzero, even when the model ID resolved.
Alias harness declarations are ordered preferences, not pins. Exact and live alias
commands share selected-route projection; prefix commands retain the longest base
alias's harness preference and provider constraint. Static listings do not route.

## Lossiness Gating

`SyncRequest.lossiness_mode` (`LossinessMode::Surface` | `Hidden`) is applied when the
pipeline creates its `DiagnosticCollector`. Lossiness-category diagnostics are suppressed
at emission time when mode is `Hidden`.

| Route | Mechanism | `lossiness_mode` | Calls lossiness preview directly |
|---|---|---|---|
| `mars sync` | Sync pipeline | `Surface` | No |
| `mars upgrade` | Sync pipeline | `Surface` | No |
| `mars validate` / `export` / `add` / `repair` | Sync pipeline | `Hidden` | No |
| `mars check` | Direct preview | `Surface` | Yes |
| `mars init` | Direct preview | `Surface` | Yes |

`mars check` and `mars init` call `lossiness_preview::collect_source_lossiness_diagnostics`
with `LossinessMode::Surface` (bypassing the sync pipeline).

## Catalog / probe refresh flags

`--refresh-models` and `--no-refresh-models` are mutually exclusive. They map to
`models::resolve_models_refresh_control` (catalog `ensure_fresh` mode + probe refresh mode).
See [`src/models/AGENTS.md`](../models/AGENTS.md) for the full matrix.

| Command | Flags on |
|---|---|
| `mars models list` | `ListArgs` |
| `mars models resolve <alias>` | `ResolveAliasArgs` |
| `mars build launch-bundle` | `LaunchBundleArgs` |
| `mars sync` | `SyncArgs` (`cli/sync.rs`) |

`mars models refresh` always fetches; `alias` / `refresh` subcommands have no refresh flags.

## Output

- Human-readable by default, `--json` for machine consumption
- `output.rs` has shared formatting helpers
- Exit codes from `MarsError::exit_code()`
- Lock corruption hint: suggests `mars repair`

## Patterns

**Adding a command:**
1. Define args struct in `cli/<cmd>.rs`
2. Add variant to `Command` enum in `mod.rs`
3. Add dispatch arm in `dispatch_with_root()` (or root-free in `dispatch_result()`)
4. Implement `run()` → call library function → format output

**Project context:**
```rust
let ctx = find_agents_root(cli.root.as_deref())?;
// ctx.project_root, ctx.meridian_managed
```

## See Also

- `.context/` — none (too thin for deep context)
- `src/config/AGENTS.md` — config schema the CLI manipulates
- `src/sync/AGENTS.md` — what `mars sync` actually does
