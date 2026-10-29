# Troubleshooting

## `mars doctor`

The primary diagnostic tool. It reports all detected issues at once.

```bash
mars doctor
```

### What it checks

| Area | What's validated |
|---|---|
| Config | `mars.toml` parses without errors |
| Lock file | `mars.lock` parses without errors |
| File integrity | Every locked item exists on disk |
| Conflict markers | Agent files don't contain `<<<<<<<` / `>>>>>>>` |
| Config-lock consistency | Every dependency in config has a lock entry |
| Skill references | Every agent's declared skill deps exist on disk |
| Legacy item symlinks | Symlinked agents and skills in `.mars/` are reported for normalization |
| Repository hygiene | `.mars/` gitignore coverage and an unconfigured legacy `.agents/` tree |
| Target divergence | Each locked item's target copy matches lock checksum |

### Exit codes

| Code | Meaning |
|---|---|
| 0 | All checks passed |
| 2 | Issues found (printed to stderr) |

### Example output

```
  agents/coder.md has unresolved conflict markers
  dependency `dev` is in config but not in lock — run `mars sync`
  agent `coder` references missing skill `planning` — add a source that provides it
  legacy symlinked agent `coder` detected in managed dir — run `mars sync` to normalize to copied content
```

### Target divergence

Doctor checks each managed target directory against the lock's expected checksums. Missing files and files with unexpected content are reported as warnings with suggestions to run `mars sync --force` or `mars repair` to restore canonical state.

## `mars repair`

When things are broken beyond what `mars sync` can fix.

```bash
mars repair
```

### What it does

1. Loads the lock file. If corrupt, treats it as empty in memory and warns while preserving the on-disk bytes.
2. Runs one forced sync (`--force` mode) to rebuild managed files from config and dependencies.
3. Replaces a corrupt lock only if the complete pipeline reaches atomic finalization. Unreadable removed-schema hooks cause a recovery halt before materialization.

### When to use

- Lock file is corrupt (parse errors)
- Files were manually deleted or moved
- Managed root is in an inconsistent state
- `mars sync` reports errors that don't resolve

### What it doesn't do

- Fix config errors (you need to fix `mars.toml` manually)
- Recover deleted sources (the git cache may need to re-fetch)
- Preserve local modifications (forced sync overwrites everything)

## Common Problems

### "no mars.toml found"

Mars auto-detects from the current directory, then maps the managed root back to the project root (directory containing `mars.toml`), stopping at the nearest `.git` boundary. Solutions:

- Run `mars init` to create the config
- Use `--root <path>` to point directly at the project root (the directory holding `mars.toml`) when auto-detection picks the wrong location
- Make sure you're inside the git repository that contains the project root

### "dependency `X` has both `url` and `path`"

Each dependency must have exactly one source type. Edit `mars.toml` to keep only `url` or `path`.

### "only_skills and only_agents are mutually exclusive"

Invalid filter combination. A dependency can restrict to skills-only or agents-only, not both. Pick one or remove both for all items.

### "lock file error" / "failed to parse mars.lock"

The lock file is corrupt. Run:

```bash
mars repair
```

Repair rebuilds from config and dependencies. It preserves the corrupt lock
bytes until a complete run succeeds, so a recovery halt or other failure
leaves the original evidence on disk.

### "collides with unmanaged path"

A managed item would overwrite a file that Mars doesn't own. Options:
- Move the unmanaged file out of the way
- Rename the managed item: `mars rename agents/conflict.md agents/other-name.md`
- Use `--force` with a command that supports deliberate adoption; repair does not run a special collision-removal retry loop

### Missing files on disk

If `mars doctor` reports missing files:

```bash
mars sync      # Reinstall from dependencies
# or
mars repair    # Full rebuild if sync doesn't fix it
```

### Conflict markers in files

Current sync behavior is source-wins: `mars sync` overwrites local modifications with upstream content rather than producing conflict markers. Conflict markers in managed files would only come from manual edits or legacy state from an older mars version.

If conflict markers are present, edit the file to remove them, then:

```bash
mars resolve                     # Resolve all
mars resolve agents/coder.md    # Resolve specific file
```

### Link points to wrong target

```bash
mars unlink .claude          # Remove old link
mars link .claude            # Re-create correct link
```

If link fails with an unmanaged collision (hand-written file at a path Mars wants to manage):

```bash
mars link .claude --force    # Adopt collision and record ownership in mars.lock
```

### "override `X` references a dependency not in mars.toml"

The `mars.local.toml` has an override for a dependency that doesn't exist in config. Either:
- Add the dependency to `mars.toml`
- Remove the stale override from `mars.local.toml`

### Cache taking too much space

```bash
mars cache info     # See disk usage
mars cache clean    # Remove all cached sources
```

The cache is rebuilt automatically on the next sync.

### Stale lock after dependency changes

If you edited `mars.toml` manually:

```bash
mars sync    # Re-resolve and update lock
```

If `mars sync` fails with frozen mode in CI:

```bash
# Locally:
mars sync          # Update lock
git add mars.lock
git commit -m "Update lock file"
```

### A hook does not fire

Run `mars sync` and read its warnings (with `--json`, the `diagnostics[]` array). Each code
below has its full account in
[What sync reports](../config/mcp-and-hooks.md#what-sync-reports).

| What you see | Cause and fix |
|---|---|
| Hook works on Claude but not on Codex | Codex runs `.codex/hooks.json` only in a trusted project. Trust the project once in Codex. See [Ordering and Codex trust](../config/mcp-and-hooks.md#ordering-and-codex-trust) |
| `codex-hooks-need-trust` | A sync added or moved a handler in `.codex/hooks.json`. Codex skips it, silently, until you review it in `/hooks`. Adding a hook ahead of an existing one moves the later one too |
| `hook-event-skipped` | The target has no counterpart for an event, handler type or matcher. The hook's other events still install. The table in [Which harness gets what](../config/mcp-and-hooks.md#which-harness-gets-what) shows what each harness runs |
| `hook-invalid-kept` | A dependency's `hook.toml` failed validation under this Mars. Mars keeps that hook as the last sync installed it, or installs nothing if there was none. Update Mars, or ask the package for a fix |
| `hook-not-installed` | Mars withheld the hook's config entry because its directory was not installed into that target (an unmanaged or edited copy sits at the path the warning names). Resolve that path, then sync again |
| The hook is on neither warning list and still does nothing | Check the target is linked (`mars link`) and not in the hook's `skip-targets` |
| `mars sync` fails on a package's `hook.toml` | The package uses `[hooks]` and your Mars is older than 0.25. Update Mars |

## Diagnostic Workflow

For any issue, start with:

```bash
mars doctor          # What's wrong?
mars sync            # Can sync fix it?
mars repair          # Nuclear option: rebuild everything
```

If the problem persists after repair, check:
1. Is `mars.toml` valid? (`mars doctor` reports config errors)
2. Are git sources accessible? (network, auth)
3. Is the managed root writable? (permissions)

## Smoke Checks

For a change to how a hook renders for a harness, run `scripts/hook-smoke`. It syncs a scratch project with a guard hook and a context hook, then runs a one-shot prompt on each installed harness and reports whether the guard blocked and the context arrived. `--sync-only` checks the installed files and runs no harness.

For parser, discovery, transport, or sync regressions, use the manual smoke matrices in [Smoke Testing (POSIX)](smoke-testing-posix.md) or [Smoke Testing (Windows)](smoke-testing-windows.md).
