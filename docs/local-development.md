# Local Development

When developing agents and skills, you need fast iteration: edit source, see changes immediately, without committing and pushing to git. Mars supports several workflows for this.

## `.mars-src/` — project-local agents and skills

`.mars-src/` is the standard place to keep agents and skills that live in this repo rather than being installed from an external package. It works in any project — `[package]` in `mars.toml` is not required.

```
.mars-src/
  agents/
    my-agent.md
  skills/
    my-skill/
      SKILL.md
```

Items in `.mars-src/` are discovered automatically on every `mars sync` and installed into the managed root under the `_self` source name. Edit a file and run `mars sync` to propagate changes.

> **`.mars-src/` vs `.mars/`**: `.mars-src/` is your editable, committed source — put your own agents and skills here. `.mars/` is a gitignored cache directory rebuilt by sync; never edit it directly.

### Adopting existing items

If you already have unmanaged agents or skills sitting in a target directory (e.g., `.agents/`), use `mars adopt` to bring them under `.mars-src/` management in one step:

```bash
mars adopt .agents/agents/my-agent.md
mars adopt .agents/skills/my-skill
```

This moves the item into `.mars-src/`, then syncs so it's immediately tracked. Use `--dry-run` to preview first:

```bash
mars adopt .agents/agents/my-agent.md --dry-run
```

`mars adopt` requires the item to be on the same filesystem as the project root.

### Edit cycle

```bash
# Edit source directly
vim .mars-src/agents/my-agent.md

# Re-sync to propagate changes to all targets
mars sync
```

## `mars override`

The primary mechanism for iterating on an external dependency locally. Overrides swap a git source for a local path without modifying the shared config.

```bash
mars override base --path ../meridian-base
```

This writes to `mars.local.toml`:

```toml
[overrides.base]
path = "../meridian-base"
```

And re-syncs, using the local path instead of the git URL. The original git spec is preserved internally, so:
- Other developers aren't affected (the shared `mars.toml` still points at git)
- `mars doctor` can validate that the override name matches a real dependency
- Removing the override returns to the git source seamlessly

### Override + Sync Cycle

```bash
# Set up override once
mars override base --path ../meridian-base

# Edit agents/skills in ../meridian-base/
vim ../meridian-base/agents/coder.md

# Re-sync to pick up changes
mars sync
```

Path sources have no version caching, so `mars sync` always reads the latest content from the local path.

### Removing Overrides

Edit `mars.local.toml` directly and remove the override entry, then `mars sync`:

```bash
# Remove the override
vim mars.local.toml   # delete the [overrides.base] section

# Sync back to git source
mars sync
```

## `mars.local.toml`

This file is gitignored (added by `mars init`). Each developer can have different overrides.

```toml
[overrides.base]
path = "../meridian-base"

[overrides.dev-workflow]
path = "../meridian-dev-workflow"
```

Rules:
- Override names must match dependency names in `mars.toml`
- If an override references a non-existent dependency, Mars warns but continues
- Overrides replace the source URL but preserve filter and rename config from `mars.toml`

## Local Path Dependencies

For sources that are always local (not published to git), use `path` directly in `mars.toml`:

```toml
[dependencies.my-agents]
path = "../my-agents"
```

This is appropriate when:
- The source is a sibling directory that won't be published
- You're developing a new source package and haven't pushed it yet
- The source is a git submodule (path is relative to project root)

Path sources:
- Always resolve to the canonical filesystem path
- Have no version constraint (no semver tags to check)
- Don't appear in `mars outdated` output
- Re-read content on every `mars sync` (no caching)

## Working with Submodules

If your agent sources are git submodules:

```bash
# Submodule at ./meridian-base/
git submodule add https://github.com/meridian-flow/meridian-base

# Reference as path dependency
# mars.toml:
# [dependencies.base]
# path = "./meridian-base"
```

Or use a git URL dependency with `mars override` for local edits:

```bash
# mars.toml points at git
# [dependencies.base]
# url = "https://github.com/meridian-flow/meridian-base"
# version = "^1.0"

# Override locally to use the submodule checkout
mars override base --path ./meridian-base
```

## Source Package Development

If your project is itself a published source package (i.e., other projects depend on it), add `[package]` to `mars.toml`:

```toml
[package]
name = "my-project-agents"
version = "0.1.0"
```

With `[package]` present, Mars also reads legacy repo-root `agents/` and `skills/` directories in addition to `.mars-src/`. If the same item name exists in both, `.mars-src/` takes precedence (with a warning). See [configuration.md](configuration.md#package-optional) for the full schema.

### Validating Before Publishing

Before publishing a source package, validate its structure:

```bash
mars check
```

This checks frontmatter (the YAML metadata block at the top of agent/skill Markdown files), naming conventions, duplicate names, and skill dependency references. See [commands.md](commands.md#mars-check) for details.

## Workflow Summary

| Scenario | Approach |
|---|---|
| Add agents/skills to this project | Put them in `.mars-src/` |
| Bring an existing unmanaged item under management | `mars adopt <path>` |
| Iterate on a git dependency locally | `mars override <name> --path ../local-checkout` |
| Permanent external local source | `path = "../source"` in `mars.toml` |
| Git submodule source | `path = "./submodule"` in `mars.toml` or override |
| Publish this project as a package | Add `[package]` to `mars.toml` |
| Validate before publishing | `mars check` |
