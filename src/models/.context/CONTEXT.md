# `resolve_harness_model` — selected launch argv model ID

`harness_model.rs` projects the selected routing assessment into the ID passed to
harness CLIs. Availability and launch-bundle policy call this same function; neither
reruns slug selection. Native-agent OpenCode emission also uses selected route evidence.

## Resolution order

1. Empty requested model → empty passthrough ID.
2. Native Claude/Codex → **requested spelling** (trimmed), never the normalized
   `chosen_model` from catalog matching. For example, a catalog match on
   `claude-opus-4-6` still launches `claude-opus-4.6` if that was requested.
3. Probe-backed Pi/OpenCode/Cursor → selected `chosen_slug`, then `chosen_model`,
   then requested ID. A selected slug is confirmed probe evidence.
4. Pi/OpenCode constrained passthrough may qualify an unqualified requested ID
   as `provider/model`. Cursor passthrough remains unqualified.

`provider_constraint` filters routing slug selection; it is never a blind prefix
before routing. A missing or failed Pi listing is support-unknown passthrough,
not evidence that the model is absent. A successful listing with a failed later
refresh keeps last-good support, but does not imply current auth.

P3 live-row association must compare model IDs with routing's normalized
`model_ids_match`, not raw equality, to associate punctuation/case variants.

## Related

- [`harness_model.rs`](../harness_model.rs)
- [`availability.rs`](../availability.rs)
- [`src/routing/.context/CONTEXT.md`](../../routing/.context/CONTEXT.md)
