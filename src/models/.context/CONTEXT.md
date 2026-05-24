# `resolve_harness_model` — launch argv model id

Maps resolved canonical `model_id` + selected `harness` to `routing.harness_model` (the
token harness CLIs receive on `--model`). Alias `provider` feeds **routing and probe
selection**, not an unconditional `provider/model` prefix.

## Resolution order

1. **Empty `model_id`** → empty `harness_model`, `passthrough`, `unknown` confidence.
2. **Pi / OpenCode** → probe slug selection (`select_probe_slug`) when probe cache is
   compatible; `provider_constraint` biases slug choice only (see `probe_constraint_for_selection`).
   Without a usable probe → `constraint_qualified_passthrough` when the constraint is already
   qualified (`openai-codex/foo`), else bare passthrough.
3. **Native harnesses (`codex`, `claude`)** → when `provider_constraint` or `provider_for_order`
   matches the harness (`slug::provider_matches_native_harness`, including variants like
   `openai-codex` on Codex), return **bare** `model_id` with `provider-match` / `likely`.
4. **Default** → bare `model_id`, `passthrough`, `unknown`.

## Anti-patterns (removed)

Do **not** prepend `{provider_constraint}/{model_id}` before harness branches. That produced
`openai/gpt-5.4-mini` for aliases such as `gptmini` (`provider = "openai"`, `harness = "codex"`),
which breaks ChatGPT-auth Codex (expects bare `gpt-5.4-mini`) and Pi (expects probe slugs like
`openai-codex/gpt-5.4-mini`).

## Examples

| Input | Harness | Typical `harness_model` | Source |
|-------|---------|-------------------------|--------|
| alias `gptmini` | codex (alias-fixed) | `gpt-5.4-mini` | `provider-match` |
| alias `gptmini` | pi (CLI) | `openai-codex/gpt-5.4-mini` | `cached-probe` |
| CLI `gpt-5.4-mini` | codex (auto, native match) | `gpt-5.4-mini` | `provider-match` |

Manual evidence: `tests/smoke/manual/results-launch-bundle-resolver.md` (gptmini section).

## Related

- [`harness_model.rs`](../harness_model.rs) — implementation + unit tests
- [`src/build/.context/CONTEXT.md`](../../build/.context/CONTEXT.md) — bundle consumer contract
- [`src/routing/.context/CONTEXT.md`](../../routing/.context/CONTEXT.md) — harness selection vs argv model
