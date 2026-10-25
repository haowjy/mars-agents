# src/resolve/ — Package Resolution

Dependency resolution with semver constraints. 13 source files + tests, ~9600 lines.

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

`ResolveOptions.staging_root` enables per-dependency canonical source staging.
`stage_rooted_package` runs **after** `apply_subpath` in both resolution paths
(first-resolve and re-resolution). It always calls `package_prepare::classify`
before manifest reading or discovery; classification does not depend on materialization.
When `staging_root` is set:

1. Schema-v1 packages copy without schema rewrites.
2. Legacy packages run `mars_schema::upgrade` and warn once per package.
3. Manifest-free sources resolve their foreign dialect and use the foreign lift.
4. The resolver repoints `package_root` to the staged tree.

All downstream consumers read the prepared tree. A non-materializing resolver may read schema-v1
or manifest-free content after classification; it refuses legacy packages with a `mars migrate`
message. Plugin builds provide a temporary staging root during resolution, so legacy inputs are
materialized before discovery and relative dependency paths remain present in the copied tree.

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
| `RealSourceProvider` (`provider.rs`) | Production bridge from resolver traits to git/path source fetching for sync, link, and publish |

## Types

- `PendingSource` — unresolved dependency request with constraint
- `PendingItem` — agent/skill to resolve from a package
- `ResolvedGraph` — final output: nodes + deterministic alphabetical order
- `ResolverContext` — accumulates registry, visited items, version constraints
- `VersionConstraint` — `Semver`, `Latest`, `RefPin`

`ResolvedGraph::dependency_depths()` and `dependency_decl_orders()` derive traversal properties
from the graph plus the consumer's ordered direct dependencies for compiler ordering; callers do
not reconstruct them in sync or lowering. Depth is the shortest distance from a direct dependency.

## Production Lenses

- `PublishLens` loads the consumer-facing config once: it ignores `mars.local.toml`
  and clears local dependencies while retaining the optional manifest and effective config.
  `resolve_publish_graph(&lens, diag)` resolves it with no lock or project staging;
  source fetches use `GlobalCache` and diagnostics belong to the caller.
  `resolve_publish_graph_from_config` gives internal publish consumers the same provider and
  no-lock boundary with a caller-built consumer view. Resolve relative package dependencies
  before staging: staging changes the package root from which relative paths are interpreted.

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
