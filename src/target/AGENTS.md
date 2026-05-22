# src/target/ — Per-Target Compilation Adapters

Target-specific lowering for `.claude`, `.codex`, `.opencode`, `.pi`, `.cursor`, and legacy `.agents`. 7 files, ~2300 lines.

## Mental Model

```
TargetRegistry → TargetAdapter trait → per-target implementations
    ↓
default_dest_path()  → where items go
write_config_entries() → MCP/hook config files
skill_variant_key()  → which skill variant directory to use
```

The adapter boundary isolates all per-target branching here, keeping shared compiler code free of `if target == ...` chains.

## Target Adapters

| Adapter | Target | Skill Variant Key | Notes |
|---|---|---|---|
| `AgentsAdapter` | `.agents` | None | Legacy, deprecated |
| `ClaudeAdapter` | `.claude` | `claude` | Native Claude format |
| `CodexAdapter` | `.codex` | `codex` | TOML-based config |
| `OpencodeAdapter` | `.opencode` | `opencode` | OpenCode format |
| `PiAdapter` | `.pi` | `pi` | Pi format |
| `CursorAdapter` | `.cursor` | `cursor` | Experimental; agents not materialized (`default_dest_path` is `None` for agents — skills/MCP only) |

## TargetAdapter Trait

| Method | Purpose |
|---|---|
| `name()` | Target root name (e.g., `.claude`) |
| `skill_variant_key()` | Which `variants/<key>/` directory this target consumes |
| `default_dest_path(kind, name)` | Where an item goes; `None` if target rejects the kind |
| `write_config_entries(entries, target_dir)` | MCP/hook config file writes |
| `emit_pre_write_diagnostics(entries, diag)` | Lossiness warnings before writes |
| `remove_config_entries(keys, target_dir)` | Stale config cleanup |

## Config Entries

Two entry types flow through adapters:
- `McpServerEntry` — name, command, args, env (symbolic variable names)
- `HookEntry` — name, event, native_event, script_path, order

Adapters translate env variable names to target interpolation syntax (e.g., `${VAR}` for Claude).

## Windows Filename Validation

`validate_agent_filename()` runs on every platform to ensure generated packages are portable. Rejects:
- Windows invalid chars: `:`, `*`, `?`, `<`, `>`, `|`, `"`, `/`, `\`
- Reserved device names: `CON`, `PRN`, `AUX`, `NUL`, `COM1-9`, `LPT1-9`

## Hook Command Generation

`hook_command()` produces platform-appropriate command strings:
- POSIX: `bash '/path/to/script.sh'` (single quotes with escaping)
- Windows: `bash "C:/path/to/script.sh"` (double quotes, normalized slashes)

## Patterns

**Registry lookup:**
```rust
let registry = TargetRegistry::new();
let adapter = registry.get(".claude").unwrap();
let dest = adapter.default_dest_path(ItemKind::Agent, "coder").unwrap();
```

**Adding a new target:**
1. Create `src/target/<name>.rs` implementing `TargetAdapter`
2. Register in `TargetRegistry::new()`
3. Add to `HarnessKind` in `compiler/agents/mod.rs` if it's a harness target

## See Also

- `src/compiler/AGENTS.md` — uses adapters during compilation
- `src/compiler/agents/lower.rs` — per-harness lowering logic
- `src/target_sync/` — copies compiled content to target directories
