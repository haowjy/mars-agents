# Model Provider Visibility

Status: implemented on `feat/model-provider-visibility`

## Summary

`mars models list` currently shows every merged alias, including models the
developer cannot run — no Claude Code, no Codex login, or no provider
subscription. Harness and auth gaps are already detected (`--live` prunes
`Unavailable`), but paid access has no machine-readable signal. This change lets
a developer **declare the providers they have access to** so the alias list
shows only what they can plausibly run. The declaration is a discovery filter:
it changes what is offered by default, never what can be named explicitly.

## Goals

- Declare a set of provider keys in config; `mars models list` shows only
  aliases whose resolved provider is in that set.
- Keep the declaration out of resolution and launch. `mars models resolve`,
  `-m <alias>`, `-m <plain-id>`, profiles, and model-policies are unaffected.
- Override the declaration: local config over project config, CLI over config,
  and a full bypass to see everything.
- Keep the existing harness/auth detection as the "can I actually run this"
  signal; the declaration covers the entitlement signal it cannot see.

## Non-goals

- No alias removal or tombstone.
- No gating of resolution or launch. A declared-out alias still resolves when
  named, and a plain model string still passes through to the harness.
- No automatic credit/balance/rate-limit detection. Those are not observable
  from the harness surface and are out of scope here.
- No change to `include`/`exclude` glob semantics.

## Background (behavior before this change)

`mars models list` resolves every alias from consumer + dependency config and
prints them. There is no availability or entitlement check unless `--live` is
passed. Three knobs exist today:

| Knob | Effect |
|---|---|
| `settings.model_visibility.include` / `exclude` | display filter, glob patterns over model id / `provider/model` / slug |
| `mars models list --include` / `--exclude` | per-invocation override; replaces config visibility entirely |
| `mars models list --live` | adds availability and prunes `Unavailable` (missing harness / unauthenticated / negative probe); keeps `Unknown` |

Observed before behavior:

```
$ mars models list
composer deepseek deepseekflash fable glm gpt gpt55 gptmini grok kimi luna
opus opus46 opus46[1m] opus48 sol sonnet sonnet5 terra deepseekpro astra

$ mars models list --include 'xai/*,deepseek/*,openai/*'    # glob workaround
deepseek deepseekflash gpt gpt55 gptmini grok luna sol terra deepseekpro astra

$ mars models resolve opus46
Model: claude-opus-4-6          # still resolves even when hidden
```

The gap: there is no first-class way to say "I have xai + deepseek + openai."
The glob form is fragile (a pattern like `opencode-go/*` cannot match `glm`,
whose model id is itself `opencode-go/glm-5.2`), and it is only a display
filter with no provider semantics.

## Requirements

- **R1** New `providers` list on `[settings.model_visibility]`, accepted in
  both `mars.toml` and `mars.local.toml`.
- **R2** Unset `providers` means no filter (current behavior).
- **R3** When set, `mars models list` shows only aliases whose resolved
  `provider` matches one of the listed keys.
- **R4** Display-only. Resolution, launch, profiles, and policies are
  untouched. `filter_by_visibility` is only called from list handlers.
- **R5** Precedence: CLI flag > `mars.local.toml` > `mars.toml` > unset.
- **R6** `mars models list --providers a,b` replaces config visibility for that
  invocation, consistent with `--include`/`--exclude`.
- **R7** `mars models list --no-visibility` bypasses all visibility filters and
  shows every alias.
- **R8** Empty and blank entries are ignored; an effectively-empty allow-list
  disables the provider filter, consistent with `include`/`exclude` and with
  leaving `providers` unset.
- **R9** Applies to every list view: default, `--all`, `--live`, `--catalog`.
- **R10** Matching is case-insensitive and collapses known provider variants
  (`openai-codex` matches `openai`, `anthropic-claude` matches `anthropic`).

## Design

### Config schema

```toml
# mars.toml — project default
[settings.model_visibility]
providers = ["xai", "deepseek", "openai"]

# mars.local.toml — machine override (replaces the list)
[settings.model_visibility]
providers = ["xai", "deepseek"]
```

`providers: Option<Vec<String>>` is added to `ModelVisibility` and
`LocalModelVisibility`.

### Provider matching

Match with `routing::slug::providers_match` (normalize, then compare). This
gives case-insensitive matching for cache display names (`DeepSeek`, `OpenAI`,
`Anthropic`) and variant collapsing for harness-suffixed keys. It is exact, not
glob, which sidesteps the slash-segment limitation of `matches_visibility_pattern`.

A provider is matched against the alias's **resolved** provider
(`ResolvedAlias.provider`), not the harness and not the access channel. `grok`
is `provider = xai` routed through opencode; declaring `xai` admits it. A model
reached through a reseller key must declare that key (`opencode-go`).

### Filter order

`include` and `providers` both narrow (intersection); `exclude` then removes:

```
visible = (matches(include) or include unset)
      and (matches(providers) or providers unset)
      and not matches(exclude)
```

### Scope boundary

The declaration is consulted only in the list seam. An alias that is declared
out is still resolvable when named, and a plain model string still flows to the
harness:

