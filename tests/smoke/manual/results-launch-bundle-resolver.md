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
  "version": 2,
  "agent": null,
  "routing": {
    "model": "gpt-5.4-mini",
    "harness": "codex",
    "selection_kind": "auto",
    "match_evidence": "confirmed",
    "harness_model": "gpt-5.4-mini",
    "harness_model_source": "provider-match",
    "harness_model_confidence": "likely"
  },
  "provenance": {
    "candidates_tried": "codex",
    "harness_source": "provider",
    "model_source": "cli",
    "selection_kind": "auto",
    "match_evidence": "confirmed"
  },
  "warnings": []
}
```

Why: OpenAI provider routing tries Codex first. Codex was installed and usable,
so Mars selected it without needing Pi/OpenCode/Cursor fallback.

## Explicit Pi harness resolves qualified harness_model from probe

Command shape from a plain directory with no `mars.toml` (real `pi` on PATH with
`--list-models` showing `openai-codex/gpt-5.4-mini`):

```bash
mkdir -p "$SCRATCH/plain"
cd "$SCRATCH/plain"
mars build launch-bundle --model gpt-5.4-mini --harness pi --json
```

Representative output:

```json
{
  "version": 2,
  "routing": {
    "harness": "pi",
    "model": "gpt-5.4-mini",
    "selection_kind": "fixed",
    "match_evidence": "confirmed",
    "harness_model": "openai-codex/gpt-5.4-mini",
    "harness_model_source": "cached-probe",
    "harness_model_confidence": "confirmed"
  },
  "provenance": {
    "candidates_tried": "pi",
    "harness_source": "cli",
    "model_source": "cli",
    "selection_kind": "fixed",
    "match_evidence": "confirmed"
  },
  "warnings": []
}
```

Why: Mars resolves bare model IDs against the Pi probe and passes a qualified
`provider/model` slug to Meridian/Pi. Bare passthrough is no longer used when
the probe lists a matching slug.

Qualified CLI tokens pass through unchanged:

```bash
mars build launch-bundle --model openai-codex/gpt-5.4-mini --harness pi --json
# routing.harness_model == "openai-codex/gpt-5.4-mini", harness_model_source == "passthrough"
```

## Meridian smoke (after Mars bump)

From `meridian-cli` with updated Mars:

```bash
meridian spawn --harness pi -m gpt-5.4-mini -- <prompt>
```

Verify Pi argv includes `--model openai-codex/gpt-5.4-mini` (not bare
`gpt-5.4-mini`) and does **not** require `-- --provider openai-codex`.

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
  "version": 2,
  "routing": {
    "harness": "pi",
    "selection_kind": "fixed",
    "match_evidence": "passthrough",
    "harness_model": "gpt-5.4-mini",
    "harness_model_source": "passthrough",
    "harness_model_confidence": "unknown"
  },
  "provenance": {
    "candidates_tried": "",
    "harness_source": "profile",
    "model_source": "profile",
    "selection_kind": "fixed",
    "match_evidence": "passthrough"
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
  "version": 2,
  "routing": {
    "harness": "opencode",
    "selection_kind": "auto",
    "match_evidence": "confirmed",
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
  "version": 2,
  "routing": {
    "harness": "codex",
    "selection_kind": "linked_fallback",
    "match_evidence": "passthrough",
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
