# Launch Bundle / Resolver Smoke Results

Date run: 2026-05-20
Repo: `/home/jimyao/gitrepos/mars-agents.worktrees/capability-cache-resolver`
Scratch root: `/tmp/tmp.jn6iDu2lBm`
Project cache root: scratch-local `.mars/`
Probe cache root: `MARS_CACHE_DIR=/tmp/tmp.jn6iDu2lBm/.cache/mars`

## Scope

Real-command smoke probes for the new resolver / launch-bundle behavior.

Goals covered:
- environment inventory
- ad-hoc `build launch-bundle`
- explicit harness overrides
- linked-harness constraints versus generic targets
- agent-profile launch bundles
- `models resolve` / `models list` parity checks
- cache write locations under isolated scratch state

## Syntax discovered from `--help`

Commands used after checking help:

```bash
cargo run --manifest-path "$MARS_REPO/Cargo.toml" -- build launch-bundle --help
cargo run --manifest-path "$MARS_REPO/Cargo.toml" -- models resolve --help
cargo run --manifest-path "$MARS_REPO/Cargo.toml" -- models list --help
```

Observed syntax:

- `mars build launch-bundle [--agent <name> | --model <token>] [--harness <claude|codex|opencode|cursor|pi>]`
- `mars models resolve <NAME> [--no-refresh-models]`
- `mars models list [--all|--catalog|--unavailable|--no-refresh-models]`

## 1) Environment inventory

Command (exit 0):

```bash
for cmd in codex claude opencode cursor pi; do command -v "$cmd" || true; done
MARS_CACHE_DIR="$MARS_CACHE_DIR" cargo run --manifest-path "$MARS_REPO/Cargo.toml" -- --version
```

Output:

```text
codex -> /home/jimyao/.nvm/versions/node/v24.13.0/bin/codex
claude -> /home/jimyao/.local/bin/claude
opencode -> /home/jimyao/.opencode/bin/opencode
cursor -> /usr/bin/cursor
pi -> /home/jimyao/.local/share/pnpm/pi
mars 0.4.8-rc.2
```

Why this matters:
- Native harness presence is real on this machine, so resolver results are not synthetic.
- Because `codex`, `claude`, `opencode`, `cursor`, and `pi` all exist on `PATH`, candidate selection is driven by provider match and probe gates rather than by total harness absence.

## 2) Fresh project init + ad-hoc launch bundle (`gpt-5.4-mini`, no agent)

Setup command (exit 0):

```bash
mkdir -p "$SCRATCH/proj-default"
cd "$SCRATCH/proj-default"
MARS_CACHE_DIR="$MARS_CACHE_DIR" "$MARS_REPO/target/debug/mars" init --json
```

Output:

```json
{"already_initialized":false,"links":[],"managed_root":null,"ok":true,"project_root":"/tmp/tmp.jn6iDu2lBm/proj-default"}
```

Generated `mars.toml`:

```toml
[dependencies]

[settings]
models_cache_ttl_hours = 24
```

Launch-bundle command (exit 0):

```bash
cd "$SCRATCH/proj-default"
MARS_CACHE_DIR="$MARS_CACHE_DIR" "$MARS_REPO/target/debug/mars" build launch-bundle --model gpt-5.4-mini --json
```

Representative output:

```json
{
  "agent": null,
  "routing": {
    "model": "gpt-5.4-mini",
    "harness": "codex",
    "route_confidence": "confirmed",
    "harness_model": "gpt-5.4-mini",
    "harness_model_source": "provider-match",
    "harness_model_confidence": "likely"
  },
  "provenance": {
    "candidates_tried": "codex",
    "harness_source": "provider",
    "model_source": "cli",
    "route_confidence": "confirmed"
  },
  "warnings": []
}
```

Why this happened:
- `gpt-5.4-mini` is treated as an OpenAI model token.
- Provider candidate order tried `codex` first for an OpenAI model.
- `codex` was installed, so Mars resolved immediately without needing `pi`, `opencode`, or fallback.
- This is ad-hoc mode, so `agent` is `null`, execution policy fields are unset, and the prompt surface contains only the generic report contract.

## 3) Same model with explicit `--harness pi`

Command (exit 0):

```bash
cd "$SCRATCH/proj-default"
MARS_CACHE_DIR="$MARS_CACHE_DIR" "$MARS_REPO/target/debug/mars" build launch-bundle --model gpt-5.4-mini --harness pi --json
```

