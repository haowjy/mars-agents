# src/resolve/ — Package Resolution

Dependency resolution with semver constraints. 11 source files + tests, ~9500 lines.

## Mental Model

Two-phase algorithm:

```
Phase 1: Bottom-up package resolution (with restart loop)
  mars.toml dependencies → resolve_package_bottom_up() → registry

Phase 2: DFS item traversal
  seeded items → parse skill deps → resolve skill refs → graph
```

### Restart Algorithm

The bottom-up phase can discover that an already-resolved package would select a different version under the full accumulated constraint set. When this happens, `resolve_package_bottom_up` emits `ResolutionRestartNeeded`. The driver:

1. Reads the "correct" ref from context
2. Carries it as override into a fresh `ResolverContext`
3. Restarts bottom-up from scratch

Convergence is guaranteed by two monotones: constraint-driven selections move
toward the lock-preferred/latest-compatible optimum, while engine-incompatible
`(source, version)` exclusions only accumulate across restarts and move that
optimum downward. Oscillation detection reports true per-package ref cycles.

Package `requires-mars` and `requires-meridian` checks run after manifest read
and before manifest dependency collection. Semver sources walk down to the
newest compatible candidate; path, ref-pinned, and untagged sources hard-error
because they have no alternate candidate.

Resolution-summary diagnostics (e.g. `engine_fallbacks`) are reconciled
against the final `ResolvedGraph` after the restart loop settles — never
trust state accumulated during passes, since a restart can change which
sources and versions appear in the final graph.

## Staging Seam

`ResolveOptions.staging_root` (resolve/types.rs:304) enables per-dependency canonical source staging.
`stage_rooted_package` (package.rs:384–408) runs **after** `apply_subpath` in both resolution paths (first-resolve and re-resolution). When `staging_root` is set:

1. Resolves `Dialect` from `EffectiveDependency.dialect` (via `Dialect::resolve`)
2. Calls `staging::stage_rooted_source`, which lifts foreign frontmatter to canonical form and writes the staged tree to `<staging_root>/<source-name>/<dialect>/`
3. Repoints `package_root` to the staged tree

All downstream consumers (manifest reading, item discovery) transparently read from the staged tree. Without `staging_root`, the raw checkout is used unchanged.

`ResolveOptions.source_overrides` applies project-local path overrides by source
name to both direct and manifest-discovered transitive dependencies. Declared
source identity remains separate from the effective override identity, so an
override cannot collapse conflicting same-name declarations. Staging records
removed-schema package hooks in `ResolvedGraph.unreadable_hook_surfaces`. The
sync recovery gate is its only consumer; downstream compilation has no
unreadable-content exception.

## Key Traits

| Trait | Role |
|---|---|
| `VersionLister` | Lists semver-tagged versions from git remote |
| `SourceFetcher` | Fetches concrete source trees (git version/ref/commit, path) |
| `ManifestReader` | Reads source manifests for transitive deps |
| `SourceProvider` | Composite of all three — production impl |

## Types

- `PendingSource` — unresolved dependency request with constraint
- `PendingItem` — agent/skill to resolve from a package
- `ResolvedGraph` — final output: nodes + deterministic alphabetical order
- `ResolverContext` — accumulates registry, visited items, version constraints
- `VersionConstraint` — `Semver`, `Latest`, `RefPin`

## Version Policy

- **Normal sync**: lock-preferred latest-compatible (replay locked version if constraint allows)
- **Upgrade (`mars upgrade`)**: maximize versions, optionally bump constraints
- **Frozen (`--frozen`)**: error if any change would occur

## Item Resolution

Resolved item traversal is DFS from seeded agent/skill requests. Source items themselves come from `src/discover/`'s convention walk; this phase parses skill frontmatter deps and resolves those references transitively. Version conflicts between packages produce errors (local items skip conflicts).

## Patterns

**Test with fake provider:**
```rust
struct FakeProvider;
impl VersionLister for FakeProvider { ... }
impl SourceFetcher for FakeProvider { ... }
impl ManifestReader for FakeProvider { ... }
let graph = resolve(&config, &FakeProvider, Some(&lock), &options, &mut diag)?;
```

## See Also

- `src/source/AGENTS.md` — how sources are fetched
- `src/config/AGENTS.md` — EffectiveConfig input
- `src/sync/AGENTS.md` — consumes ResolvedGraph
