# Lock File

`mars.lock` is the ownership registry for all managed items. It records what Mars installed, where it came from, and what the content looked like. This file is committed to version control.

## Format

TOML format with deterministically sorted keys for clean git diffs.

```toml
version = 3

[dependencies.base]
url = "https://github.com/meridian-flow/meridian-base"
version = "v1.2.0"
commit = "abc123def456"

[dependencies.local]
path = "/home/dev/my-agents"

[dependency_model_aliases.fast]
harness = "codex"
model = "gpt-5"

[items."agent/coder"]
source = "base"
kind = "agent"
version = "v1.2.0"
source_checksum = "sha256:aaa111..."

[[items."agent/coder".outputs]]
target_root = ".mars"
dest_path = "agents/coder.md"
state = "installed"
installed_checksum = "sha256:bbb222..."

[[items."agent/coder".outputs]]
target_root = ".codex"
dest_path = "agents/coder.md"
state = "pending-deletion"

[items."skill/review"]
source = "base"
kind = "skill"
version = "v1.2.0"
source_checksum = "sha256:ddd444..."

[[items."skill/review".outputs]]
target_root = ".mars"
dest_path = "skills/review"
state = "installed"
installed_checksum = "sha256:eee555..."
```

## Schema

### Top Level

| Field | Type | Description |
|---|---|---|
| `version` | integer | Schema version (`3` is current; `2` is temporarily promoted on read) |
| `dependencies` | table | Resolved source entries |
| `dependency_model_aliases` | table | Dependency-only model alias winners (declaration-order merge) |
| `items` | table | Logical items with source checksums and per-target output records |

### `[dependencies.<name>]`

Resolved provenance for each source. Built from the resolved dependency graph, not copied from config.

| Field | Type | Present when | Description |
|---|---|---|---|
| `url` | string | Git source | Git URL |
| `path` | string | Path source | Canonical local path |
| `version` | string | Tagged git source | Resolved version tag (e.g., `v1.2.0`) |
| `commit` | string | Git source | Resolved commit hash |

### `[items."<kind/name>"]`

Each item key is the logical item identity (e.g., `agent/coder`, `skill/review`).

| Field | Type | Description |
|---|---|---|
| `source` | string | Dependency name that provided this item |
| `kind` | string | `"agent"` or `"skill"` |
| `version` | string? | Version from the source's resolved graph node |
| `source_checksum` | string | SHA-256 of the original source content |
| `outputs` | array | Per-target materialized outputs |

### `[[items."<kind/name>".outputs]]`

Each output is one path Mars materialized under one target root.

| Field | Type | Description |
|---|---|---|
| `target_root` | string | Target directory, such as `.mars`, `.codex`, `.pi`, or `.agents` |
| `dest_path` | string | Destination path relative to `target_root` |
| `state` | string | Lifecycle claim: `"installed"` or `"pending-deletion"` |
| `installed_checksum` | string | SHA-256 of what Mars wrote; present only when `state = "installed"` |

An installed record asserts both path ownership and the installed bytes. A
pending-deletion record asserts only the authority to retry removal; it does
not claim that the current bytes were installed by Mars.

## Dual Checksums

Each item tracks two checksums:

- **`source_checksum`**: Hash of the content as it exists in the source tree, before any transformations. Used to detect when the source has changed (new version, upstream edit).

- **`installed_checksum`**: Hash of what Mars actually wrote to one output. May differ from `source_checksum` when frontmatter rewriting or native-target lowering occurred. Used to detect whether that exact output changed locally.

This dual-checksum design enables the [conflict diff](conflicts.md#diff-matrix):
- Source changed? → compare new source hash against `source_checksum`
- Local changed? → compare current disk hash against the `installed_checksum` for the same `(target_root, dest_path)`

## Checksums

Checksums use the format `sha256:<hex>`. For agents (single files), this is the SHA-256 of the file content. For skills (directories), this is a deterministic hash of the directory tree.

## The `_self` Source

Project-local agents and skills in `.mars-src/` appear in the lock under `source = "_self"`. `_self` is the reserved synthetic source name for all items provided by the current project. The `_self` dependency entry uses `path = "."` to indicate the local project.

`[package]` is not required for `_self` items to appear in the lock — any project with content in `.mars-src/` will have them after sync.

```toml
[dependencies._self]
path = "."

[items."skill/local-skill"]
source = "_self"
kind = "skill"
source_checksum = "sha256:..."

[[items."skill/local-skill".outputs]]
target_root = ".mars"
dest_path = "skills/local-skill"
state = "installed"
installed_checksum = "sha256:..."
```

## Lock v3 lifecycle and per-target outputs

Current locks use `version = 3`. Items are keyed by stable id (e.g. `agent/coder`) and may have multiple `[[items."…".outputs]]` rows:

| Field | Description |
|---|---|
| `target_root` | Target directory Mars wrote to (e.g. `.mars`, `.cursor`, `.agents`) |
| `dest_path` | Path relative to that target root |
| `state` | `"installed"` when Mars confirms its bytes are present; `"pending-deletion"` when removal remains to be retried |
| `installed_checksum` | SHA-256 of content at that target path; present only for installed records |

Mars may delete a path in a linked target only when the previous lock contains
a matching `(target_root, dest_path)`. Overwriting existing content additionally
requires an installed checksum claim; pending deletion authorizes only another
removal attempt. The same `dest_path` under `.mars` does not imply ownership
under `.cursor`.

The same rule applies to divergence checks for installed records. Canonical sync diff compares `.mars/` disk content only with installed `.mars` output records. Linked/native targets may have different compiled bytes and different `installed_checksum` values for the same relative `dest_path`. Pending-deletion records retain deletion authority without participating in checksum comparison. See `src/lock/.context/CONTEXT.md` and `src/target_sync/.context/CONTEXT.md`.

Version 2 locks are accepted through a temporary promotion path. Mars checks
each v2 output against disk: matching regular content becomes installed, while
a missing, changed, or non-regular path becomes pending deletion.

## Building the Lock

The lock is rebuilt on every sync from two inputs:

1. **Resolved graph** provides source provenance (URL, version, commit) for dependency entries
2. **Apply outcomes** provide checksums for item entries

Items are categorized by their apply action:

| Action | Lock behavior |
|---|---|
| Installed / Updated | New item entry with computed checksums |
| Kept (local modification preserved) | Carried forward from old lock |
| Skipped | Carried forward from old lock |
| Removed | Excluded from new lock |
| Installed (`_self`) | New item entry with computed checksums |

## Absent Lock File

When `mars.lock` doesn't exist, Mars treats it as empty. The first `mars sync` or `mars add` creates it. A missing lock is not an error.

## Corrupt Lock File

If `mars.lock` fails to parse, Mars reports a `LockError::Corrupt` and suggests running `mars repair`. Repair treats the lock as empty in memory and rebuilds from `mars.toml` plus dependencies. The corrupt on-disk bytes remain untouched unless the complete pipeline succeeds and atomically finalizes the rebuilt lock; a recovery halt or other failure preserves them for diagnosis.

## Atomic Writes

The lock file is written atomically via tmp+rename to prevent corruption from interrupted writes. Keys are sorted (by `IndexMap` insertion order, which the build function ensures is sorted) for deterministic output.
