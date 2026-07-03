# Sync Pipeline

Every mutating command (`add`, `remove`, `sync`, `upgrade`, `override`, `rename`) runs the same pipeline. The pipeline resolves the full desired state before touching any files.

## Pipeline Overview

The pipeline is implemented as typed phase functions in `src/sync/mod.rs`. Each phase consumes the prior phase's output struct by value (move semantics, no cloning).

```
mars.toml + mars.local.toml
        │
        ▼
┌─────────────────┐
│  1. Load Config │  Acquire sync lock, load config, apply mutations, merge effective config
└────────┬────────┘
         ▼
┌─────────────────┐
│  2. Resolve     │  Fetch sources, discover transitive deps, merge model aliases,
│                 │  STAGE: each dep through stage_rooted_package — lift foreign
│                 │  frontmatter to canonical, apply skill overlays, repoint package_root
│                 │  to .mars/staging/<source>/<dialect>/
└────────┬────────┘
         ▼
┌─────────────────┐
│  3. Build Target│  Discover items, apply filters, detect collisions.
│                 │  STAGE: local (_self) items through staging::stage_local_item
│                 │  before hashing — same lift/overlay path as deps.
└────────┬────────┘
         ▼
┌─────────────────┐
│  4. Create Plan │  Diff target vs lock + disk → Add/Update/Conflict/Orphan → actions
└────────┬────────┘
         ▼
┌─────────────────┐
│  5. Apply Plan  │  Write resolved content to .mars/ canonical store (atomic writes)
└────────┬────────┘
         ▼
┌─────────────────┐
│  6. Sync Targets│  Copy from .mars/ to each configured target directory
└────────┬────────┘
         ▼
┌─────────────────┐
│  7. Finalize    │  Write lock, persist dep model aliases, build report
└─────────────────┘
```

### Phase handoff structs

| Phase | Struct | Key contents |
|---|---|---|
| 1 | `LoadedConfig` | `Config`, `LocalConfig`, `EffectiveConfig`, `old_lock`, `_sync_lock` |
| 2 | `ResolvedState` | `LoadedConfig` + `ResolvedGraph` + `model_aliases` |
| 3 | `TargetedState` | `ResolvedState` + `TargetState` + renames + validation warnings |
| 4 | `PlannedState` | `TargetedState` + `SyncPlan` |
| 5 | `AppliedState` | `PlannedState` + `ApplyResult` |
| 6 | `SyncedState` | `AppliedState` + `Vec<TargetSyncOutcome>` |

A `DiagnosticCollector` is threaded through all phases. No `eprintln!` in library code — all warnings/info go through structured diagnostics.

## Step Details

### 1. Load Config (`load_config`)

Acquires `.mars/sync.lock` via advisory file locking, then loads `mars.toml` and `mars.local.toml`. If the command includes a mutation and `mars.toml` doesn't exist, an empty config is created (auto-init for `mars add` on a fresh project).

Under the sync lock, applies the command's mutation atomically:

| Mutation | Source |
|---|---|
| `UpsertDependency` | `mars add` |
| `BatchUpsert` | `mars add source1 source2` |
| `RemoveDependency` | `mars remove` |
| `SetOverride` / `ClearOverride` | `mars override` |
| `SetRename` | `mars rename` |

For `UpsertDependency`, filter replacement is atomic: if any filter field is present in the new entry, the entire filter config replaces the existing one. If no filter fields are set (e.g., version bump only), existing filters are preserved.

Then merges `mars.toml` with `mars.local.toml` overrides into `EffectiveConfig`. For each dependency:

