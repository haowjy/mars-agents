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
3. **Ad-hoc launch**: `build launch-bundle --model <x>` (no `--agent`) — works in plain directory

## Command Categories

| Category | Commands | Notes |
|---|---|---|
| Package mgmt | `add`, `remove`, `upgrade`, `outdated`, `why` | Mutate mars.toml + lock |
| Sync | `sync`, `repair` | Make reality match config |
| Build | `validate`, `export`, `build` | Dry-run or produce artifacts |
| Config | `link`, `unlink`, `override`, `resolve` | Target/dev settings |
| Models | `models` | Alias resolution, catalog cache |
| Diagnostics | `doctor`, `check`, `list`, `version` | Read-only inspection |
| Init | `init` | Bootstrap project |

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
// ctx.project_root, ctx.managed_root, ctx.meridian_managed
```

## See Also

- `.context/` — none (too thin for deep context)
- `src/config/AGENTS.md` — config schema the CLI manipulates
- `src/sync/AGENTS.md` — what `mars sync` actually does
