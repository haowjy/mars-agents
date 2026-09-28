# src/curation/ — Display-Only Model Rules

`CuratedRules` reads three authored TOML tiers: user `curated.toml` under
`MARS_CONFIG_DIR` (otherwise the platform `mars/` config dir), project
`mars.curated.toml`, and local `mars.curated.local.toml`. This module never
writes them and never participates in routing, build, or alias resolution.

## Contract

- Strict schema: `inherit`, `default`, `[[show]]`, `[[hide]]`; reject unknown
  keys and unknown concrete harness names with file-named errors.
- A tier's strongest matching kind wins: literal hide > literal show > glob
  hide > glob show. Then fold user → project → local. A higher glob show cannot
  undo an effective lower hide; a literal show can. `inherit=false` removes all
  lower tiers before this fold.
- An unmatched row takes the highest explicit default. Without one, project or
  local shows imply hide; user-only shows stay additive.
- Only a concrete-harness, literal `show` may declare a non-Possible row. A
  normalized/bare match against an existing Possible row prevents a phantom.
  Out-of-scope declarations are dropped with one diagnostic per distinct rule;
  uninstalled declarations remain. Installation state belongs to `--live`, not
  curation's default diagnostics.
- `project` retains hidden rows for P3's `--all` view; `shown_rows` supplies the
  default display subset. Neither function changes Possible or Selection.
- `matcher(possible)` precomputes literal/full-launch precedence once; use its
  `decide` for bulk decisions rather than rebuilding context per row.

## Boundaries

Curation may import `models::possible`, `routing::slug` matching helpers, and
`harness::registry`. `routing/`, `build/`, and `models::possible` must never
import `curation`. P3's `cli/models` renderer is the intended consumer.

Tests live in `tests.rs` and exercise the full tier fold and file diagnostics.
