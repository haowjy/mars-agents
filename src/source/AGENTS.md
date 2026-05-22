# src/source/ — Package Sources

Git and local path source fetching with global cache. 7 files, ~2500 lines.

## Mental Model

```
Resolver asks for source → GlobalCache → git clone/fetch or path resolve → ResolvedRef
```

Sources are fetched once into the global cache, then checked out at specific versions/commits for resolution.

## Global Cache

Resolution order:
1. `MARS_CACHE_DIR` env var
2. OS cache directory + `mars/cache`
3. `{cwd}/.mars/cache` fallback

Two subdirectories:
- `archives/` — downloaded archives (tarballs)
- `git/` — bare git clones for version listing and fetching

## Source Types

| Type | Fetching |
|---|---|
| Git (version tag) | `fetch_git_version()` — list tags, pick semver match, checkout |
| Git (ref/branch) | `fetch_git_ref()` — resolve ref, checkout |
| Git (exact commit) | `fetch_git_commit()` — direct checkout |
| Path | `fetch_path()` — canonicalize local path |

## Key Types

- `ResolvedRef` — pinned source reference: version, version_tag, commit, tree_path
- `AvailableVersion` — semver-tagged version from remote: tag, version, commit_id
- `GlobalCache` — cache root with `archives_dir()` and `git_dir()`

## Git Operations

- `git.rs` — version listing, fetching via git CLI
- `git_cli.rs` — git command construction and execution
- `canonical.rs` — URL canonicalization (strip `.git`, normalize protocols)
- `parse.rs` — source URL parsing and validation
- `path.rs` — local path source resolution
- `archive.rs` — tarball download and extraction (for registry sources)

## Patterns

**List versions:**
```rust
let versions = list_versions(&url, &cache)?;
```

**Test with fake cache:**
```rust
let temp = TempDir::new()?;
std::env::set_var("MARS_CACHE_DIR", temp.path());
let cache = GlobalCache::new()?;
// cache.root == temp.path()
```

## See Also

- `src/resolve/AGENTS.md` — consumes sources via `SourceProvider` trait
- `src/platform/` — OS cache root discovery
