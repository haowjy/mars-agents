# src/target/ — Per-Target Compilation Adapters

Target-specific lowering for `.claude`, `.codex`, `.opencode`, `.pi`, `.cursor`, and legacy `.agents`.

## Mental Model

```
TargetRegistry → TargetAdapter trait → per-target implementations
    ↓
default_dest_path()  → where items go
write_hook_entries(ConfigWrite, …) → hook config files; MCP goes through mcp_file::reconcile
skill_variant_key()  → which skill variant directory to use
```

The adapter boundary isolates all per-target branching here, keeping shared compiler code free of `if target == ...` chains.

## Target Adapters

| Adapter | Target | Skill Variant Key | Notes |
|---|---|---|---|
| `AgentsAdapter` | `.agents` | None | Legacy, deprecated |
| `ClaudeAdapter` | `.claude` | `claude` | Native Claude format |
| `CodexAdapter` | `.codex` | `codex` | MCP in `.codex/config.toml` (TOML edited in place) |
| `OpencodeAdapter` | `.opencode` | `opencode` | OpenCode format |
| `PiAdapter` | `.pi` | `pi` | MCP in `.pi/mcp.json` |
| `CursorAdapter` | `.cursor` | `cursor` | Unmaintained; agents not materialized (`default_dest_path` is `None` for agents — skills/MCP only) |

## TargetAdapter Trait

| Method | Purpose |
|---|---|
| `name()` | Target root name (e.g., `.claude`) |
| `skill_variant_key()` | Which `variants/<key>/` directory this target consumes |
| `default_dest_path(kind, name)` | Where an item goes; `None` if target rejects the kind |
| `mcp_file()` | The adapter's one MCP spec (below); `None` when the target takes no MCP servers |
| `write_hook_entries(write, project_root)` | Hook config file writes; MCP is reconciled by `mcp_file::reconcile` |
| `hook_fragment_mode()` | Declares `MergeJson { events }` or `File` fragment placement |
| `hook_file_dest_path()` | Managed relative path for an opaque file fragment |
| `hook_fragment_problem(content)` | Names a contract violation in a file-mode fragment; callers build the warning |
| `hook_config_file_names()` | Config files mutated by merge-mode hook entries |
| `remove_owned_hook_entries(token, records, target_dir)` | Structural removal: find lock-recorded entries by JSON equality and delete them |

## MCP Spec (`mcp_file.rs`)

Each adapter declares one `McpFile`: the project-relative path of the file its
harness reads, the format of the section (`JsonSection(key)` or `TomlTable(key)`),
and `render`, which turns an `McpServerEntry` into the native value plus
lossiness. Preflight, the reconcile and the lock record all derive from it
(`mcp_file.rs`; `config_doc.rs` reads and edits either format as JSON values). An
adapter that needs a different file or shape changes its spec, nothing else.

- **One call per target**: `mcp_file::reconcile(file, project_root, target, previous, desired,
  force, diag)` loads the file once, removes stale entries the lock still owns, writes the desired ones, saves each file at
  most once, and returns exactly the records the lock should hold. On failure it returns
  `previous`'s MCP records so the next sync retries. `mars unlink` calls it with `desired`
  empty. Servers are rendered once, by `render_all`, where they are lowered.
- **Ownership**: one predicate, `mcp_file::ownership(record, current)` → `Unowned | Owned |
  Edited`. `Owned` is replaced in place (keeping the file's key order) or removed; `Edited` is
  kept with a warning (`--force` replaces it on write); `Unowned` is kept on write with a
  warning, and `--force` adopts it.
- **Empty file**: mars records no ownership of the file itself, only of entries. A file is
  deleted when this reconcile changed it and it then holds nothing but an empty section (`{}`,
  an empty section, a 0-byte TOML); a file it did not change never is, since it may be the
  user's, empty on purpose.
- **Old locks**: a record without `emitted_json` (mars 0.23 and earlier) never reaches here;
  the lock drops it on load, so mars never removes an entry by name alone.
- The `mcpServers` family (Claude, Pi, Cursor, and the Claude/Codex plugin tree) has one codec,
  `mars_schema::mcp_servers`: `render(transport, Flavor)` and the parse the plugin import uses.
  Claude and Pi share `claude_render` (`${VAR}`, remote typed `http`); Cursor passes
  `Flavor::Cursor` (`${env:VAR}`, no `type`, since it is not known to accept one); the plugin tree
  (`build/plugin/tree/claude.rs`) passes `Flavor::Plugin`, which adds `env_vars` for same-name stdio
  references. Codex renders its own TOML shape (`render_remote`) and OpenCode its own, sharing
  `env_map`/`header_map`/`partition_env` from the codec. `McpRender::lossy` holds full sentences
  everywhere; sync and the plugin tree report them as `mcp-lossy`.

- **Files**: `config_doc` holds JSON as `OrderedValue` so a rewrite keeps the user's key order
  (the crate does not enable `serde_json/preserve_order`; that would reorder every JSON
  output). A symlinked destination is written through to its target, so the link survives, and
  an existing file keeps its permissions. Hook config files load, validate and save through the
  same `JsonDoc` (`hook_file.rs`), so they get the same guarantees.

Hooks do not use the spec: their event-array semantics differ. Hooks are unkeyed array
elements, so a write cannot replace one in place; they keep the retention plan
(`surface_ownership::retention`): remove every recorded entry, then append (`hook_file.rs`).

Two entry types flow through lowering:
- `McpServerEntry { name, transport }` — `transport` is the schema's `McpTransport` (stdio command/args/env, or http url/headers) with symbolic variable names; `${MARS_MCP_DIR}` is already replaced with an absolute path in stdio
- `HookEntry` — hook name, native event name, and an opaque `Vec<Value>` of entries (post-substitution); structural removal matches these exact entries by JSON equality

The permit and removal token are sealed proofs issued by `surface_ownership::retention` for hooks; adapter removals cannot bypass outcome recording, and hook writes cannot bypass suppression.

Adapters translate env variable names to target interpolation syntax (e.g., `${VAR}` for Claude) in `render`.

## Windows Filename Validation

`validate_agent_filename()` runs on every platform to ensure generated packages are portable. Rejects:
- Windows invalid chars: `:`, `*`, `?`, `<`, `>`, `|`, `"`, `/`, `\`
- Reserved device names: `CON`, `PRN`, `AUX`, `NUL`, `COM1-9`, `LPT1-9`

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
3. Add the stable id and authored name to `mars_schema::harness` if it is a harness target

## See Also

- `src/compiler/AGENTS.md` — uses adapters during compilation
- `src/compiler/agents/lower.rs` — per-harness lowering logic
- `src/target_sync/` — copies compiled content to target directories
