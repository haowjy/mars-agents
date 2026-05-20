# Launch Bundle / Resolver Smoke Results

Date run: 2026-05-20
Repo: `/home/jimyao/gitrepos/mars-agents.worktrees/capability-cache-resolver`

## Purpose

Focused smoke evidence for Mars launch-bundle routing and warning semantics after
PR #51 resolver work.

Warning bar used here: user-facing `warnings` should mean an unexpected,
problematic, user-actionable condition. Expected route facts belong in routing
fields/provenance, not warnings.

## Environment inventory

Representative harness inventory on the test machine:

```text
codex -> /home/jimyao/.nvm/versions/node/v24.13.0/bin/codex
claude -> /home/jimyao/.local/bin/claude
opencode -> /home/jimyao/.opencode/bin/opencode
cursor -> /usr/bin/cursor
pi -> /home/jimyao/.local/share/pnpm/pi
mars 0.4.8-rc.2
```

## Ad-hoc OpenAI model routes to Codex when available

Command shape from a plain directory with no `mars.toml`:

```bash
mkdir -p "$SCRATCH/plain"
cd "$SCRATCH/plain"
mars build launch-bundle --model gpt-5.4-mini --json
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
    "model_source": "cli"
  },
  "warnings": []
}
```

Why: OpenAI provider routing tries Codex first. Codex was installed and usable,
so Mars selected it without needing Pi/OpenCode/Cursor fallback.

## Explicit Pi harness passthrough is quiet

Command shape from a plain directory with no `mars.toml`:

```bash
mkdir -p "$SCRATCH/plain"
cd "$SCRATCH/plain"
mars build launch-bundle --model gpt-5.4-mini --harness pi --json
```

Representative output after the warning-semantics fix:

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

Why: CLI harness selection is fixed intent. Pi receives the model token as a
provider-router/runtime concern, so passthrough is an expected route fact, not a
warning.

## Profile Pi harness passthrough is quiet

Profile:

```markdown
---
name: pi-agent
description: Pi profile probe
model: gpt-5.4-mini
harness: pi
---
Pi profile probe body.
```

Command shape:

```bash
mars build launch-bundle --agent pi-agent --json
```

Representative output:

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

Why: profile `harness: pi` is fixed harness intent. Mars does not run backup
harness resolution for profile-fixed harnesses; it compiles the bundle for Pi and
records passthrough as routing metadata.

## Known harness links constrain routing; generic targets do not

Constrained PATH probe hid `codex` while leaving `opencode` and `cursor`
available.

Generic target project:

```toml
[settings]
targets = [".agents"]
```

Representative result:

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
    "harness_source": "provider"
  },
  "warnings": []
}
```

Known harness target project:

```toml
[settings]
targets = [".codex"]
```

Representative result under the same PATH:

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
    "harness_source": "provider"
  },
  "warnings": [
    "known linked harness constraints left no eligible auto-routing candidates; selecting linked harness `codex` without unrelated fallback"
  ]
}
```

Why: `.agents` is materialization-only and does not constrain routing. `.codex`
is a known harness link, so Mars refuses unrelated fallback to OpenCode even when
OpenCode is available. This warning is user-facing because linked-harness
constraints caused degraded fallback behavior.

## Cache locations

With `MARS_CACHE_DIR` set inside scratch state:

```text
$MARS_CACHE_DIR/availability/opencode-probe.json
$MARS_CACHE_DIR/availability/pi.json
$PROJECT/.mars/models-cache.json
```

Why: capability/probe cache is global-ish host state controlled by
`MARS_CACHE_DIR`; model catalog cache remains project-local under `.mars/`.
