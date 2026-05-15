# Workflow Release Model

## Normal CI

`ci.yml` runs on pull requests and pushes to `main`.

- PRs get a non-blocking release-label warning.
- The shared full gate is `scripts/preflight.sh full`.
- The local pre-push hook runs the same full gate before branch pushes.

## Auto-Release

`release-on-main.yml` runs on pushes to `main`, but it only creates a release
when the pushed commit is associated with a PR that has a `release:*` label.

- `release:patch` creates the next patch release.
- `release:skip` skips release.
- Missing `release:*` label skips release.
- Direct pushes to `main` skip release because there is no PR label to inspect.
- `release:skip` in the pushed head commit message also skips release.

When release is enabled, the workflow:

1. Runs `scripts/preflight.sh full`.
2. Bumps Cargo, PyPI, and npm package versions.
3. Promotes `CHANGELOG.md` `[Unreleased]`.
4. Commits `release: vX.Y.Z`.
5. Creates and pushes `vX.Y.Z`.
6. Calls `release.yml` directly with that tag.

The direct workflow call avoids depending on GitHub firing a second workflow from
a CI-created tag.

## Manual Backfill

`release.yml` also runs on manual `v*` tag pushes.

Manual tags must point at a valid release commit:

- Cargo, PyPI, and npm versions match the tag version.
- `CHANGELOG.md` has the promoted release section.
- Commit subject is `release: vX.Y.Z`.
- The tagged commit is reachable from the default branch.

Use `scripts/manually-release.sh` for manual/backfill releases. It runs the
shared preflight, blocks empty `[Unreleased]` releases, updates versions and
changelog, creates the release commit, and tags the result.
