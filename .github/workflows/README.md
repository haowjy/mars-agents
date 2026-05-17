# Workflow Release Model

## Normal CI

`ci.yml` runs on pull requests and pushes to `main`.

- PRs get a non-blocking release-label warning.
- The shared full gate is `scripts/preflight.sh full`.
- The local pre-push hook runs the same full gate before branch pushes.

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
- Duplicate guard is release-kind aware: RC skips when this trigger already has RC or stable tag; stable skips only when a stable tag exists (so RC can still be promoted to stable).

When release is enabled, the workflow:

1. Runs `scripts/preflight.sh full`.
2. Bumps Cargo, PyPI, and npm package versions.
3. Promotes `CHANGELOG.md` `[Unreleased]`.
4. Commits `release: vX.Y.Z` or `release: vX.Y.Z-rc.N`.
5. Creates and pushes `vX.Y.Z` or `vX.Y.Z-rc.N`.
6. Calls `release.yml` directly with that tag.

The direct workflow call is the auto-release publish path. To avoid duplicate
publish runs when checkout uses `RELEASE_TOKEN`, the tag push is forced through
`${{ github.token }}` / `GITHUB_TOKEN`, so the CI-created tag does not trigger
a second `release.yml` run from `push.tags`.

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
runs the shared preflight, blocks empty `[Unreleased]` releases, updates
versions and changelog, creates the release commit, and tags the result.

`scripts/manually-release.sh` does **not** create RC release commits. Manual RC
backfill requires a pre-existing valid RC release commit and an RC tag push
(`vX.Y.Z-rc.N`) that satisfies `release.yml` provenance checks.
