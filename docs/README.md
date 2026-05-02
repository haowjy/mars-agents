# Mars Documentation

Mars is a package manager for AI agent profiles. Install agents and skills from git and local sources into Claude Code, Cursor, Codex, OpenCode — any tool that reads from a config directory.

Mars installs into a `.mars/` canonical store, tracks ownership in `mars.lock`, and copies managed content into the configured managed root (`.agents/` by default). Use `mars link` to add additional tool directories (`.claude/`, `.cursor/`, etc.) as managed targets.

## Core Concepts

**Sources** are git repositories or local directories that contain agents and skills. Each source may expose many installable units. Mars resolves versions and **transitive skill dependencies** (skills referenced by installed agents that must also be installed) across sources before installing anything.

**Project root** is the directory containing `mars.toml` and `mars.lock` (typically your repo root).

**Canonical store** (`.mars/`) is where Mars installs resolved content. Gitignored. Rebuilt by `mars sync` from sources + lock.

**Managed root** is the directory Mars copies content into from `.mars/` (default: `.agents/`, configurable via `settings.managed_root`). The managed root may contain non-mars content — mars only manages files it created (tracked in the lock). Additional tool directories (`.claude/`, `.cursor/`) can be added as targets via `mars link`.

**`--root`** points at the project root to override auto-detection. Mars resolves from the directory containing `mars.toml`.

**Lock file** (`mars.lock`) records every managed item with its source, version, and content checksums. It is the authority for what Mars manages. See [internals/lock-file.md](internals/lock-file.md) for format details.

**Filters** control which items from a source get installed: include specific agents/skills, exclude items, or restrict to agents-only or skills-only. See [config/mars-toml.md](config/mars-toml.md).

## Dependency Names vs Source Specifiers

- In `mars.toml`, the key under `[dependencies.<NAME>]` is the dependency name.
- `mars add` derives that name from the source specifier by default (for example, `meridian-flow/meridian-base` becomes `meridian-base`).
- Commands like `mars remove`, `mars override`, `mars upgrade`, and `mars why` operate on dependency names, not source URLs.
- `mars list --status` shows a `SOURCE` column; combine with `--source <name>` to filter to one dependency.

## Quick Start

```bash
# Initialize a mars project and link to Claude
mars init --link .claude

# Add sources
mars add meridian-flow/meridian-base
mars add meridian-flow/meridian-dev-workflow

# See what's installed
mars list

# Explain why an item is present
mars why reviewer
```

## Managed Layout

```
project/
  mars.toml              # Dependency config (committed)
  mars.lock              # Ownership registry (committed)
  mars.local.toml        # Dev overrides (gitignored)
  .mars/                 # Canonical store (gitignored)
    agents/              # Resolved agent profiles
    skills/              # Resolved skills
    models-cache.json    # Cached model catalog
    models-merged.json   # Dependency-sourced model aliases
  .agents/               # Managed root — files copied here (committed)
    agents/
    skills/
  .claude/               # Additional managed target — content copied from .mars/ (committed)
    agents/
    skills/
```

## Source Inputs

Mars accepts several source forms:

| Form | Example |
|---|---|
| GitHub shorthand | `owner/repo` or `owner/repo@^1.0` |
| HTTPS URL | `https://github.com/owner/repo` |
| SSH URL | `git@github.com:owner/repo.git` |
| Local path | `../my-agents` or `/absolute/path` |

## Documentation

### Config

| Document | Contents |
|---|---|
| [config/mars-toml.md](config/mars-toml.md) | `mars.toml` reference: all fields, filter modes, model alias merge precedence, settings |
| [config/agent-profiles.md](config/agent-profiles.md) | Agent profile format: every field, allowed values, harness-overrides, model-policies, fanout, examples |
| [config/agent-compilation.md](config/agent-compilation.md) | How agents compile to native harness formats: field mapping, lossiness, emission control |
| [config/mcp-and-hooks.md](config/mcp-and-hooks.md) | MCP server and hook compilation: declaring, collision resolution, stale cleanup, Windows |

### CLI

| Document | Contents |
|---|---|
| [cli/commands.md](cli/commands.md) | Full CLI reference: every subcommand with flags and examples |

### Internals

| Document | Contents |
|---|---|
| [internals/sync-pipeline.md](internals/sync-pipeline.md) | How sync works: resolve → target → diff → apply → sync targets → finalize |
| [internals/lock-file.md](internals/lock-file.md) | Lock file format and semantics |
| [internals/conflicts.md](internals/conflicts.md) | Collision handling: naming, unmanaged files, merge, resolution |

### Dev

| Document | Contents |
|---|---|
| [dev/local-development.md](dev/local-development.md) | Dev workflows: overrides, local paths, submodules |
| [dev/troubleshooting.md](dev/troubleshooting.md) | `mars doctor`, `mars repair`, common problems |
| [dev/smoke-testing-posix.md](dev/smoke-testing-posix.md) | Manual smoke checks (POSIX) |
| [dev/smoke-testing-windows.md](dev/smoke-testing-windows.md) | Manual smoke checks (Windows) |

## Design Constraints

- **Resolve first, then act.** If resolution fails, nothing is mutated.
- **Atomic writes.** Config, lock, and installed files use tmp+rename.
- **Lock is authority.** `mars.lock` defines what Mars manages.
- **Explicit intent.** User intent comes from flags and arguments, not heuristics.
