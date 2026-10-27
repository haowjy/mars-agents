# Extending Mars

How to add new capabilities to mars. Each section lists the files you need to touch and the contracts you need to satisfy.

## Adding a New Item Kind

`ItemKind` currently has `Agent` (single `.md` file), `Skill` (directory with `SKILL.md`), `Hook`, `McpServer` and `BootstrapDoc`. Only agents and skills are *definitions* (`DefinitionKind`) that `mars-schema` selects and reads; the other kinds are binary-owned. Adding another kind (e.g., `Tool`, `Prompt`) touches these files:

### 1. Type definition

**`crates/mars-schema/src/item.rs`** — Add variant to `ItemKind` enum (`src/lock/mod.rs` consumes it):
```rust
pub enum ItemKind {
    Agent,
    Skill,
    Hook,
    McpServer,
    BootstrapDoc,
    Tool,  // new
}
```
Update the `Display` impl. The `#[serde(rename_all = "lowercase")]` attribute handles serialization automatically.

### 2. Discovery

Pick the owner by what the kind is:

- **A binary-owned kind** (like hooks, bootstrap docs, MCP servers): add a scan to `src/discover/mod.rs`. `discover_binary_owned_items()` is the place: it already calls `discover_bootstrap_surfaces()` and takes hooks pre-parsed by the compiler. Add your scan there (reuse `mars_schema::discover::walk_dirs` for the bounded walk; do not keep a second skip list), then `package_items()` merges it with agents and skills. MCP servers are discovered separately (`compiler::mcp::discover_mcp_items`, called from `src/sync/target.rs`).
- **An authored definition** (selected, read and validated like agents and skills): extend `mars-schema` instead: `DefinitionKind` in `item.rs`, the convention scan in `crates/mars-schema/src/discover.rs` (`discover()`), and `prepare`. `discover::definition_item()` / `package_items()` then map `Prepared.definitions` to items.

Either way, items must produce `DiscoveredItem { id: ItemId { kind: ItemKind::Tool, name }, source_path }`; `finalize_items()` dedupes by path and rejects duplicate names. Local (`_self`) items go through `discover_local_items()` in `src/local_source.rs`; update it if the kind can live in the project root.

### 3. Hashing

**`src/hash/mod.rs`** — Add hash computation for the new kind in `compute_hash()`:
```rust
pub fn compute_hash(path: &Path, kind: ItemKind) -> Result<String, MarsError> {
    match kind {
        ItemKind::Agent => { /* file hash */ }
        ItemKind::Skill | ItemKind::Hook | ItemKind::McpServer => compute_dir_hash(path),
        ItemKind::BootstrapDoc => { /* hashes the BOOTSTRAP.md's directory */ }
        ItemKind::Tool => { /* your hash logic */ }
    }
}
```

Agents use file content SHA-256. Skills use deterministic directory hash. Choose based on whether your kind is a single file or a directory.

### 4. Target building

**`src/sync/target.rs`** — Likely no changes needed if discovery produces standard `DiscoveredItem` structs. The target builder is kind-agnostic after discovery.

If the new kind needs special destination paths (not `agents/` or `skills/`), update:
- `default_dest_path()` — maps `RenamableKind` → destination path (only kinds a rename can name; hooks and MCP servers have paths owned by `src/sync/package_dir.rs`)
- `parse_rename_dest()` — handles rename value parsing

### 5. Apply

**`src/sync/apply.rs`** — Update `install_item()` to handle the new kind:
```rust
fn install_item(target: &TargetItem, dest: &Path) -> Result<ContentHash, MarsError> {
    match target.id.kind {
        ItemKind::Agent => { /* write_file_and_verify */ }
        ItemKind::BootstrapDoc => { /* atomic_install_dir on the doc directory */ }
        ItemKind::Skill | ItemKind::Hook | ItemKind::McpServer => { /* atomic_install_dir */ }
        ItemKind::Tool => { /* your install logic */ }
    }
}
```

Also check the removal path in `src/fs/mod.rs`/`src/platform/fs.rs` and the merge-base caching in `apply.rs` if the kind participates in three-way merge.

### 6. Filter config

**`src/config/mod.rs`** — If the new kind should be filterable, add a field to `FilterConfig`:
```rust
pub struct FilterConfig {
    pub agents: Option<Vec<ItemName>>,
    pub skills: Option<Vec<ItemName>>,
    pub tools: Option<Vec<ItemName>>,  // new
    // ...
}
```

Update `FilterMode` derivation in `merge_with_root()` and `apply_filter()` in `src/sync/filter.rs`.

### 7. CLI output

**`src/cli/output.rs`**, **`src/cli/list.rs`** — Update display and listing logic to show the new kind.

### 8. Validation

**`src/sync/validate.rs`**, **`src/cli/validate.rs`** — If the new kind has cross-reference semantics (like agents referencing skills), add validation logic.

### Summary: files touched

| File | Change |
|---|---|
| `crates/mars-schema/src/item.rs` | `ItemKind` variant + Display |
| `src/discover/mod.rs` (binary-owned kinds) or `crates/mars-schema/src/discover.rs` (authored definitions) | Discovery convention |
| `src/hash/mod.rs` | `compute_hash()` match arm |
| `src/sync/target.rs` | `default_dest_path()`, `parse_rename_dest()` |
| `src/sync/apply.rs` | `install_item()`, merge-base caching |
| `src/config/mod.rs`, `src/sync/filter.rs` | `FilterConfig` field, `apply_filter()` (optional) |
| `src/cli/output.rs` | Display formatting |
| `src/cli/list.rs` | List output |
| `src/sync/validate.rs` | Cross-reference validation (if applicable) |