Representative output:

```json
{
  "routing": {
    "harness": "pi",
    "route_confidence": "explicit",
    "harness_model": "gpt-5.4-mini",
    "harness_model_source": "passthrough",
    "harness_model_confidence": "unknown"
  },
  "provenance": {
    "candidates_tried": "pi",
    "harness_source": "cli",
    "model_source": "cli",
    "route_confidence": "explicit"
  },
  "warnings": [
    "model 'gpt-5.4-mini' does not have a confirmed runnable path for harness 'pi'; using passthrough path 'gpt-5.4-mini'",
    "harness-model for 'gpt-5.4-mini' targeting 'pi' is unconfirmed (passthrough)",
    "model 'gpt-5.4-mini' not found in models cache; passing through as harness model ID"
  ]
}
```

Why this happened:
- CLI harness override is authoritative, so Mars did not auto-route elsewhere.
- Pi did not have a confirmed runnable path for this model token, so Mars kept the explicit harness and used passthrough model text with warnings.
- This confirms explicit harness selection bypasses normal provider-driven routing but still surfaces confidence loss.

## 4) Same model with explicit native harnesses

### 4a) Explicit `--harness codex`

Command (exit 0):

```bash
cd "$SCRATCH/proj-default"
MARS_CACHE_DIR="$MARS_CACHE_DIR" "$MARS_REPO/target/debug/mars" build launch-bundle --model gpt-5.4-mini --harness codex --json
```

Representative output:

```json
{
  "routing": {
    "harness": "codex",
    "route_confidence": "explicit",
    "harness_model": "gpt-5.4-mini",
    "harness_model_source": "provider-match",
    "harness_model_confidence": "likely"
  },
  "warnings": []
}
```

Why this happened:
- Explicit codex override matched the OpenAI provider cleanly.
- The result stayed warning-free because the native harness matched the model family.

### 4b) Extra edge probe: explicit `--harness claude`

Command (exit 0):

```bash
cd "$SCRATCH/proj-default"
MARS_CACHE_DIR="$MARS_CACHE_DIR" "$MARS_REPO/target/debug/mars" build launch-bundle --model gpt-5.4-mini --harness claude --json
```

Representative output:

```json
{
  "routing": {
    "harness": "claude",
    "route_confidence": "explicit",
    "harness_model": "gpt-5.4-mini",
    "harness_model_source": "passthrough",
    "harness_model_confidence": "unknown"
  },
  "warnings": [
    "model 'gpt-5.4-mini' does not have a confirmed runnable path for harness 'claude'; using passthrough path 'gpt-5.4-mini'",
    "harness-model for 'gpt-5.4-mini' targeting 'claude' is unconfirmed (passthrough)",
    "model 'gpt-5.4-mini' not found in models cache; passing through as harness model ID"
  ]
}
```

Why this happened:
- Same explicit-override rule as Pi: Mars obeyed the requested harness.
- Because the provider did not natively match Claude routing, Mars downgraded to passthrough and warned.

## 5) Project linked to `.codex`

Setup command (exit 0):

```bash
mkdir -p "$SCRATCH/proj-codex-link"
cd "$SCRATCH/proj-codex-link"
MARS_CACHE_DIR="$MARS_CACHE_DIR" "$MARS_REPO/target/debug/mars" init --link .codex --json
```

Observed output shape:

```text
{"diagnostics":[],"ok":true,"removed":0,"settings_updated":true,"synced":0,"target":".codex"}
{"already_initialized":false,"links":[".codex"],"managed_root":null,"ok":true,"project_root":"/tmp/tmp.jn6iDu2lBm/proj-codex-link"}
```

Generated config:

```toml
[dependencies]

[settings]
targets = [".codex"]
models_cache_ttl_hours = 24
```

Launch-bundle command (exit 0):

```bash
cd "$SCRATCH/proj-codex-link"
MARS_CACHE_DIR="$MARS_CACHE_DIR" "$MARS_REPO/target/debug/mars" build launch-bundle --model gpt-5.4-mini --json
```

Representative output:

```json
{
  "routing": {
    "harness": "codex",
    "route_confidence": "confirmed"
  },
  "provenance": {
    "candidates_tried": "codex",
    "harness_source": "provider"
  },
  "warnings": []
}
```

