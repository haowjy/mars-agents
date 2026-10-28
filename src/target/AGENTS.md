# src/target/ — Per-Target Compilation Adapters

Target-specific lowering for `.claude`, `.codex`, `.opencode`, `.pi`, `.cursor`, and legacy `.agents`.

## Mental Model

```
TargetRegistry → TargetAdapter trait → per-target implementations
    ↓
default_dest_path()  → where items go
write_hook_entries(WritePermit, …) → hook config files; MCP goes through mcp_file.rs
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
| `write_hook_entries(permit, entries, target_dir)` | Hook config file writes; MCP is written by `mcp_file.rs` |
| `hook_fragment_mode()` | Declares `MergeJson { events }` or `File` fragment placement |
| `hook_file_dest_path()` | Managed relative path for an opaque file fragment |
| `hook_fragment_problem(content)` | Names a contract violation in a file-mode fragment; callers build the warning |
| `hook_config_file_names()` | Config files mutated by merge-mode hook entries |
| `legacy_hook_config_file_names()` | One-release legacy hook files for old lock records |
| `remove_owned_hook_entries(token, records, target_dir)` | Structural removal: find lock-recorded entries by JSON equality and delete them |

## Config Entries

## MCP Spec (`mcp_file.rs`)

Each adapter declares one `McpFile`: the project-relative path of the file its
harness reads, the format of the section (`JsonSection(key)` or `TomlTable(key)`),
and `render`, which turns an `McpServerEntry` into the native value plus
lossiness. Preflight, write, removal, and the lock record all derive from it
(`mcp_file.rs`; `mcp_doc.rs` reads and edits either format as JSON values). An
adapter that needs a different file or shape changes its spec, nothing else.

- **Ownership**: one predicate, `mcp_file::ownership(record, current)` → `Unowned | Owned |
  Edited`, used by write and remove. Write is the only place an entry is replaced: `Owned`
  replaces in place (keeping the file's key order); `Edited` and `Unowned` are kept with a
  warning (`--force` adopts them). Remove deletes `Owned`, keeps `Edited` with a warning, and
  removes an `Unowned` legacy record by name.
- **Legacy**: a record with no `emitted_json` predates the spec. It is removed by name from
  `legacy` (the adapter's earlier file, or the same file's earlier section), and that file is
  deleted only if it ends up empty. An adapter with no `legacy` drops such a record and
  touches no file, so deleting a `legacy` spec later can never reach a user entry by name.
  `RemovalPlan` pre-removes only these records (the one arm in `surface_ownership::retention`
  to delete with the legacy cleanup), so a changed record with content is never removed before
  its write. Delete the `legacy` specs one release after they ship.
- The Claude plugin tree has its own renderer (`build/plugin/tree/claude.rs`): it adds
  `env_vars` for same-name stdio references (sharing `mcp_file::partition_env` with Codex) and
  writes remote servers as `type: http` with `${VAR}` headers, which Codex passes literally (a
  lossy warning). `McpRender::lossy` holds full sentences everywhere.
- `mcp_file::servers_entry` is the `mcpServers`-family entry builder (Claude, Cursor, Pi) for
  both transports; `insert_headers` writes a remote server's `headers`. Claude and Pi share
  `braced_render` (`${VAR}`, remote typed `http`); Cursor passes `typed_http: false` because it
  is not known to accept `type`. Codex renders remote servers itself (`render_remote`).

- **Files**: `mcp_doc` holds JSON as `OrderedValue` so a rewrite keeps the user's key order
  (the crate does not enable `serde_json/preserve_order`; that would reorder every JSON
  output). A symlinked destination is written through to its target, so the link survives.

Hooks do not use the spec: their event-array semantics differ.

Two entry types flow through adapters:
- `McpServerEntry { name, transport }` — `transport` is the schema's `McpTransport` (stdio command/args/env, or http url/headers) with symbolic variable names; `${MARS_MCP_DIR}` is already replaced with an absolute path in stdio
- `HookEntry` — hook name, native event name, and an opaque `Vec<Value>` of entries (post-substitution); structural removal matches these exact entries by JSON equality

The permit and removal token are sealed proofs issued by `surface_ownership::retention`; adapter removals cannot bypass outcome recording, and config writes cannot bypass suppression.

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