- Validates `url` XOR `path` (exactly one required)
- Validates filter combinations (see [configuration.md](../config/mars-toml.md#filter-mode-rules))
- Applies local overrides (path replaces URL, preserves original git spec)
- Computes `SourceId` for each dependency (git URL or canonical path)
- Rejects `_self` as a dependency name (`_self` is reserved for local package items from the current project)

### 2. Resolve (`resolve_graph`)

Fetches sources, resolves concrete versions, and stages each package.

**Staging seam (dependency packages).** After selecting a concrete version,
`stage_rooted_package` (src/resolve/package.rs:384–408) creates a dialect-scoped
copy of the package under `.mars/staging/<source>/<dialect>/`. The copy lifts
foreign frontmatter spellings (e.g. `allowed-tools` → `tools:`) to canonical,
applies `[skills.<name>]` overlays, and renames items per config. The staged
tree — not the raw source — is what downstream phases hash and discover.
Staging requires `ResolveOptions.staging_root`; when it is unset (e.g. unit
tests), the raw source is used directly.

**Algorithm (src/resolve/mod.rs):**
1. Fetch dependencies from EffectiveConfig
2. Read `mars.toml` manifests in source trees to discover transitive dependencies (including transitive skill dependencies pulled in through agent declarations)
3. Intersect version constraints across dependents
4. Select concrete versions
5. Topological sort (Kahn's algorithm: deps before dependents)

**Version selection strategy:**

| Mode | Strategy | Used by |
|---|---|---|
| Normal | Minimum Version Selection (MVS) | `mars sync`, `mars add` |
| Maximize | Newest compatible version | `mars upgrade` |

**MVS** picks the minimum version satisfying all constraints. This is deterministic and conservative: you get exactly what you asked for, not the newest thing available. `mars upgrade` switches to maximize mode to find the newest compatible version.

**Lock replay:** When a lock file exists, the resolver tries to reuse locked commits for sources whose version constraints haven't changed. This makes `mars sync` deterministic after the first install. Normal sync runs a post-resolution compatible-upgrade check; `--no-upgrade-hint` skips that lookup. In `--frozen` mode, lock replay failures become hard errors (the lock must fully reproduce the previous state).

**Source types:**

| Source | Resolution |
|---|---|
| Git with version constraint | List tags → filter by semver constraint → select version → fetch tree |
| Git without version | Fetch HEAD (default branch tip) |
| Git with ref pin | Fetch the specific branch/commit ref |
| Local path | Resolve to canonical path, no version logic |

Additionally, this phase merges model aliases from the dependency tree. Each resolved dependency's `[models]` config is collected in **declaration order** (the order deps appear in the consumer's `mars.toml`, not alphabetical). `merge_model_config()` applies two layers: dependencies first (declaration-order first-wins on sibling conflicts), consumer config on top (always wins). Within transitive subtrees, each parent's manifest declaration order determines its children's ordering. Diamond deps inherit the position of the earliest direct dep that reaches them. See [configuration.md](../config/mars-toml.md#merge-precedence) for the full precedence rules, conflict warnings, and examples.

### 3. Build Target (`build_target`)

Constructs the desired target state from the resolved graph.

For each source in topological order:

For project-local (`_self`) items, staging runs here rather than during
resolve. `staging::stage_local_item` (src/sync/mod.rs:297–304) applies the
same lift + overlay pipeline as dependency staging, but works from the local
`.mars-src/` tree. Local items are staged before hashing so the sync diff
compares the canonical form, not the raw source.
1. **Discover** items in the source tree with the bounded convention walk (`agents/`, `skills/`, and `bootstrap/` directories at non-hidden depth up to `MAX_DISCOVERY_WALK_DEPTH = 5`, plus package-root `SKILL.md` fallback)
2. **Apply filter** (All, Include, Exclude, OnlySkills, OnlyAgents)
3. **Apply rename** mappings from config
4. **Compute source hash** (SHA-256 of source content)

After building all items:
5. **Detect cross-source destination collisions** — dependency agents and skills from different sources that want the same destination path are both auto-renamed with `__{source_name}`; explicit dependency `rename` mappings run first and can prevent or customize the rename
6. **Check unmanaged collisions** — prune items that would overwrite files not tracked in the lock
7. **Rewrite frontmatter** — build one `RenameIndex` from explicit skill renames and automatic collision renames that survived pruning, then apply one rewrite pass per agent; dependency fallback candidates follow mars.toml declaration order (`frontmatter` is the YAML metadata block at the top of each agent Markdown file)

### 4. Create Plan (`create_plan`)

Computes diff and converts to executable actions.

Compares target state against the lock file and disk to produce diff entries.

Uses dual checksums from the lock:
- `source_checksum`: what the source provided (before any rewriting)
- `installed_checksum`: what mars wrote to disk (after frontmatter rewriting)

The diff matrix:

| Source changed? | Local changed? | Result |
|---|---|---|
| No | No | **Unchanged** (skip) |
| Yes | No | **Update** (clean overwrite) |
| No | Yes | **LocalModified** (keep local) |
| Yes | Yes | **Conflict** → source wins overwrite + warning |
| — | — | **Add** (new item) |
| — | — | **Orphan** (in lock but not in target → remove) |

With `--force`, the baseline for "local changed" shifts to `source_checksum`, so conflicted files are treated as local modifications and get overwritten.

Also injects project-local items under the `_self` source name (`_self` is the reserved local-project source identifier):
- Items from `.mars-src/` are always discovered by the same convention walk as dependency packages, regardless of whether `[package]` is present. Nested `.mars-src/**/agents/`, `.mars-src/**/skills/`, and `.mars-src/**/bootstrap/` folders are included when they are in the shallowest discovered package layer.
- Repo-root `agents/`/`skills/` directories are not local discovery roots; published source packages expose `agents/`, `skills/`, and `bootstrap/` convention folders at non-hidden depth up to `MAX_DISCOVERY_WALK_DEPTH = 5` when consumed as dependencies.
- Dot-prefixed directories are skipped during default discovery. Import a foreign hidden layout explicitly with dependency `subpath` (for example `.claude`) and `dialect`.
- If one source exposes duplicate `(kind, name)` items through convention scanning, manifest declarations, or both, discovery fails with `DiscoveryCollision`; sync does not choose a winner inside that source.

### 5. Apply Plan (`apply_plan`)

Executes planned actions against the `.mars/` canonical store:

| Action | Behavior |
|---|---|
| Install | Atomic write (tmp + rename) or atomic directory install |
| Update | Replace with new source content |
| Overwrite | Replace with source content (conflicts: source wins) |
| Remove | Delete file or directory |
| Skip / KeepLocal | No-op, recorded in outcomes |

Project-local (`_self`) items follow the same Install path as dependency items — there is no special handling for them here.

In `--diff` (dry run) mode, actions are computed but not executed.

### 5.1 Config Entry Compilation (`compile_config_entries`)

Runs after the apply plan and before target sync. Compiles MCP server
registrations and lifecycle hooks from all packages in the resolved graph and
writes them to per-target config files.

**Steps:**

1. Discover `mcp/<name>/mcp.toml` items from each package (local first, then
   dependencies in topological order).
2. Discover `hooks/<name>/hook.toml` items from each package.
3. Filter items whose `visibility` is `"local"` (default) when they originate
   from a dependency — only `"exported"` items cross the package boundary.
4. Check env references — warn (or error under `--strict`) for missing env vars.
5. Resolve per-target MCP name collisions:
   - `_self` (local package) wins over any dependency, silently.
   - Among dependencies, earlier `[dependencies]` declaration order wins; a
     warning is emitted naming both sources.
   - Same declaration order: alphabetically-first source name wins; a warning
     is emitted.
6. Order hooks deterministically: package depth → declaration order → `order`
   field → hook name.
7. Translate universal hook events to native target events (with lossiness
   classification); drop events with no native support and emit a warning.
8. Write entries to target config files via target adapters (`.mcp.json`,
   `settings.json`, etc.). Non-fatal per-target.
9. Compare current config entries against the previous lock to find stale
   entries, then remove them via `adapter.remove_config_entries()`.

In `--diff` (dry run) mode, stale entries are reported as warnings but not
removed; writes are skipped.

See [mcp-and-hooks.md](../config/mcp-and-hooks.md) for the full reference.

### 6. Sync Targets (`sync_targets`)

Copies content from `.mars/` canonical store to each configured target directory (`.agents/`, `.claude/`, etc.). Implemented in `src/target_sync/mod.rs`.

- Targets include the managed root (default: `.agents/`) plus any additional directories added via `mars link` (`settings.targets`)
- All targets get file copies
- Uses `reconcile::fs_ops` for atomic operations (tmp+rename)
- Orphan cleanup: scoped per target via `output_dest_paths_for_target(target_root)` — only removes paths Mars previously managed **in that target** (`target_root` + `dest_path` on lock `OutputRecord`). A `.mars`-only record does not authorize deletes under `.cursor` or other targets. Untracked collisions are preserved unless `mars sync --force` adopts them (see `src/target_sync/.context/CONTEXT.md`)
- Non-fatal per-target: errors on one target are recorded in `TargetSyncOutcome` but don't stop other targets from syncing

### 7. Finalize (`finalize`)

Writes lock and constructs the final `SyncReport`.

- **Lock write**: constructs new `mars.lock` from resolved graph + apply outcomes (checksums). Keys sorted deterministically for clean git diffs. Lock is written **regardless of target sync outcome** — this ensures the lock always reflects what's in `.mars/`, even if a target sync failed.
- **Model aliases**: persists dependency-only alias winners to committed `mars.lock` (`dependency_model_aliases`). Consumer aliases are not baked into lock state, so `mars models list` overlays current consumer config at read time.
- **Validation warnings**: emits diagnostics for missing skill references in agents.
- **Report**: assembles `SyncReport` with apply outcomes, target sync outcomes, diagnostics, and dry-run flag.

## Project-Local Items (`_self`)

Project-local agents and skills are discovered, hashed, and installed into the managed root via the normal sync pipeline — the same install/copy path as dependency items.

**Discovery sources:**

| Source | When included |
|---|---|
| `.mars-src/` | Always; scanned by the same convention walk as dependency packages |

`.mars-src/` is the only project-local source root. Nested `.mars-src/**/agents/`, `.mars-src/**/skills/`, and `.mars-src/**/bootstrap/` convention folders are included when they are in the grounded package layer. Repo-root `agents/` and `skills/` directories remain valid package contents when the project is consumed as a dependency, but `mars sync` no longer scans them as local `_self` items in that same project.

All `_self` items follow the same behavior:
- Shadow external dependency items if names collide (with a warning)
- Cleaned up on the next `mars sync` when removed from source
- Appear in `mars list` output after sync (Mars reads from the lock + `.mars/` cache)