```
mars models list --providers xai,deepseek,openai   # opus46 hidden
mars models resolve opus46                         # resolves
mars spawn -a coder -m opus46                      # launches (harness decides)
mars spawn -a coder -m some-vendor/new-model       # passthrough, harness decides
```

## Behavior spec (after)

### Example A — declare providers

```toml
[settings.model_visibility]
providers = ["xai", "deepseek", "openai"]
```

```
$ mars models list
deepseek deepseekflash gpt gpt55 gptmini grok luna sol terra deepseekpro astra
```

`opus*`, `sonnet*`, `fable` (Anthropic), `glm` (opencode-go), `kimi` (unknown),
`composer` (cursor) are hidden.

### Example B — CLI override

```
$ mars models list --providers xai
grok

$ mars models list --providers xai,deepseek
grok deepseek deepseekflash deepseekpro
```

### Example C — bypass

```
$ mars models list --no-visibility
composer deepseek deepseekflash fable glm gpt gpt55 gptmini grok kimi luna
opus opus46 opus46[1m] opus48 sol sonnet sonnet5 terra deepseekpro astra
```

### Example D — explicit naming is never blocked

```
$ mars models list --providers deepseek      # opus46 hidden from the list
$ mars models resolve opus46                 # Model: claude-opus-4-6
$ mars spawn -a coder -m claude-opus-4-6     # plain id passes through
```

### Before → after

| Scenario | Before | After |
|---|---|---|
| No config | all aliases shown | unchanged |
| `providers = ["xai","deepseek","openai"]` | setting does not exist | only those providers shown |
| Hide a provider with a slashy model id (`glm`) | glob workaround fails | `providers` excludes it |
| `resolve` a declared-out alias | resolves | resolves (unchanged) |
| `-m <plain id>` | passthrough | passthrough (unchanged) |
| See everything despite config | edit the file | `--no-visibility` |

## Edge cases

- **Unknown provider** (`kimi` resolves to `unknown`): always filtered out when
  `providers` is set. Declaring a provider cannot re-admit an alias whose
  provider is unknown.
- **Auto-resolve aliases**: match against the provider of the winning catalog
  entry. A new release that moves an alias to a different provider changes its
  visibility accordingly.
- **Pinned aliases**: same rule; pinned aliases are not exempt (unlike
  `catalog_providers`, which only constrains auto-resolve).
- **Empty vs unset**: unset and `providers = []` both mean no filter. The
  predicate trims entries and drops blanks, so a list of only blanks is also
  treated as unset.
- **`--catalog`**: the raw catalog view honors `providers` via the same
  predicate.

## Implementation plan

1. `src/config/mod.rs`
   - Add `providers: Option<Vec<String>>` to `ModelVisibility` and
     `LocalModelVisibility`.
   - Extend `ModelVisibility::is_empty` to include `providers`.
   - No new validation rule: empty/blank handling lives in the predicate so the
     config and CLI paths behave identically (the list path does not run
     `validate`).
2. `src/config/layering.rs`
   - `apply_model_visibility_overlay` copies `providers` (replace).
3. `src/models/mod.rs`
   - Add `visibility_permits(visibility, model_id, provider, paths) -> bool`
     applying include ∩ providers then exclude.
   - `filter_by_visibility` uses it; early-return only when visibility is empty.
   - Unit tests for providers-only, providers + exclude, variant/case matching,
     unknown provider dropped.
4. `src/cli/models.rs`
   - `ListArgs`: add `--providers` (comma-delimited, conflicts with
     `--no-visibility`) and `--no-visibility`.
   - `effective_visibility`: `--no-visibility` returns default; any of
     include/exclude/providers returns a fresh visibility from flags.
   - `filter_model_entries_by_visibility` uses `visibility_permits`.
5. Docs: update `docs/config/mars-toml.md` Model Visibility section and the
   settings table. Add a `CHANGELOG.md` entry.

## Test plan

- Unit (`src/models/mod.rs`): providers allow-list keeps only matching;
  providers + exclude ordering; variant collapsing (`openai` matches
  `openai-codex`); case-insensitivity; unknown provider dropped; `unknown`
  declared as a key does not re-admit unresolved aliases; empty/blank lists and
  blank entries are ignored; entries are trimmed.
- Config (`src/config/mod.rs`): overlay replaces the project list; roundtrip
  preserves `providers`.
- Integration (`tests/model_config.rs`): config `providers` filtering and
  `mars.local.toml` override; `--providers` flag override; `--no-visibility`
  and its conflict with `--providers`; `resolve` still works for a hidden
  alias.

## Decisions

- **Display-only.** Explicit naming and passthrough are never blocked.
  Authorization, if ever needed, is a separate `deny` that gates resolution.
- **Replace, not union,** for local over project and CLI over config. An
  allow-list you cannot narrow is not an allow-list.
- **Exact normalized match, not glob,** for providers. Avoids the
  slash-segment limitation and keeps the schema readable.
- **Empty means unset.** `providers = []` and blank entries disable the filter,
  matching `include`/`exclude`. Validation that fired only in `mars validate`
  (not `models list`) split the two paths; normalizing in the predicate keeps
  them identical.
