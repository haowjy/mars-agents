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
cargo build --locked
export MARS_BIN="$MARS_REPO/target/debug/mars"
export SCRATCH="$(mktemp -d)"
mkdir -p "$SCRATCH/home" "$SCRATCH/config" "$SCRATCH/data"
export HOME="$SCRATCH/home" USERPROFILE="$SCRATCH/home"
export XDG_CONFIG_HOME="$SCRATCH/config" APPDATA="$SCRATCH/config"
export XDG_DATA_HOME="$SCRATCH/data" LOCALAPPDATA="$SCRATCH/data"
export MARS_CACHE_DIR="$SCRATCH/.cache/mars"
# Local-only smoke: no network or installed harness discovery.
export MARS_OFFLINE=1
export PATH=""
cd "$SCRATCH"
```

Then run a guide under `tests/smoke/manual/`. A guide requiring Git, catalog
HTTP, or harness discovery must explicitly configure its test tools/server and
PATH; do not silently fall back to the developer environment.

Prefer the local binary while developing:

```bash
"$MARS_BIN" <mars args>
```

For example:

```bash
"$MARS_BIN" init
"$MARS_BIN" models list --json
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
