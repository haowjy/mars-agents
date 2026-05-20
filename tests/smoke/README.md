# Mars Smoke Tests

LLM-runnable smoke coverage for Mars CLI behavior that is easier to verify from
real commands than from narrow unit tests.

Automated Rust tests still cover stable contracts under `tests/*.rs` and
`tests/launch_bundle/*.rs`. Use these smoke guides when the question is:
"what does the CLI actually do from a fresh project?"

## Layout

| Path | Purpose |
|---|---|
| `tests/smoke/README.md` | Smoke-test index and run conventions. |
| `tests/smoke/manual/` | Manual/LLM-runnable guides that execute real `mars` commands. |

## How to Run a Manual Guide

From the Mars repo root:

```bash
export MARS_REPO="$PWD"
export SCRATCH="$(mktemp -d)"
export MARS_CACHE_DIR="$SCRATCH/.cache/mars"
cd "$SCRATCH"
```

Then run the commands in one guide under `tests/smoke/manual/`.

Prefer the local binary while developing:

```bash
cargo run --manifest-path "$MARS_REPO/Cargo.toml" -- <mars args>
```

For example:

```bash
cargo run --manifest-path "$MARS_REPO/Cargo.toml" -- init
cargo run --manifest-path "$MARS_REPO/Cargo.toml" -- models list --json
```

## LLM Runner Rules

- Use a fresh `SCRATCH` directory per guide.
- Set `MARS_CACHE_DIR` inside `SCRATCH`; do not rely on OS cache locations.
- Prefer `--json` where available and record the actual output shape.
- Treat any panic, traceback, hang, or unexpected non-zero exit as failure.
- Do not require git unless the guide explicitly says it needs git.
- Do not mutate the developer's real `.mars/`, `.codex/`, `.claude/`, `.cursor/`, `.opencode/`, or `.pi/` directories.
- When testing installed/native harness detection, record which harness CLIs are present with `command -v` before interpreting the result.

## First Guides to Add

These are the high-value Mars-owned smoke guides for the launch-bundle/resolver
work:

1. `manual/model-resolution.md` — `mars models list/resolve` candidate ordering,
   aliases, passthrough models, cache states.
2. `manual/launch-bundle.md` — `mars build launch-bundle` with agent and ad-hoc
   launches, model/harness overrides, warnings, prompt surface.
3. `manual/capability-cache.md` — cold/warm/stale probe cache behavior,
   `--no-refresh-models`, and `MARS_CACHE_DIR` isolation.
4. `manual/harness-links.md` — `settings.targets` normalization, known harness
   links versus generic materialization targets.
5. `manual/native-config.md` — target-specific raw config projection such as
   Codex native settings.
6. `manual/pi-routing.md` — Pi candidate behavior and fallback interaction.