Why this happened:
- The known harness link did not change the visible result for this model because provider routing already preferred `codex` and found it immediately.
- The interesting effect of linked-harness constraints only shows up when the preferred linked harness is unavailable and Mars would otherwise fall through to unrelated harnesses.

## 6) Generic `.agents` target does **not** constrain routing; `.codex` link **does**

This is where linked-harness semantics became visible.

I created a custom `PATH` containing `opencode` and `cursor`, but **not** `codex`. That left OpenAI routing with no native Codex executable while still leaving a cross-provider fallback surface available.

Setup commands (exit 0):

```bash
mkdir -p "$SCRATCH/harness-bin-opencode-cursor"
ln -sf "$(command -v opencode)" "$SCRATCH/harness-bin-opencode-cursor/opencode"
ln -sf "$(command -v cursor)" "$SCRATCH/harness-bin-opencode-cursor/cursor"

mkdir -p "$SCRATCH/proj-agents-link"
cd "$SCRATCH/proj-agents-link"
MARS_CACHE_DIR="$MARS_CACHE_DIR" "$MARS_REPO/target/debug/mars" init --link .agents --json
```

Probe PATH:

```text
/tmp/tmp.jn6iDu2lBm/harness-bin-opencode-cursor:/usr/bin:/bin
```

### 6a) Generic `.agents` project

Command (exit 0):

```bash
cd "$SCRATCH/proj-agents-link"
MARS_CACHE_DIR="$MARS_CACHE_DIR" \
PATH="$SCRATCH/harness-bin-opencode-cursor:/usr/bin:/bin" \
"$MARS_REPO/target/debug/mars" build launch-bundle --model gpt-5.4-mini --json
```

Representative output:

```json
{
  "routing": {
    "harness": "opencode",
    "route_confidence": "likely",
    "harness_model": "openai/gpt-5.4-mini",
    "harness_model_source": "cached-probe",
    "harness_model_confidence": "confirmed"
  },
  "provenance": {
    "candidates_tried": "codex,pi,opencode",
    "harness_source": "provider",
    "route_confidence": "likely"
  },
  "warnings": []
}
```

Why this happened:
- `.agents` is a generic materialization target, not a known harness link.
- With `codex` hidden and Pi not compatible, Mars was free to continue to `opencode`.
- `opencode` had positive cached capability evidence, so Mars selected it with `likely` route confidence and a confirmed harness-model mapping.

### 6b) `.codex`-linked project under the same PATH

Command (exit 0):

```bash
cd "$SCRATCH/proj-codex-link"
MARS_CACHE_DIR="$MARS_CACHE_DIR" \
PATH="$SCRATCH/harness-bin-opencode-cursor:/usr/bin:/bin" \
"$MARS_REPO/target/debug/mars" build launch-bundle --model gpt-5.4-mini --json
```

Representative output:

```json
{
  "routing": {
    "harness": "codex",
    "route_confidence": "passthrough",
    "harness_model": "gpt-5.4-mini",
    "harness_model_source": "provider-match",
    "harness_model_confidence": "likely"
  },
  "provenance": {
    "candidates_tried": "codex,codex",
    "harness_source": "provider",
    "route_confidence": "passthrough"
  },
  "warnings": [
    "known linked harness constraints left no eligible auto-routing candidates; selecting linked harness `codex` without unrelated fallback"
  ]
}
```

Why this happened:
- `.codex` is a **known harness** link, so it gates auto-routing candidates.
- Once `codex` was unavailable on `PATH`, Mars did **not** fall through to unrelated `opencode` even though `opencode` was installed and had positive cached probe evidence.
- Instead it stayed inside the linked-harness boundary, selected `codex` without unrelated fallback, and warned about the constraint.
- This is the cleanest observed proof that generic targets do not constrain routing, while known harness links do.

## 7) Minimal agent profile under `.mars/agents/reviewer.md`

Profile file created:

```markdown
---
name: reviewer
description: Reviewer smoke probe
model: gpt-5.4-mini
approval: confirm
sandbox: read-only
tools:
  bash: allow
  write: deny
---
Review the supplied changes and explain the top risks.
```

Bundle command (exit 0):

```bash
cd "$SCRATCH/proj-default"
MARS_CACHE_DIR="$MARS_CACHE_DIR" "$MARS_REPO/target/debug/mars" build launch-bundle --agent reviewer --json
```

