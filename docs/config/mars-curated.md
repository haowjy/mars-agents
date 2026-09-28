# Curated model rules

Mars can load display-only model curation from three TOML files, in increasing
precedence:

1. `$MARS_CONFIG_DIR/curated.toml`, or the platform config directory's
   `mars/curated.toml` when the variable is unset;
2. `<project>/mars.curated.toml` (shareable);
3. `<project>/mars.curated.local.toml` (gitignored, machine-local).

**These rules affect only `mars models list` display.** They do not affect
routing, launch bundles, or alias resolution.
No Possible catalog is persisted: Mars derives harness-model rows from the
existing models.dev cache and retained harness listings.

```toml
# Both top-level keys are optional.
inherit = true           # false drops lower tiers
default = "show"         # "show" or "hide" for unmatched rows

[[show]]
harness = "codex"      # claude, codex, pi, opencode, cursor, or "*"
model = "gpt-6-*"        # * crosses provider/model slashes
provider = "openai"     # optional; provider variants such as openai-codex match

[[hide]]
harness = "*"
model = "*-preview*"
```

Rules in one file are order-independent. Their specificity is **literal hide >
literal show > glob hide > glob show**. Mars then folds user, project, local
verdicts in that order. A higher-tier hide overrides a lower show. A higher
literal show can override a lower hide; a broad glob show cannot. An
`inherit = false` tier discards all lower rules and defaults.

Literal model IDs compare case-insensitively with `.` and `-` treated alike.
Provider-qualified literals first match the exact harness launch ID; bare
literals can match every provider copy of that model. A literal `[[show]]` for a
concrete harness can declare a row absent from the discovered Possible catalog;
`*` harnesses and glob patterns cannot declare rows. Out-of-scope declarations
are dropped with a deduplicated diagnostic. Uninstalled declarations remain
visible; `mars models list --live` reports their `not_installed` availability.

For native Claude/Codex rules, a matching provider-qualified literal such as
`anthropic/claude-opus-4-6` names the same row as its bare model ID. Its
launch ID remains bare; an incompatible provider qualifier is diagnosed.

For native provider globs, use `provider = "openai"` (or `"anthropic"`) plus
a model glob such as `model = "gpt-*"`; `model = "openai/gpt-*"` is not a
second native-glob dialect.

If no rule matches, the highest explicit `default` wins. Otherwise, project or
local files with any `[[show]]` imply `hide`; a user-only `[[show]]` remains
additive and leaves unrelated models visible. Missing or empty files show every
Possible row. Unknown keys, unknown concrete harnesses, unreadable files, and
malformed TOML are errors naming the file.
