# Workflow Release Model

## Normal CI

`ci.yml` runs on pull requests and pushes to `main`.

- PRs get a non-blocking release-label warning.
- The shared full gate is `scripts/preflight.sh full`.
- The local pre-push hook runs the same full gate before branch pushes.

## PR Body Gate

`pr-body.yml` runs on pull requests and skips drafts: it fires on open, edit,
synchronize, reopen and ready-for-review. One step runs
`python3 scripts/changelog-notes.py validate` on the PR body's `## Changelog`
section, then compares the branch's `CHANGELOG.md` with the base branch's copy:

- A missing or malformed `## Changelog` section fails, naming the line.
- The PR adding content under `## [Unreleased]` fails; removing it passes.
- The PR adding a release heading (`## [X]`, including a second
  `## [Unreleased]`) fails; a typo fix inside a released section passes.

The gate is advisory until it is a required status check on `main`.

## Auto-Release

`release-on-main.yml` runs on every push to `main` (including merged PR commits),
but it only creates a release when the pushed commit is associated with a PR
that has a `release:*` label.

- `release:patch` or `release:stable` creates the next stable patch release.
- `release:rc` creates the next RC release (`vX.Y.Z-rc.N`).
- `release:skip` skips release.
- Any other `release:*` label defaults to RC release (safe default) and logs a notice.
- If labels conflict (for example `release:stable` + `release:rc`, or stable + unknown `release:*`), RC wins as the safe default.
- Missing `release:*` label skips release.
- Direct pushes to `main` skip release because there is no PR label to inspect.
- `release:skip` in the pushed head commit message also skips release.
- The workflow queries associated PRs via GitHub API and fails immediately if that API call fails.
- PR selection is deterministic: prefer merged PRs targeting `main` whose `merge_commit_sha` equals the trigger SHA; if none, fall back to merged PRs targeting `main`.
- If PR selection remains ambiguous (more than one merged candidate), the workflow fails instead of guessing labels.
- Labels are read only from the selected merged PR.
- Duplicate guard is release-kind aware: RC skips when this trigger already has RC or stable tag; stable skips only when a stable tag exists (so RC can still be promoted to stable).
- Release commits include `Release-Trigger: <trigger-sha>` in the commit body for exact trigger tracking.

When release is enabled, the workflow:

1. Computes the next version from `v*` tags.
2. Collects the `## Changelog` section of every merged PR since the last
   `release: v` commit (`scripts/collect-release-notes.sh`) and dry-runs the
   renderer, so a missing or malformed section fails in seconds. The walk is one
   `gh` call per first-parent commit, capped at 200 commits; above the cap it
   fails rather than drop notes.
3. Runs `scripts/preflight.sh full`.
4. Bumps Cargo, PyPI, and npm package versions.
5. Renders the collected notes into `CHANGELOG.md` under
   `## [X.Y.Z] - YYYY-MM-DD` (`scripts/changelog-notes.py render`), replacing
   the `[Unreleased]` promotion. `[Unreleased]` stays an empty anchor; content
   under it fails the render.
6. Commits `release: vX.Y.Z` or `release: vX.Y.Z-rc.N`.
7. Creates and pushes `vX.Y.Z` or `vX.Y.Z-rc.N`.
8. Lets the tag push trigger `release.yml`.

`release-on-main.yml` only creates release commits/tags. `release.yml` is the
only publish workflow identity, for both auto-release tags and manual backfill
tags. npm trusted publishing allows one trusted workflow per package, so all
publish paths converge on `.github/workflows/release.yml`. Its GitHub Release
job extracts the tag's `## [X.Y.Z]` section as the release body, so the notes a
contributor writes in the PR body are the release notes.

Reruns are idempotent by trigger marker:
- If the release tag already exists for the trigger, rerun does not recreate it.
- If the release commit exists but the expected tag is missing (partial success), rerun recreates/pushes the tag so `release.yml` can publish it.

If a merged PR's `## Changelog` section is missing or malformed, the collect
step fails before the preflight and names the PR: edit that merged PR body and
re-run the failed job. If the failure is content under `## [Unreleased]` (an old
branch's entry), move that text into the merged PR body, delete it from
`CHANGELOG.md` with a direct push to `main` (direct pushes release nothing), then
re-run the failed job. A range above the 200-commit cap needs a manual release
that carries the notes itself:

```bash
scripts/manually-release.sh patch --no-collect --notes-file notes.md --push
```

## Manual Backfill

`release.yml` also runs on manual `v*` tag pushes.

Manual tags must point at a valid release commit (stable or RC):

- Cargo and npm versions match the semver tag version.
- Every `npm/@meridian-flow/mars-agents*/package.json` has matching `version` and `optionalDependencies` values for the tag version.
- pyproject version matches PyPI form (`X.Y.Z` stable, `X.Y.ZrcN` for `X.Y.Z-rc.N` tags).
- `CHANGELOG.md` has the semver-tag release section (`## [X.Y.Z] - ...` or `## [X.Y.Z-rc.N] - ...`).
- Commit subject is `release: v<tag-version>`.
- The tagged commit is reachable from the default branch.

Use `scripts/manually-release.sh` for stable manual/backfill release commits. It
collects merged PR notes by default (same walk as CI, so it needs an
authenticated `gh`), takes an optional `--notes-file PATH` rendered after them,
runs the shared preflight, updates versions and renders the changelog, creates
the release commit, and tags the result. `--no-collect` releases from
`--notes-file` alone and then the file must carry every merged PR's notes since
the last release.

`scripts/manually-release.sh` does **not** create RC release commits. Manual RC
backfill requires a pre-existing valid RC release commit and an RC tag push
(`vX.Y.Z-rc.N`) that satisfies `release.yml` provenance checks.