Representative output:

```json
{
  "agent": "reviewer",
  "routing": {
    "harness": "codex",
    "route_confidence": "confirmed"
  },
  "execution_policy": {
    "approval": "confirm",
    "sandbox": "read-only"
  },
  "prompt_surface": {
    "system_instruction": "# Agent Profile\n\nReview the supplied changes and explain the top risks.\n\n# Meridian Agents ...",
    "inventory_prompt": "# Meridian Agents ... reviewer: Reviewer smoke probe | Model: gpt-5.4-mini"
  },
  "tools": {
    "allowed": ["shell"],
    "disallowed": ["file_write"],
    "mcp": []
  },
  "provenance": {
    "approval_source": "profile",
    "model_source": "profile",
    "sandbox_source": "profile"
  },
  "warnings": []
}
```

Why this happened:
- Agent mode loads policy from `.mars/agents/reviewer.md` instead of relying on ad-hoc CLI-only inputs.
- The profile body became part of `prompt_surface.system_instruction`.
- Profile execution policy survived into the bundle (`approval=confirm`, `sandbox=read-only`).
- Tool policy was normalized for the target harness: `bash` became `shell`, `write: deny` became `file_write` in the disallowed set.
- Compared with ad-hoc mode, the bundle is materially richer even though routing still landed on the same harness.

## 8) `mars models resolve` and `mars models list` compared with launch-bundle routing

### 8a) `models resolve gpt-5.4-mini`

Command (exit 0):

```bash
cd "$SCRATCH/proj-default"
MARS_CACHE_DIR="$MARS_CACHE_DIR" "$MARS_REPO/target/debug/mars" models resolve gpt-5.4-mini --json
```

Representative output:

```json
{
  "availability": "runnable",
  "availability_source": "harness_installed",
  "harness": "codex",
  "harness_candidates": ["codex", "pi", "opencode", "cursor"],
  "harness_source": "pattern_guess",
  "model_id": "gpt-5.4-mini",
  "provider": "openai",
  "resolved_model": "gpt-5.4-mini",
  "runnable_paths": [
    {"harness": "codex", "harness_model_id": "gpt-5.4-mini", "mars_provider": "openai"},
    {"harness": "opencode", "harness_model_id": "openai/gpt-5.4-mini", "mars_provider": "openai"}
  ],
  "source": "passthrough",
  "warning": "model 'gpt-5.4-mini' not found in catalog, passing through to harness"
}
```

Why this happened:
- `models resolve` agreed with launch-bundle on the preferred runnable path: `codex` first, `opencode` also possible.
- The warning explains why some bundle probes emitted passthrough notes: this token was not found in the local catalog cache, so Mars reasoned from provider/path knowledge instead of a catalog entry.

### 8b) `models list`

Command (exit 0):

```bash
cd "$SCRATCH/proj-default"
MARS_CACHE_DIR="$MARS_CACHE_DIR" "$MARS_REPO/target/debug/mars" models list --json
```

Representative output excerpts:

```json
{
  "aliases": [
    {
      "name": "codex",
      "harness": "codex",
      "resolved_model": "gpt-5.3-codex",
      "availability": "runnable"
    },
    {
      "name": "gpt",
      "harness": "codex",
      "resolved_model": "gpt-5.5",
      "availability": "runnable"
    },
    {
      "name": "gemini",
      "harness": "opencode",
      "resolved_model": "gemini-3.1-pro-preview",
      "availability": "runnable"
    }
  ],
  "cache_available": true,
  "probe_results": {
    "opencode": {
      "models_found": 420,
      "providers_found": ["openrouter", "openai", "google", "opencode"],
      "success": true
    },
    "pi": {
      "compatible": false,
      "missing_surface_tokens": ["--mode", "rpc", "--model", "--append-system-prompt", "--session", "--fork", "--session-dir | PI_CODING_AGENT_SESSION_DIR", "--no-extensions", "--no-skills", "--no-context-files", "--no-prompt-templates", "-e | --extension"],
      "version": null
    }
  }
}
```

Why this happened:
- `models list` exposed the same cross-harness facts that launch-bundle later used:
  - OpenAI aliases prefer `codex` when present.
  - `opencode` has positive probe evidence and many discovered models.
  - Pi is installed but not compatible with the required surface on this machine, which explains why it never won auto-routing.

