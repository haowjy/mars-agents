# src/config/ — Config Loading & Routing Settings

mars.toml + mars.local.toml schemas, load/save, merge to EffectiveConfig. 5 files + `.context/`, ~3500 lines.

## Mental Model

```
mars.toml (Config) + mars.local.toml (LocalConfig)
    ↓ merge_with_root()
EffectiveConfig { dependencies, settings }
    ↓
pipeline operates on EffectiveConfig only
```

## Key Types

| Type | Role |
|---|---|
| `Config` | Full mars.toml: package, dependencies, local-dependencies, settings, models, agents |
| `LocalConfig` | Gitignored dev overrides: source path swaps, local agent overlays |
| `EffectiveConfig` | Merged result — what the pipeline operates on |
| `EffectiveDependency` | Resolved source with override tracking (`is_overridden`, `original_git`) |

## Dependency Entry Validation

- `url` XOR `path` (not both, not neither)
- Include filters (`agents`/`skills`) XOR `exclude` (not both)
- `only_skills` XOR `only_agents` (not both)
- Category flags cannot combine with include/exclude lists

## Link Normalization (`targets.rs`)

`normalize_link(raw)` is the **single** link-normalization function. `LinkKind` determines routing impact:

| Kind | Examples | Effect on routing |
|---|---|---|
| `KnownHarness` | `claude`, `.claude`, `codex`, etc. | Produces `linked_harnesses` — filters candidates |
| `GenericTarget` | `agents`, `.agents`, `.foo` | Materialization only — invisible to routing |
| `PathLike` | `path/to/dir`, `C:\foo` | Materialization only — invisible to routing |

**Invariant:** Adding `.agents` to `settings.targets` must never change harness routing.

## Routing Settings (`routing_settings.rs`)

`resolve(settings)` converts raw `Settings` into `ResolvedRoutingSettings`. Both models commands and build policy must consume this typed result — they must not re-parse harness names independently.

Invalid harness names produce diagnostics (not hard errors) so commands continue with degraded config.

## Model Policies

`ModelPolicyRule` — match by `model`, `alias`, or `model-glob` with optional overrides and `no-fallback` flag. Used in agent overlays, settings, and profile frontmatter.

## Save Roundtrip Validation

`save()` validates the serialized config roundtrips identically before writing. Prevents data loss from serialization bugs.

## Patterns

**Load and merge:**
```rust
let config = load(project_root)?;
let local = load_local(project_root)?;
let (effective, diagnostics) = merge_with_root(config, local, project_root)?;
```

**Link inspection:**
```rust
let links = config.settings.effective_links();
let linked = links.linked_harnesses();  // routing constraints
let targets = links.managed_targets();  // materialization paths
```

## See Also

- `.context/CONTEXT.md` — link normalization contracts, routing settings boundary
- `src/routing/AGENTS.md` — consumes `linked_harnesses` from config
- `src/sync/AGENTS.md` — uses EffectiveConfig as input
