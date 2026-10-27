# src/resolve/ — Package Resolution

Dependency resolution with semver constraints (the grammar and the version choice are `mars_schema::version`). 14 source files + tests, ~9700 lines.

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
- `rooted_ref.package_root`: starts at the authored root and is replaced with normalized scratch
  materialization only after graph and local-package prepared diagnostics pass.

Binary-owned dependency declarations are parsed once from the prepared normalized manifest.
Agent and skill graph items come from `Prepared.definitions`; consumer filters and renames follow.
Resolver restart passes may repeat candidate preparation while selecting a version, but commands do
not rediscover or revalidate the settled graph.

`ResolveOptions.prepared_sources` seeds a package already prepared by its caller. Plugin builds use
this for their implicit root package so graph resolution does not prepare that root twice.

`ResolveOptions.source_overrides` applies local path overrides by source name while declared source
identity remains separate. Removed hook schema state is binary-owned and recorded in
`ResolvedGraph.unreadable_hook_surfaces`.

## Marketplace plugin dependencies

`plugin = "<name>"` on a dependency installs one plugin of the Claude marketplace at its source.
The selector is part of `SourceId` (`plugin()`), so two plugins of one marketplace are two
packages with distinct identities; the lock matches an entry only while the plugin is the same.

The marketplace itself resolves as any git or path source: version constraints, lock replay,
restarts. `root_package` then roots the result: below `subpath` for an ordinary dependency, or
through `plugin::acquire` for a plugin. `acquire` inspects the marketplace (`mars_schema::inspect`),
finds the plugin, and roots the package at its files: a directory of the marketplace checkout, or
another repository fetched through the same `SourceFetcher` at the entry's `sha`, else `ref`, else
its default branch. The package is then prepared through the entry (`Hint.plugin`) like any other.
`RootedSourceRef.plugin` keeps that entry and the content coordinates, so a restart pass prepares
the same plugin again without fetching, and the lock builder reads `plugin` and `content` from it.
`mars-schema` never fetches; `package_prepare` is the only IO adapter.

- **Replay and refresh.** Sync replays the locked content commit only while the marketplace
  resolves to the locked commit and the entry names the same repository and path; a changed
  marketplace, or `mars upgrade` (`LatestOnly`), resolves the content anew. `--frozen` requires the
  locked content commit. An unreachable locked commit re-resolves with a warning, except frozen.
- **Moves.** A plugin whose repository or path differs from the lock's `content` is reported by
  `plugin-source-moved` ("old -> new").
- **Validation.** Config refuses `plugin` with `subpath` or `dialect` or empty; resolution refuses a
  source that is not a marketplace, an unknown plugin (listing the available ones), `npm`, `archive`
  and `command` sources (naming the shape), and a remote plugin that is itself a marketplace. A
  marketplace dependency with no `plugin` takes the marketplace's only plugin (a lock written for it records no `plugin`, and no `content` for a plugin in the marketplace's own tree, as v0.19.0 wrote it) and is an error that lists the plugins when there are several.
- **Trust.** A marketplace entry is third-party text that reaches `git`'s argument list. The
  crate refuses it when it reads the entry (`parse_source`): the source comes back `unsupported`
  with a reason for a repository or ref that starts with `-` or holds whitespace, a repository
  that is not `https://`, `ssh://` or `user@host:path` (a local repository is refused too), and a
  `sha` that is not 40 hex. The binary acts only on what the crate calls remote, and
  `git_cli.rs` refuses an option-like argument again at the point git runs. GitHub-only hosting
  is Flow's server policy, not Mars's.

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

Resolved item traversal is DFS from seeded agent/skill requests. A skill ref `x__p` that no package ships is deferred (`resolve_skill_ref` returns `None`) when the requesting package's `uses-providers` lists `p`, and is `SkillNotFound` otherwise; the resolver never reads `.mars/provided.lock`, so the answer depends on the package alone, and sync's validator reports what is actually provided. Source items themselves come from `src/discover/`'s convention walk; this phase parses skill frontmatter deps and resolves those references transitively. Version conflicts between packages produce errors (local items skip conflicts).

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