## Adding a New Source Type

Currently mars supports git repos and local paths. Adding a third source type (e.g., HTTP archive, registry) touches these files:

### 1. Source spec

**`src/config/mod.rs`** — Add to `SourceSpec` (in the effective config) and `DependencyEntry`:
```rust
pub struct DependencyEntry {
    pub url: Option<SourceUrl>,
    pub path: Option<PathBuf>,
    pub registry: Option<String>,  // new
    // ...
}
```

Update the `url XOR path` validation to include the new field.

### 2. SourceId

**`src/types.rs`** — Add variant to `SourceId`:
```rust
pub enum SourceId {
    Git { url: SourceUrl },
    Path { canonical: PathBuf },
    Registry { name: String, package: String },  // new
}
```

### 3. Source adapter

**`src/source/`** — Create a new module (e.g., `src/source/registry.rs`):
- Implement fetching logic that produces a `ResolvedRef` (with `tree_path` pointing to extracted content)
- Register in `src/source/mod.rs`

### 4. Resolver integration

**`src/resolve/mod.rs`** — Update `resolve_single_source()` to handle the new spec:
```rust
fn resolve_single_source(...) -> Result<ResolvedRef, MarsError> {
    match &pending.spec {
        SourceSpec::Path(path) => provider.fetch_path(...),
        SourceSpec::Git(git) => resolve_git_source(...),
        SourceSpec::Registry(reg) => resolve_registry_source(...),  // new
    }
}
```

Add a trait method to `SourceFetcher` or create a new trait if the fetching interface differs significantly.

### 5. Lock file

**`src/lock/mod.rs`** — Add provenance fields to `LockedSource`:
```rust
pub struct LockedSource {
    pub url: Option<SourceUrl>,
    pub path: Option<String>,
    pub registry: Option<String>,  // new
    pub registry_version: Option<String>,  // new
    // ...
}
```

Update `to_locked_source()` in `src/lock/mod.rs` to populate the new fields.

### 6. Sync pipeline

**`src/sync/mod.rs`** — Update `RealSourceProvider` to handle the new source type in its trait implementations.

### Summary: files touched

| File | Change |
|---|---|
| `src/config/mod.rs` | `DependencyEntry` field, `SourceSpec` variant, validation |
| `src/types.rs` | `SourceId` variant |
| `src/source/new_type.rs` | New module: fetch, list versions |
| `src/source/mod.rs` | Re-export, register |
| `src/resolve/mod.rs` | `resolve_single_source()` match arm, trait methods |
| `src/lock/mod.rs` | `LockedSource` fields, `to_locked_source()` |
| `src/sync/mod.rs` | `RealSourceProvider` trait impls |
| `src/cli/add.rs` | Parse new source spec from CLI args |

## Adding a New CLI Command

### 1. Command module

**`src/cli/new_cmd.rs`** — Create the command handler:
```rust
use clap::Args;
use crate::error::MarsError;
use crate::types::MarsContext;

#[derive(Debug, Args)]
pub struct NewCmdArgs {
    // clap fields
}

pub fn run(ctx: &MarsContext, args: &NewCmdArgs) -> Result<(), MarsError> {
    // implementation
    Ok(())
}
```

### 2. Register in CLI

**`src/cli/mod.rs`** — Add the subcommand:
```rust
pub mod new_cmd;

#[derive(Debug, Subcommand)]
pub enum Commands {
    // existing...
    /// Description of new command
    NewCmd(new_cmd::NewCmdArgs),
}
```

### 3. Wire up in main

**`src/main.rs`** — Add the match arm:
```rust
Commands::NewCmd(args) => cli::new_cmd::run(&ctx, args),
```

### Patterns to follow

- **Commands that modify state** should use the sync pipeline: construct a `SyncRequest` with the appropriate `ConfigMutation` and call `sync::execute()`. See `src/cli/add.rs`, `src/cli/remove.rs`, `src/cli/rename.rs`.
- **Read-only commands** can load config/lock directly: `config::load()`, `lock::load()`. See `src/cli/list.rs`, `src/cli/why.rs`, `src/cli/outdated.rs`.
- **Output formatting** should use helpers from `src/cli/output.rs` for consistent display.

### Existing CLI commands for reference

| Command | File | Pattern |
|---|---|---|
| `add` | `src/cli/add.rs` | Mutating: `UpsertDependency` → `sync::execute()` |
| `remove` | `src/cli/remove.rs` | Mutating: `RemoveDependency` → `sync::execute()` |
| `sync` | `src/cli/sync.rs` | Mutating: no mutation, just sync |
| `upgrade` | `src/cli/upgrade.rs` | Mutating: `ResolutionMode::Maximize` |
| `override` | `src/cli/override_cmd.rs` | Mutating: `SetOverride`/`ClearOverride` |
| `rename` | `src/cli/rename.rs` | Mutating: `SetRename` |
| `list` | `src/cli/list.rs` | Read-only: load lock + discover installed |
| `why` | `src/cli/why.rs` | Read-only: trace dependency provenance |
| `outdated` | `src/cli/outdated.rs` | Read-only: compare locked vs available |
| `doctor` | `src/cli/doctor.rs` | Read-only: health checks |
| `check` | `src/cli/check.rs` | Read-only: validate lock integrity |
| `repair` | `src/cli/repair.rs` | Mutating: reset lock, re-sync |
| `resolve` | `src/cli/resolve_cmd.rs` | Mutating: mark conflicts resolved |
| `init` | `src/cli/init.rs` | Creates `mars.toml` |
| `link` | `src/cli/link.rs` | Settings mutation (lightweight, no full sync) |
| `cache` | `src/cli/cache.rs` | Cache management |
