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

## Prepared package seam

After `apply_subpath`, `package_prepare::prepare_rooted` calls `mars_schema::prepare` once for the
candidate. Each final `ResolvedNode` retains the crate result. The node also keeps two filesystem
locations with distinct roles:

- `rooted_ref.authored_root`: immutable origin after subpath; relative manifest dependencies use it
  at every depth and diagnostics render against it.
- `rooted_ref.package_root`: optional normalized materialization for filesystem-based compiler and
  binary-only hook/bootstrap consumers.

Binary-owned dependency declarations are parsed once from the prepared normalized manifest.
Agent and skill graph items come from `Prepared.definitions`; consumer filters and renames follow.
Resolver restart passes may repeat candidate preparation while selecting a version, but commands do
not rediscover or revalidate the settled graph.

`ResolveOptions.source_overrides` applies local path overrides by source name while declared source
identity remains separate. Removed hook schema state is binary-owned and recorded in
`ResolvedGraph.unreadable_hook_surfaces`.

## Provider traits

`VersionLister` lists remote versions and `SourceFetcher` obtains concrete checkouts. Production
manifest semantics do not use `ManifestReader`; its implementation remains test scaffolding for
resolver fixtures that inject manifests without authored files.

## Types

- `PendingSource` — unresolved dependency request and its declared/effective identities
- `PendingItem` — selected Agent or skill waiting for DFS traversal
- `ResolvedNode` — source identity, immutable authored origin, prepared result and parsed manifest
- `ResolvedGraph` — final nodes, filters and deterministic order
- `ResolverContext` — restart, constraint and traversal state

`ResolvedGraph::dependency_depths()` and `dependency_decl_orders()` derive traversal properties
from the graph plus direct dependency order.

## Publish lens

`PublishLens` prepares the authored root and builds the consumer-facing config from normalized
manifest bytes. It ignores `mars.local.toml` and clears local dependencies. Plugin and check graph
resolution use the same source provider and prepared-node path as sync.

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
