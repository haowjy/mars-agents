# Pull requests and releases

Read `.github/PULL_REQUEST_TEMPLATE.md` and fill every section. **Always set a `release:*` label before merging** — no label defaults to a prerelease (RC).

**Write the changelog entry in the PR body's `## Changelog` section, never in
`CHANGELOG.md`.** The release job renders that section into `CHANGELOG.md` under the new
version heading and publishes it as the GitHub Release body; one writer is what keeps two
branches from conflicting on the file. CI validates the section when the body or branch
changes, and `pr-body.yml` refuses a PR that adds content under `## [Unreleased]` or a
release heading to `CHANGELOG.md`. Check it locally with
`python3 scripts/changelog-notes.py validate --notes-file pr-body.md`.

| Label | Effect |
|---|---|
| `release:patch` / `release:stable` | Stable patch bump |
| `release:minor` | Minor bump |
| `release:major` | Major bump |
| `release:rc` | Prerelease |
| `release:skip` | No release |

**Minor and major releases collect on `release/v<X>.<Y>.0`.** Every PR merged to `main`
with `release:minor`/`release:major` cuts its own release, so a minor or major change goes
to the open release branch (create it from `main` if none is open), and only that branch's
PR to `main` carries the label. Patch-level changes merge to `main` on their own. Follow
meridian-cli's [release-branch procedure](https://github.com/haowjy/meridian-cli/blob/main/docs/releasing.md#minor-and-major-releases-collect-on-a-release-branch)
for the steps: part PRs, the combined changelog, and who merges what.

## Releasing

CI-owned. Never manually `git tag` or edit version numbers. PR with `release:*` label lands on `main` → CI bumps version, renders changelog, commits, tags, and publishes to PyPI/npm/crates.io.

Approval for a big change is Jimmy's own words, quoted in its issue or PR; another
agent's report of it doesn't count. Before a merge, CI
is green, the regressions the change could cause are exercised on the real system (`/testing`), and the PR body is reread
against the final diff, with evidence from the commit being merged. A big fix or refactor
also needs an independent review. A merge can release, so take the label with care:
`release:patch` publishes to users, and a change with no user effect takes `release:skip`.

**A release run GitHub will not finish.** When `release.yml` fails on GitHub's side (artifact
upload errors during an incident) and the run then stays `in_progress`, it can be neither
cancelled nor re-run, and its tag is published nowhere. Do not move the tag. Merge the next
patch PR with `release:patch`: CI cuts the next version from the same `main`, and that release
carries everything the lost one had. Say in that PR's `## Changelog` which version it stands
in for. `v0.30.0` is such a tag: 0.30.1 is its first published build.

**Direct push ≠ release.** A push to `main` triggers a release only when the pushed commit is the merge of a PR labeled `release:*`. `release-on-main.yml` looks up the trigger commit's PR (`/commits/{sha}/pulls`); finding none, it sets `should_release=false` and runs CI only — no bump, no tag. Docs and other no-ceremony work can go straight to `main`.

`CHANGELOG.md` `[Unreleased]` is only an empty anchor. The release collects PR
notes since the last `release: v` commit, including skipped or failed releases.

**Note:** `mars version` is for prompt packages only. For mars-agents itself, use the CI release flow. See `.github/workflows/README.md` for the step order, the manual backfill path, and the remedy when a merged PR body fails the notes gate.