## 9) Cache / state write locations after the probes

Commands (exit 0):

```bash
find "$MARS_CACHE_DIR" -maxdepth 4 \( -type f -o -type d \) | sort
find "$SCRATCH/proj-default/.mars" -maxdepth 3 \( -type f -o -type d \) | sort
```

Observed `MARS_CACHE_DIR` contents:

```text
/tmp/tmp.jn6iDu2lBm/.cache/mars/availability/.opencode-probe.lock
/tmp/tmp.jn6iDu2lBm/.cache/mars/availability/.pi.lock
/tmp/tmp.jn6iDu2lBm/.cache/mars/availability/opencode-probe.json
/tmp/tmp.jn6iDu2lBm/.cache/mars/availability/pi.json
```

Observed project-local `.mars/` contents for `proj-default`:

```text
/tmp/tmp.jn6iDu2lBm/proj-default/.mars/.models-cache.lock
/tmp/tmp.jn6iDu2lBm/proj-default/.mars/agents/reviewer.md
/tmp/tmp.jn6iDu2lBm/proj-default/.mars/models-cache.json
```

Why this happened:
- Probe/capability cache writes went under the isolated `MARS_CACHE_DIR` scratch path.
- The model catalog cache was project-local in `.mars/models-cache.json`.
- In practice this means the resolver used **two** state zones during the smoke run:
  - scratch-global probe cache under `MARS_CACHE_DIR`
  - per-project model cache under `.mars/`

## Key findings

1. Ad-hoc OpenAI launch-bundle routing chose `codex` immediately when `codex` was present.
2. Explicit harness overrides are honored even when confidence drops to passthrough.
3. Pi was present on `PATH` but effectively ruled out by capability incompatibility evidence.
4. `opencode` became the winning route only when `codex` was hidden and generic routing was still allowed.
5. Known harness links are real routing constraints: `.codex` prevented unrelated fallback to `opencode` under the same PATH conditions.
6. Generic `.agents` targets do not constrain routing.
7. Agent-mode bundles correctly injected profile prompt text, execution policy, and normalized tool policy.
8. `models resolve` and launch-bundle agreed on the same OpenAI runnable paths.
9. Cache isolation worked: probe cache stayed in `MARS_CACHE_DIR`; model catalog cache stayed in each project's `.mars/`.

## Surprises / notable details

- `init --link .codex --json` emitted a link result JSON object before the final init result JSON object.
- In the constrained `.codex` probe with `codex` removed from `PATH`, `provenance.candidates_tried` was `codex,codex`. That looks consistent with “try candidate, then constrained fallback to linked harness”, but it is worth noting because it is visually unusual.

## Post-fix focused verification: explicit/profile harness passthrough warnings

After adjusting warning policy, I reran focused probes in fresh scratch state.

Command shape:

```bash
cargo build -q
export MARS_CACHE_DIR="$SCRATCH/.cache/mars"
mars build launch-bundle --model gpt-5.4-mini --harness pi --json
mars build launch-bundle --agent pi-agent --json   # profile has `harness: pi`
```

Explicit CLI harness result:

```json
{
  "routing": {
    "harness": "pi",
    "route_confidence": "explicit",
    "harness_model": "gpt-5.4-mini",
    "harness_model_source": "passthrough",
    "harness_model_confidence": "unknown"
  },
  "provenance": {
    "candidates_tried": "pi",
    "harness_source": "cli",
    "model_source": "cli",
    "route_confidence": "explicit"
  },
  "warnings": []
}
```

Profile harness result:

```json
{
  "routing": {
    "harness": "pi",
    "route_confidence": "passthrough",
    "harness_model": "gpt-5.4-mini",
    "harness_model_source": "passthrough",
    "harness_model_confidence": "unknown"
  },
  "provenance": {
    "candidates_tried": "",
    "harness_source": "profile",
    "model_source": "profile",
    "route_confidence": "passthrough"
  },
  "warnings": []
}
```

Why this is now correct:

- CLI/profile harness selections are fixed harness intent.
- Passthrough model text is expected for provider-router harnesses such as Pi.
- Mars should warn when automatic routing loses confidence or when a fixed harness actually falls back away from the requested harness, not merely because an explicit/profile harness uses passthrough model text.
