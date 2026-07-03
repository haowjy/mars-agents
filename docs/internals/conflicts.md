# Conflict and Collision Handling

Mars handles three types of conflicts: naming collisions between sources, unmanaged file collisions, and content conflicts during updates.

MCP server and hook name collisions are resolved separately — see [mcp-and-hooks.md](../config/mcp-and-hooks.md#collision-resolution).

## Naming Collisions

Mars treats within-source duplicates as errors. Across dependency sources, Mars
keeps both colliding items by making their installed names explicit.

### Within one source: `DiscoveryCollision`

If one source exposes two items with the same `(kind, name)`, discovery fails with `DiscoveryCollision`. This applies to duplicates found by convention scanning, plugin-manifest declarations, or both. There is no precedence rule inside a source.

### Across sources: auto-rename both

If two dependency sources would install an agent or skill to the same destination
path, sync auto-renames both colliders by appending `__{source_name}` to the
installed name:

- `agents/coder.md` from `base` → `agents/coder__base.md`
- `skills/planning` from `team` → `skills/planning__team`

Mars emits an `auto-rename-collision` warning for each automatic rename. Agent
frontmatter references are rewritten in scope: `skills:` references follow
renamed skills, and `subagents:` references follow renamed agents from the same
source or that source's dependency graph.

To choose custom names or prevent auto-renaming, add an explicit dependency
rename in `mars.toml`; explicit renames are applied before collision detection:

```toml
[dependencies.base]
url = "https://github.com/meridian-flow/meridian-base"
rename = { "agents/coder.md" = "agents/base-coder.md" }
```

When an explicit skill rename changes the installed skill name, Mars also
rewrites dependent agent frontmatter (the YAML metadata block at the top of an
agent Markdown file) to reference the installed skill name.

## Unmanaged File Collisions

When sync would install an item at a path where an unmanaged file already exists (not tracked in `mars.lock`), Mars skips the install and warns:

```
warning: source `base` collides with unmanaged path `agents/custom.md` — leaving existing content untouched
```

The item is removed from the target state, so the unmanaged file is preserved. This protects user-created local agents and skills from being overwritten.

During `mars repair` with a corrupt lock file, unmanaged collisions are handled more aggressively: the colliding path is removed and sync retries, since there's no lock to distinguish managed from unmanaged files.

## Content Conflicts

When both the source and local disk have changed for a managed item, source always wins — Mars overwrites the local file with the new source content and emits a warning.

### Diff Matrix

| Source changed? | Local changed? | Action |
|---|---|---|
| No | No | Skip (unchanged) |
| Yes | No | Update (clean overwrite) |
| No | Yes | Keep local modification |
| Yes | Yes | **Source wins** — overwrite + warning |

"Local changed" is determined by comparing the current disk hash against `installed_checksum` in the lock file. "Source changed" compares the new source hash against `source_checksum` in the lock.

### Warning

When a conflict is detected, Mars emits a `conflict-overwrite` warning before overwriting:

```
warning: agent `coder` has local modifications — overwriting with upstream
```

Both agents and skills use the same strategy: source wins, no merge attempted.

### Force Flag

`--force` suppresses the `conflict-overwrite` warning. Since source already wins by default, `--force` primarily signals intent — it also causes `LocalModified` entries (only local changed) to be overwritten rather than preserved.

### Resolving Conflict Markers

If a file contains conflict markers from a manual edit or a legacy sync, use `mars resolve` to clear them once you've resolved the markers by hand:

```bash
# Edit the file to fix markers
vim .agents/agents/coder.md

# Mark as resolved
mars resolve agents/coder.md

# Or resolve all at once
mars resolve
```

If conflict markers are still present, `mars resolve` reports the file as still conflicted and exits with code 1.

## Exit Codes

`mars sync` and `mars resolve` exit with code 1 when unresolved conflicts remain. Use `mars list --status` to see which items are conflicted, or `mars doctor` to check for conflict markers.
