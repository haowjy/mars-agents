# Manual Mars Smoke Guides

This directory is for LLM-runnable command guides. Each guide should be concrete
enough that an agent can execute it, paste notable output into a result file, and
say pass/fail without needing hidden project knowledge.

## Guide Template

Use this structure for each smoke guide:

```markdown
# <Behavior> Smoke Test

## Purpose

One or two sentences describing the user-visible behavior under test.

## Preconditions

- Required harness CLIs, if any.
- Required auth/cache/network state, if any.
- Whether git is required.

## Setup

\`\`\`bash
export MARS_REPO="/abs/path/to/mars-agents"
export SCRATCH="$(mktemp -d)"
export MARS_CACHE_DIR="$SCRATCH/.cache/mars"
cd "$SCRATCH"
\`\`\`

## Steps

\`\`\`bash
cargo run --manifest-path "$MARS_REPO/Cargo.toml" -- <command>
\`\`\`

## Expected

- Exit code expectations.
- Key JSON fields or files that must exist.
- Warnings that are expected or forbidden.

## Result Notes

Record actual outputs, environment facts, and any deviation.
```

## Scope Boundary

Mars manual smoke guides should verify Mars-owned behavior:

- package sync/materialization into target directories
- model alias resolution and model catalog/cache behavior
- harness candidate resolution
- launch-bundle JSON shape
- native harness config projection
- Codex rules / prompt surface compilation
- Pi/OpenCode/Cursor fallback behavior

Meridian-owned behavior belongs in the Meridian repo. Meridian should only smoke
that it calls Mars and consumes the returned launch bundle correctly.

## Result File Convention

When a long manual run needs durable notes, write them under a scratch path, not
inside the repo by default:

```bash
RESULTS="$SCRATCH/mars-smoke-results.md"
```

Only commit result files when the user explicitly asks for a checked-in evidence
artifact.
