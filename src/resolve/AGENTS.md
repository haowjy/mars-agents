# src/resolve/ — Package Resolution

Dependency resolution with semver constraints (the grammar and the version choice are `mars_schema::version`). `select` adapts the settled graph to the pure selection pass in `mars_schema::selection`.

## Mental Model

Two phases. The resolver chooses versions and prepares each package (a package's manifest entries and filters
are read from `ResolvedNode.prepared`, once); it never chooses items. Once the graph settles, selection runs
once over it:

```
Phase 1: Bottom-up package resolution (with restart loop)
  mars.toml dependencies → resolve_package_bottom_up() → registry

Phase 2: selection (pure, no restarts)
  settled graph → mars_schema::selection::select → ResolvedGraph.selection
```

`select.rs` is the adapter: it builds the `GraphView` (requests, packages in registry order, each
package's prepared result and edges, and the bootstrap documents and hooks found in its tree) and
passes it to the crate, which owns the rule. Sync, `mars check` and the plugin build read
`graph.selection`; none of them re-applies a filter.

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

`VersionLister` lists remote versions and `SourceFetcher` obtains concrete checkouts. A package's
manifest is read from its tree like any other file; resolver test fixtures write a `mars.toml` into
the tree (`write_manifest` in `tests/mod.rs`).

## Types

- `PendingSource` — unresolved dependency request and its declared/effective identities
- `ResolvedNode` — source identity, immutable authored origin, prepared result and parsed manifest
- `ResolvedGraph` — final nodes, `selection` and deterministic order
- `ResolverContext` — registry, package states, id index, version constraints and overrides

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

Selection is `mars_schema::selection::select`; the rule is documented there. In short: the consumer's
requests, and the unfiltered entries reached from an unfiltered request through unfiltered entries,
seed items; a transitive manifest's filtered entry seeds nothing and its `exclude` blocks nothing;
the skills a selected Agent names resolve requester-first, then through its dependencies, then in the crate's depth-first registry order (the binary hands over a set). A skill ref `x__p` that no package ships
is skipped when the requesting package's `uses-providers` lists `p`, and is `SkillNotFound`
otherwise (`resolve()` turns the selection's first `missing_skills` entry into that error); selection
never reads `.mars/provided.lock`, so the answer depends on the package alone, and sync's validator
reports what is actually provided. Source items come from `Prepared.definitions` and, for the kinds
the crate does not prepare (bootstrap documents, hooks), from `src/discover/`'s convention walk.
Every constraint on a package must hold on the ref it settles at, whether or not it selects anything: a ref pin holds only at that ref, a range never at a pin, `latest` anywhere (and warns beside a pin). `mars_schema::version::settle` refuses two pins, or a pin beside a range, before any fetch; a constraint arriving after the package resolved re-runs resolution (path sources aside). `validate_all_constraints` re-checks the settled graph and emits the drift warning.

## Patterns

**Test with fake provider:**
```rust
struct FakeProvider;
impl VersionLister for FakeProvider { ... }
impl SourceFetcher for FakeProvider { ... }
let graph = resolve(&config, &FakeProvider, Some(&lock), &options, &mut diag)?;
```

## See Also

- `src/source/AGENTS.md` — how sources are fetched
- `src/config/AGENTS.md` — EffectiveConfig input
- `src/sync/AGENTS.md` — consumes ResolvedGraph
