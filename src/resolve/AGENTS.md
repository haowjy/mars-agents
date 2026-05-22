# src/resolve/ — Package Resolution

Dependency resolution with semver constraints. 11 files, ~8000 lines.

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

Convergence is guaranteed — versions only move upward toward the lock-preferred/latest-compatible optimum. Oscillation detection reports true per-package ref cycles.

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

Items (agents/skills) are discovered via DFS from seeded requests. Skill frontmatter deps are parsed and resolved transitively. Version conflicts between packages produce errors (local items skip conflicts).

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
