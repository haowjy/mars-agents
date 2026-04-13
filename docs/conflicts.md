# Conflict and Collision Handling

Mars handles three types of conflicts: naming collisions between sources, unmanaged file collisions, and content conflicts during updates.

## Naming Collisions

When two sources expose an item with the same destination path (e.g., both have `agents/coder.md`), Mars auto-renames both items.

### Auto-Rename

Both colliding items are suffixed with `__{owner}_{repo}` derived from the source URL or dependency name:

```
agents/coder.md  (from base and from dev)
  → agents/coder__meridian-flow_meridian-base.md
  → agents/coder__meridian-flow_meridian-dev-workflow.md
```

Skills follow the same pattern:

```
skills/review  (collision)
  → skills/review__alice_agents
  → skills/review__bob_agents
```

### Frontmatter Rewriting

When skills are auto-renamed, Mars rewrites agent frontmatter (the YAML metadata block at the top of an agent Markdown file) to reference the new skill names. If agent `coder` declares `skills: [review]` and `review` was renamed to `review__alice_agents`, the installed agent file is updated to reference `review__alice_agents`.

The `source_checksum` in the lock file tracks the pre-rewrite hash; `installed_checksum` tracks the post-rewrite hash. This distinction lets Mars detect whether the user modified the file vs. whether Mars's own rewriting changed it.

### Resolving with `mars rename`

Auto-renamed items can be given preferred names:

```bash
mars rename agents/coder__meridian-flow_meridian-base.md agents/coder.md
```

This adds a `rename` mapping in `mars.toml` for the dependency. The rename persists across syncs. If the other colliding source is later removed, the rename mapping still works (it just applies a no-op mapping).

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
